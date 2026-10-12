use std::fs::{self, File};
use std::io::{Read, Write};
use std::net::UdpSocket;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Mutex;
use std::time::{Duration, Instant};

use super::{
    Peer, RemoteLibrary, RemoteTrack, ServerInfo, CODE_HEADER, DISCOVERY_MSG, DISCOVERY_PORT,
    HTTP_PORT,
};
use crate::{Error, Library, Result, Track};

fn agent(global_timeout: Option<Duration>) -> ureq::Agent {
    ureq::Agent::config_builder()
        // Es tráfico de la red local: nunca debe pasar por un proxy.
        .proxy(None)
        .timeout_connect(Some(Duration::from_secs(5)))
        .timeout_global(global_timeout)
        .build()
        .into()
}

/// Acepta "192.168.1.20", "192.168.1.20:47810" o "http://…" y devuelve `ip:puerto`.
pub fn normalize_address(input: &str) -> String {
    let s = input
        .trim()
        .trim_start_matches("http://")
        .trim_end_matches('/');
    if s.rsplit_once(':')
        .is_some_and(|(_, p)| p.parse::<u16>().is_ok())
    {
        s.to_string()
    } else {
        format!("{s}:{HTTP_PORT}")
    }
}

fn net_err(e: ureq::Error) -> Error {
    Error::Network(match e {
        ureq::Error::StatusCode(401) => "Código incorrecto".into(),
        ureq::Error::StatusCode(423) => {
            "Demasiados intentos: desactiva y vuelve a activar «Compartir por Wi‑Fi» en la PC"
                .into()
        }
        ureq::Error::StatusCode(404) => "La canción ya no está disponible en la PC".into(),
        ureq::Error::StatusCode(c) => format!("La PC respondió con un error ({c})"),
        ureq::Error::Io(_) | ureq::Error::ConnectionFailed | ureq::Error::HostNotFound => {
            "No se pudo conectar con la PC. ¿Estáis en la misma red Wi‑Fi?".into()
        }
        ureq::Error::Timeout(_) => "La PC tardó demasiado en responder".into(),
        other => other.to_string(),
    })
}

/// Busca PCs con BlueWhale compartiendo en la red local.
pub fn discover(wait: Duration) -> Vec<Peer> {
    discover_at(&["255.255.255.255".to_string()], wait)
}

pub(super) fn discover_at(targets: &[String], wait: Duration) -> Vec<Peer> {
    let mut peers: Vec<Peer> = Vec::new();
    let Ok(sock) = UdpSocket::bind("0.0.0.0:0") else {
        return peers;
    };
    let _ = sock.set_broadcast(true);
    let _ = sock.set_read_timeout(Some(Duration::from_millis(200)));
    let deadline = Instant::now() + wait;
    let mut next_ping = Instant::now();
    let mut buf = [0u8; 1024];
    while Instant::now() < deadline {
        // Reenviamos el aviso varias veces por si se pierde algún paquete.
        if Instant::now() >= next_ping {
            for t in targets {
                let _ = sock.send_to(DISCOVERY_MSG, (t.as_str(), DISCOVERY_PORT));
            }
            next_ping = Instant::now() + Duration::from_millis(500);
        }
        let Ok((n, from)) = sock.recv_from(&mut buf) else {
            continue;
        };
        let Ok(info) = serde_json::from_slice::<ServerInfo>(&buf[..n]) else {
            continue;
        };
        if info.app != "BlueWhale" {
            continue;
        }
        let address = format!("{}:{}", from.ip(), info.port);
        if !peers.iter().any(|p| p.address == address) {
            peers.push(Peer {
                name: info.name,
                address,
            });
        }
    }
    peers
}

/// Descarga el catálogo de la PC.
pub fn fetch_library(address: &str, code: &str) -> Result<RemoteLibrary> {
    let url = format!("http://{}/library", normalize_address(address));
    let mut res = agent(Some(Duration::from_secs(60)))
        .get(&url)
        .header(CODE_HEADER, code.trim())
        .call()
        .map_err(net_err)?;
    res.body_mut()
        .with_config()
        .limit(200 * 1024 * 1024)
        .read_json::<RemoteLibrary>()
        .map_err(net_err)
}

/// Descarga una canción directamente al almacenamiento de la app y la añade
/// a la biblioteca. `cancel` permite interrumpir a mitad del archivo.
pub fn download_track(
    address: &str,
    code: &str,
    track: &RemoteTrack,
    library: &Mutex<Library>,
    cancel: &AtomicBool,
    mut on_bytes: impl FnMut(u64),
) -> Result<Track> {
    let music_dir = {
        let lib = library.lock().map_err(|e| Error::Network(e.to_string()))?;
        lib.music_dir().to_path_buf()
    };
    let url = format!("http://{}/track/{}", normalize_address(address), track.id);
    let res = agent(None)
        .get(&url)
        .header(CODE_HEADER, code.trim())
        .call()
        .map_err(net_err)?;

    let tmp = music_dir.join(format!(".descarga-{}.part", fastrand::u64(..)));
    let result = (|| -> Result<()> {
        let mut reader = res.into_body().into_reader();
        let mut file = File::create(&tmp)?;
        let mut buf = vec![0u8; 64 * 1024];
        let mut total = 0u64;
        loop {
            if cancel.load(Ordering::Relaxed) {
                return Err(Error::Network("Descarga cancelada".into()));
            }
            let n = reader.read(&mut buf)?;
            if n == 0 {
                break;
            }
            file.write_all(&buf[..n])?;
            total += n as u64;
            on_bytes(n as u64);
        }
        file.flush()?;
        if total != track.size {
            return Err(Error::Network(format!(
                "«{}» llegó incompleta",
                track.title
            )));
        }
        Ok(())
    })();
    if let Err(e) = result {
        let _ = fs::remove_file(&tmp);
        return Err(e);
    }

    let lib = library.lock().map_err(|e| Error::Network(e.to_string()))?;
    lib.adopt_file(&tmp, &track.file_name)
}
