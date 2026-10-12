use std::fs::File;
use std::net::UdpSocket;
use std::path::Path;
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::Duration;

use tiny_http::{Header, Request, Response, StatusCode};

use super::{
    track_key, RemoteLibrary, RemotePlaylist, RemoteTrack, ServerInfo, CODE_HEADER, DISCOVERY_MSG,
    DISCOVERY_PORT, HTTP_PORT, MAX_FAILED_ATTEMPTS,
};
use crate::{Error, Library, Result};

struct Shared {
    library: Arc<Mutex<Library>>,
    code: String,
    info: ServerInfo,
    failures: AtomicU32,
    sent: AtomicU64,
    stop: AtomicBool,
}

/// Servidor que comparte la biblioteca en la red local mientras está vivo.
/// Al soltarlo (`drop`) deja de escuchar.
pub struct ShareServer {
    shared: Arc<Shared>,
    http: Arc<tiny_http::Server>,
}

fn device_name() -> String {
    let name = gethostname::gethostname()
        .to_string_lossy()
        .trim()
        .to_string();
    if name.is_empty() {
        "Mi PC".into()
    } else {
        name
    }
}

impl ShareServer {
    pub fn start(library: Arc<Mutex<Library>>) -> Result<Self> {
        let http = tiny_http::Server::http(("0.0.0.0", HTTP_PORT))
            .or_else(|_| tiny_http::Server::http(("0.0.0.0", 0)))
            .map_err(|e| Error::Network(format!("no se pudo abrir el servidor: {e}")))?;
        let port = http
            .server_addr()
            .to_ip()
            .map(|a| a.port())
            .ok_or_else(|| Error::Network("dirección del servidor desconocida".into()))?;

        let shared = Arc::new(Shared {
            library,
            code: format!("{:06}", fastrand::u32(0..1_000_000)),
            info: ServerInfo {
                app: "BlueWhale".into(),
                name: device_name(),
                version: env!("CARGO_PKG_VERSION").into(),
                port,
            },
            failures: AtomicU32::new(0),
            sent: AtomicU64::new(0),
            stop: AtomicBool::new(false),
        });
        let http = Arc::new(http);

        {
            let (http, shared) = (http.clone(), shared.clone());
            thread::spawn(move || {
                for req in http.incoming_requests() {
                    if shared.stop.load(Ordering::Relaxed) {
                        break;
                    }
                    let shared = shared.clone();
                    thread::spawn(move || handle(&shared, req));
                }
            });
        }
        {
            let shared = shared.clone();
            thread::spawn(move || discovery_loop(&shared));
        }
        Ok(Self { shared, http })
    }

    pub fn code(&self) -> &str {
        &self.shared.code
    }
    pub fn port(&self) -> u16 {
        self.shared.info.port
    }
    pub fn name(&self) -> &str {
        &self.shared.info.name
    }
    /// Canciones enviadas desde que se activó.
    pub fn sent(&self) -> u64 {
        self.shared.sent.load(Ordering::Relaxed)
    }
    pub fn locked(&self) -> bool {
        self.shared.failures.load(Ordering::Relaxed) >= MAX_FAILED_ATTEMPTS
    }
}

impl Drop for ShareServer {
    fn drop(&mut self) {
        self.shared.stop.store(true, Ordering::Relaxed);
        self.http.unblock();
    }
}

/// Direcciones IP de este equipo en la red local (para mostrarlas en la PC).
pub fn local_ips() -> Vec<String> {
    let mut ips = Vec::new();
    // Truco habitual: "conectar" un socket UDP no envía nada, pero revela qué
    // interfaz usaría el sistema para salir.
    for probe in ["8.8.8.8:80", "192.168.0.1:80", "10.0.0.1:80"] {
        if let Ok(sock) = UdpSocket::bind("0.0.0.0:0") {
            if sock.connect(probe).is_ok() {
                if let Ok(addr) = sock.local_addr() {
                    let ip = addr.ip();
                    if !ip.is_loopback() && !ip.is_unspecified() && !ips.contains(&ip.to_string()) {
                        ips.push(ip.to_string());
                    }
                }
            }
        }
    }
    ips
}

