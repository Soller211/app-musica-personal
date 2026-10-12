//! Importar canciones desde un enlace directo (http/https).
//!
//! Acepta archivos de audio sueltos y archivos `.zip` (por ejemplo, un álbum
//! comprado en Bandcamp), de los que se importan todas las canciones.

use std::fs::{self, File};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Mutex;
use std::time::{Duration, Instant};

use serde::Serialize;

use crate::metadata::is_audio_file;
use crate::{Error, Library, Result, Track};

/// Tamaño máximo de una descarga (un álbum en FLAC cabe de sobra).
pub const MAX_DOWNLOAD_BYTES: u64 = 2 * 1024 * 1024 * 1024;

#[derive(Debug, Clone, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LinkProgress {
    pub name: String,
    pub bytes_done: u64,
    /// `None` si el servidor no dice el tamaño.
    pub bytes_total: Option<u64>,
}

fn net(msg: impl Into<String>) -> Error {
    Error::Network(msg.into())
}

fn lock(l: &Mutex<Library>) -> Result<std::sync::MutexGuard<'_, Library>> {
    l.lock().map_err(|e| net(e.to_string()))
}

fn agent() -> ureq::Agent {
    ureq::Agent::config_builder()
        .timeout_connect(Some(Duration::from_secs(15)))
        .timeout_recv_response(Some(Duration::from_secs(30)))
        .user_agent(concat!("BlueWhale/", env!("CARGO_PKG_VERSION")))
        .build()
        .into()
}

fn map_err(e: ureq::Error) -> Error {
    net(match e {
        ureq::Error::StatusCode(404) => "No se encontró nada en ese enlace (404)".into(),
        ureq::Error::StatusCode(401 | 403) => {
            "El enlace no permite descargar (puede que haya caducado o requiera iniciar sesión)"
                .into()
        }
        ureq::Error::StatusCode(c) => format!("El servidor respondió con un error ({c})"),
        ureq::Error::HostNotFound | ureq::Error::ConnectionFailed | ureq::Error::Io(_) => {
            "No se pudo conectar. ¿Tienes internet?".into()
        }
        ureq::Error::Timeout(_) => "El servidor tardó demasiado en responder".into(),
        other => other.to_string(),
    })
}

/// Comprueba que sea un enlace http(s) y lo limpia.
pub fn validate_url(input: &str) -> Result<String> {
    let url = input.trim();
    let lower = url.to_ascii_lowercase();
    if !(lower.starts_with("http://") || lower.starts_with("https://")) {
        return Err(net("Pega un enlace que empiece por http:// o https://"));
    }
    if url.contains(char::is_whitespace) {
        return Err(net("El enlace contiene espacios"));
    }
    Ok(url.to_string())
}

fn percent_decode(s: &str) -> String {
    let bytes = s.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' && i + 2 < bytes.len() {
            let hex = std::str::from_utf8(&bytes[i + 1..i + 3]).ok();
            if let Some(b) = hex.and_then(|h| u8::from_str_radix(h, 16).ok()) {
                out.push(b);
                i += 3;
                continue;
            }
        }
        out.push(bytes[i]);
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

/// Nombre de archivo de una cabecera `Content-Disposition`.
fn filename_from_disposition(value: &str) -> Option<String> {
    let mut plain = None;
    for part in value.split(';').map(str::trim) {
        let lower = part.to_ascii_lowercase();
        if let Some(v) = lower.strip_prefix("filename*=").map(|_| &part[10..]) {
            // filename*=UTF-8''nombre%20codificado.mp3
            let v = v.trim_matches('"');
            let encoded = v.split_once("''").map(|(_, n)| n).unwrap_or(v);
            return Some(percent_decode(encoded));
        }
        if lower.starts_with("filename=") {
            plain = Some(part[9..].trim_matches('"').to_string());
        }
    }
    plain.filter(|s| !s.is_empty())
}

/// Último segmento de la ruta de la URL (sin `?…` ni `#…`).
fn filename_from_url(url: &str) -> Option<String> {
    let no_query = url.split(['?', '#']).next()?;
    let path = no_query.split_once("://").map(|(_, r)| r)?;
    let (_, path) = path.split_once('/')?;
    let last = path.rsplit('/').next()?;
    let name = percent_decode(last);
    (!name.trim().is_empty()).then_some(name)
}

fn ext_from_content_type(ct: &str) -> Option<&'static str> {
    let ct = ct.split(';').next()?.trim().to_ascii_lowercase();
    Some(match ct.as_str() {
        "audio/mpeg" | "audio/mp3" => "mp3",
        "audio/mp4" | "audio/x-m4a" | "audio/m4a" => "m4a",
        "audio/aac" | "audio/x-aac" => "aac",
        "audio/ogg" | "application/ogg" => "ogg",
        "audio/opus" => "opus",
        "audio/flac" | "audio/x-flac" => "flac",
        "audio/wav" | "audio/x-wav" | "audio/wave" => "wav",
        "audio/aiff" | "audio/x-aiff" => "aiff",
        "application/zip" | "application/x-zip-compressed" => "zip",
        _ => return None,
    })
}

fn is_zip(name: &str) -> bool {
    name.to_ascii_lowercase().ends_with(".zip")
}

/// Decide el nombre final del archivo a partir de cabeceras y URL.
fn pick_name(disposition: Option<&str>, content_type: Option<&str>, url: &str) -> Result<String> {
    let ct_ext = content_type.and_then(ext_from_content_type);
    let candidate = disposition
        .and_then(filename_from_disposition)
        .or_else(|| filename_from_url(url))
        .unwrap_or_else(|| "cancion".into());
    let candidate = Path::new(&candidate)
        .file_name()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_else(|| "cancion".into());

    if is_audio_file(Path::new(&candidate)) || is_zip(&candidate) {
        return Ok(candidate);
    }
    if let Some(ext) = ct_ext {
        let stem = Path::new(&candidate)
            .file_stem()
            .map(|s| s.to_string_lossy().into_owned())
            .unwrap_or_else(|| "cancion".into());
        return Ok(format!("{stem}.{ext}"));
    }
    let is_html = content_type.is_some_and(|ct| ct.to_ascii_lowercase().contains("text/html"));
    Err(net(if is_html {
        "El enlace abre una página web, no un archivo de audio. Copia el enlace directo de descarga."
    } else {
        "El enlace no parece ser un archivo de audio (MP3, M4A, FLAC, OGG, WAV…) ni un ZIP"
    }))
}

/// Descarga `url` e importa su contenido. Devuelve las canciones añadidas.
pub fn import_url(
    url: &str,
    library: &Mutex<Library>,
    cancel: &AtomicBool,
    mut progress: impl FnMut(&LinkProgress),
) -> Result<Vec<Track>> {
    let url = validate_url(url)?;
    let music_dir = lock(library)?.music_dir().to_path_buf();

    let res = agent().get(&url).call().map_err(map_err)?;
    let header = |name: &str| {
        res.headers()
            .get(name)
            .and_then(|v| v.to_str().ok())
            .map(str::to_string)
    };
    let final_url = ureq::ResponseExt::get_uri(&res).to_string();
    let name = pick_name(
        header("content-disposition").as_deref(),
        header("content-type").as_deref(),
        &final_url,
    )?;
    let total = header("content-length").and_then(|v| v.parse::<u64>().ok());
    if total.is_some_and(|t| t > MAX_DOWNLOAD_BYTES) {
        return Err(net("El archivo es demasiado grande (más de 2 GB)"));
    }

    let mut state = LinkProgress {
        name: name.clone(),
        bytes_done: 0,
        bytes_total: total,
    };
    progress(&state);
    let tmp = music_dir.join(format!(".enlace-{}.part", fastrand::u64(..)));
    let download = (|| -> Result<()> {
        let mut reader = res
            .into_body()
            .into_with_config()
            .limit(MAX_DOWNLOAD_BYTES)
            .reader();
        let mut file = File::create(&tmp)?;
        let mut buf = vec![0u8; 64 * 1024];
        let mut last = Instant::now();
        loop {
            if cancel.load(Ordering::Relaxed) {
                return Err(net("Descarga cancelada"));
            }
            let n = reader
                .read(&mut buf)
                .map_err(|e| net(format!("La descarga se cortó: {e}")))?;
            if n == 0 {
                break;
            }
            file.write_all(&buf[..n])?;
            state.bytes_done += n as u64;
            if last.elapsed() > Duration::from_millis(150) {
                last = Instant::now();
                progress(&state);
            }
        }
        file.flush()?;
        progress(&state);
        Ok(())
    })();
    if let Err(e) = download {
        let _ = fs::remove_file(&tmp);
        return Err(e);
    }

    if is_zip(&name) {
        let result = import_zip(&tmp, &music_dir, library, cancel);
        let _ = fs::remove_file(&tmp);
        return result;
    }
    let lib = lock(library)?;
    match lib.adopt_file(&tmp, &name) {
        Ok(t) => Ok(vec![t]),
        Err(Error::Metadata { .. }) => Err(net(
            "Se descargó el archivo, pero no es un audio válido o está dañado",
        )),
        Err(e) => Err(e),
    }
}