fn discovery_loop(shared: &Shared) {
    let Ok(sock) = UdpSocket::bind(("0.0.0.0", DISCOVERY_PORT)) else {
        // Otro programa usa el puerto: el teléfono podrá conectar a mano.
        return;
    };
    let _ = sock.set_read_timeout(Some(Duration::from_millis(500)));
    let reply = serde_json::to_vec(&shared.info).unwrap_or_default();
    let mut buf = [0u8; 64];
    while !shared.stop.load(Ordering::Relaxed) {
        if let Ok((n, from)) = sock.recv_from(&mut buf) {
            if &buf[..n] == DISCOVERY_MSG {
                let _ = sock.send_to(&reply, from);
            }
        }
    }
}

fn json<T: serde::Serialize>(value: &T) -> Response<std::io::Cursor<Vec<u8>>> {
    let body = serde_json::to_vec(value).unwrap_or_default();
    Response::from_data(body)
        .with_header(Header::from_bytes("Content-Type", "application/json; charset=utf-8").unwrap())
}

fn status(code: u16, msg: &str) -> Response<std::io::Cursor<Vec<u8>>> {
    Response::from_string(msg).with_status_code(StatusCode(code))
}

fn authorized(shared: &Shared, req: &Request) -> std::result::Result<(), u16> {
    if shared.failures.load(Ordering::Relaxed) >= MAX_FAILED_ATTEMPTS {
        return Err(423);
    }
    let given = req
        .headers()
        .iter()
        .find(|h| h.field.equiv(CODE_HEADER))
        .map(|h| h.value.as_str().trim().to_string())
        .unwrap_or_default();
    if given == shared.code {
        Ok(())
    } else {
        shared.failures.fetch_add(1, Ordering::Relaxed);
        Err(401)
    }
}

fn handle(shared: &Shared, req: Request) {
    let url = req.url().split('?').next().unwrap_or("").to_string();
    if url == "/info" {
        let _ = req.respond(json(&shared.info));
        return;
    }
    if let Err(code) = authorized(shared, &req) {
        let msg = if code == 423 {
            "bloqueado"
        } else {
            "código incorrecto"
        };
        let _ = req.respond(status(code, msg));
        return;
    }

    if url == "/library" {
        match build_library(shared) {
            Ok(lib) => {
                let _ = req.respond(json(&lib));
            }
            Err(e) => {
                let _ = req.respond(status(500, &e.to_string()));
            }
        }
        return;
    }

    if let Some(id) = url
        .strip_prefix("/track/")
        .and_then(|s| s.parse::<i64>().ok())
    {
        let path = shared
            .library
            .lock()
            .ok()
            .and_then(|l| l.track(id).ok())
            .map(|t| t.path);
        let Some(path) = path else {
            let _ = req.respond(status(404, "no encontrada"));
            return;
        };
        match File::open(&path) {
            Ok(file) => {
                let res = Response::from_file(file).with_header(
                    Header::from_bytes("Content-Type", "application/octet-stream").unwrap(),
                );
                if req.respond(res).is_ok() {
                    shared.sent.fetch_add(1, Ordering::Relaxed);
                }
            }
            Err(_) => {
                let _ = req.respond(status(404, "archivo no disponible"));
            }
        }
        return;
    }

    let _ = req.respond(status(404, "ruta desconocida"));
}

fn build_library(shared: &Shared) -> Result<RemoteLibrary> {
    let lib = shared
        .library
        .lock()
        .map_err(|e| Error::Network(e.to_string()))?;
    let tracks = lib
        .tracks()?
        .into_iter()
        .filter_map(|t| {
            let size = std::fs::metadata(&t.path).ok()?.len();
            let file_name = Path::new(&t.path)
                .file_name()?
                .to_string_lossy()
                .into_owned();
            Some(RemoteTrack {
                id: t.id,
                key: track_key(&t.title, &t.artist, &t.album, size),
                title: t.title,
                artist: t.artist,
                album: t.album,
                album_artist: t.album_artist,
                track_no: t.track_no,
                disc_no: t.disc_no,
                year: t.year,
                duration: t.duration,
                size,
                file_name,
                on_device: false,
            })
        })
        .collect();
    let playlists = lib
        .playlists()?
        .into_iter()
        .map(|p| RemotePlaylist {
            id: p.id,
            name: p.name,
            track_ids: p.track_ids,
        })
        .collect();
    Ok(RemoteLibrary {
        name: shared.info.name.clone(),
        tracks,
        playlists,
    })
}