/// Extrae e importa todas las canciones de un ZIP.
fn import_zip(
    zip_path: &Path,
    music_dir: &Path,
    library: &Mutex<Library>,
    cancel: &AtomicBool,
) -> Result<Vec<Track>> {
    let file = File::open(zip_path)?;
    let mut archive =
        zip::ZipArchive::new(file).map_err(|e| net(format!("El ZIP está dañado: {e}")))?;
    let mut tracks = Vec::new();
    let mut errors = 0;
    for i in 0..archive.len() {
        if cancel.load(Ordering::Relaxed) {
            break;
        }
        let mut entry = archive
            .by_index(i)
            .map_err(|e| net(format!("El ZIP está dañado: {e}")))?;
        if !entry.is_file() {
            continue;
        }
        // `enclosed_name` descarta rutas peligrosas como "../../algo".
        let Some(name) = entry
            .enclosed_name()
            .and_then(|p| p.file_name().map(|n| n.to_string_lossy().into_owned()))
        else {
            continue;
        };
        if name.starts_with("._") || !is_audio_file(Path::new(&name)) {
            continue;
        }
        let tmp: PathBuf = music_dir.join(format!(".zip-{}.part", fastrand::u64(..)));
        let copied = (|| -> Result<()> {
            let mut out = File::create(&tmp)?;
            std::io::copy(&mut entry, &mut out)?;
            Ok(())
        })();
        if copied.is_err() {
            let _ = fs::remove_file(&tmp);
            errors += 1;
            continue;
        }
        let lib = lock(library)?;
        match lib.adopt_file(&tmp, &name) {
            Ok(t) => tracks.push(t),
            Err(_) => errors += 1,
        }
    }
    if tracks.is_empty() {
        return Err(net(if errors > 0 {
            "El ZIP tiene audios, pero ninguno se pudo leer"
        } else {
            "El ZIP no contiene canciones"
        }));
    }
    Ok(tracks)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;
    use std::thread;

    #[test]
    fn names_from_headers_and_urls() {
        assert_eq!(
            filename_from_disposition(r#"attachment; filename="Mi canción.mp3""#).as_deref(),
            Some("Mi canción.mp3")
        );
        assert_eq!(
            filename_from_disposition("attachment; filename*=UTF-8''Caf%C3%A9%20del%20mar.flac")
                .as_deref(),
            Some("Café del mar.flac")
        );
        assert_eq!(
            filename_from_url("https://archive.org/download/x/01%20Tema.mp3?t=1").as_deref(),
            Some("01 Tema.mp3")
        );
        assert_eq!(filename_from_url("https://ejemplo.com/"), None);

        // Sin extensión: se deduce del Content-Type
        assert_eq!(
            pick_name(None, Some("audio/mpeg"), "https://x.com/stream/123").unwrap(),
            "123.mp3"
        );
        // Una página web no es audio
        let err = pick_name(None, Some("text/html; charset=utf-8"), "https://x.com/v").unwrap_err();
        assert!(err.to_string().contains("página web"));
        assert!(validate_url("ftp://x").is_err());
        assert!(validate_url("  https://x.com/a.mp3 ").is_ok());
    }

    fn wav(samples: u32) -> Vec<u8> {
        let data_len = samples * 2;
        let mut v = Vec::new();
        v.extend_from_slice(b"RIFF");
        v.extend_from_slice(&(36 + data_len).to_le_bytes());
        v.extend_from_slice(b"WAVEfmt ");
        v.extend_from_slice(&16u32.to_le_bytes());
        v.extend_from_slice(&1u16.to_le_bytes());
        v.extend_from_slice(&1u16.to_le_bytes());
        v.extend_from_slice(&8000u32.to_le_bytes());
        v.extend_from_slice(&16000u32.to_le_bytes());
        v.extend_from_slice(&2u16.to_le_bytes());
        v.extend_from_slice(&16u16.to_le_bytes());
        v.extend_from_slice(b"data");
        v.extend_from_slice(&data_len.to_le_bytes());
        v.resize(v.len() + data_len as usize, 0);
        v
    }

    fn album_zip() -> Vec<u8> {
        let mut buf = std::io::Cursor::new(Vec::new());
        {
            let mut z = zip::ZipWriter::new(&mut buf);
            let opts = zip::write::SimpleFileOptions::default();
            z.start_file("Album/01 Intro.wav", opts).unwrap();
            z.write_all(&wav(800)).unwrap();
            z.start_file("Album/02 Final.wav", opts).unwrap();
            z.write_all(&wav(1600)).unwrap();
            z.start_file("Album/cover.jpg", opts).unwrap();
            z.write_all(b"jpg").unwrap();
            z.start_file("__MACOSX/Album/._01 Intro.wav", opts).unwrap();
            z.write_all(b"basura").unwrap();
            z.finish().unwrap();
        }
        buf.into_inner()
    }

    /// Servidor HTTP local que responde según la ruta.
    fn serve() -> String {
        let server = Arc::new(tiny_http::Server::http("127.0.0.1:0").unwrap());
        let port = server.server_addr().to_ip().unwrap().port();
        thread::spawn(move || {
            for req in server.incoming_requests() {
                let h = |k: &str, v: &str| tiny_http::Header::from_bytes(k, v).unwrap();
                let res = match req.url() {
                    "/musica/Tema%20uno.wav" => tiny_http::Response::from_data(wav(1000)),
                    "/descarga?id=7" => tiny_http::Response::from_data(wav(2000)).with_header(h(
                        "Content-Disposition",
                        "attachment; filename*=UTF-8''Canci%C3%B3n%20secreta.wav",
                    )),
                    "/album" => tiny_http::Response::from_data(album_zip())
                        .with_header(h("Content-Type", "application/zip")),
                    "/pagina" => tiny_http::Response::from_data(b"<html></html>".to_vec())
                        .with_header(h("Content-Type", "text/html")),
                    "/roto.mp3" => tiny_http::Response::from_data(b"no soy audio".to_vec()),
                    _ => tiny_http::Response::from_data(Vec::new()).with_status_code(404),
                };
                let _ = req.respond(res);
            }
        });
        format!("http://127.0.0.1:{port}")
    }

    #[test]
    fn imports_from_links() {
        let base = serve();
        let dir = tempfile::tempdir().unwrap();
        let lib = Mutex::new(Library::open(dir.path()).unwrap());
        let cancel = AtomicBool::new(false);
        let no_progress = |_: &LinkProgress| {};

        let t = import_url(
            &format!("{base}/musica/Tema%20uno.wav"),
            &lib,
            &cancel,
            no_progress,
        )
        .unwrap();
        assert_eq!(t[0].title, "Tema uno");

        let mut last = LinkProgress::default();
        let t = import_url(&format!("{base}/descarga?id=7"), &lib, &cancel, |p| {
            last = p.clone()
        })
        .unwrap();
        assert_eq!(t[0].title, "Canción secreta");
        assert_eq!(last.bytes_total, Some(last.bytes_done));

        let album = import_url(&format!("{base}/album"), &lib, &cancel, no_progress).unwrap();
        let mut titles: Vec<_> = album.iter().map(|t| t.title.clone()).collect();
        titles.sort();
        assert_eq!(titles, ["01 Intro", "02 Final"]);

        let err = import_url(&format!("{base}/pagina"), &lib, &cancel, no_progress).unwrap_err();
        assert!(err.to_string().contains("página web"), "{err}");
        let err = import_url(&format!("{base}/roto.mp3"), &lib, &cancel, no_progress).unwrap_err();
        assert!(err.to_string().contains("no es un audio válido"), "{err}");
        let err = import_url(&format!("{base}/nada.mp3"), &lib, &cancel, no_progress).unwrap_err();
        assert!(err.to_string().contains("404"), "{err}");

        let lib = lib.into_inner().unwrap();
        assert_eq!(lib.tracks().unwrap().len(), 4);
        // Sin restos de archivos temporales
        assert!(fs::read_dir(lib.music_dir()).unwrap().all(|e| !e
            .unwrap()
            .file_name()
            .to_string_lossy()
            .ends_with(".part")));
    }
}
