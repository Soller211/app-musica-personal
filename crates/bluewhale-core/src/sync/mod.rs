//! Sincronización por Wi‑Fi: la PC comparte su biblioteca en la red local y
//! el teléfono elige qué descargar.
//!
//! Protocolo (HTTP simple, solo red local):
//! - Descubrimiento: el teléfono envía [`DISCOVERY_MSG`] por UDP broadcast al
//!   puerto [`DISCOVERY_PORT`]; la PC responde con un [`ServerInfo`] en JSON.
//! - `GET /info`        → [`ServerInfo`] (sin código)
//! - `GET /library`     → [`RemoteLibrary`] (requiere código)
//! - `GET /track/{id}`  → el archivo de audio (requiere código)
//!
//! El código de 6 dígitos viaja en la cabecera [`CODE_HEADER`]. Tras
//! [`MAX_FAILED_ATTEMPTS`] códigos incorrectos el servidor se bloquea hasta
//! que se vuelva a activar "Compartir" en la PC.

mod client;
mod server;

use std::collections::HashSet;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Mutex;
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};

use crate::{Error, Library, Result};

pub use client::{discover, download_track, fetch_library, normalize_address};
pub use server::{local_ips, ShareServer};

pub const DISCOVERY_PORT: u16 = 47809;
pub const HTTP_PORT: u16 = 47810;
pub const DISCOVERY_MSG: &[u8] = b"BLUEWHALE?1";
pub const CODE_HEADER: &str = "X-BlueWhale-Code";
pub const MAX_FAILED_ATTEMPTS: u32 = 20;

/// Identidad de una canción independiente del dispositivo: mismo archivo en
/// la PC y en el teléfono ⇒ misma clave.
pub fn track_key(title: &str, artist: &str, album: &str, size: u64) -> String {
    let norm = |s: &str| s.trim().to_lowercase();
    format!(
        "{}\u{1f}{}\u{1f}{}\u{1f}{size}",
        norm(artist),
        norm(album),
        norm(title)
    )
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct ServerInfo {
    pub app: String,
    pub name: String,
    pub version: String,
    pub port: u16,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct Peer {
    pub name: String,
    /// `ip:puerto`
    pub address: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RemoteTrack {
    pub id: i64,
    pub key: String,
    pub title: String,
    pub artist: String,
    pub album: String,
    pub album_artist: String,
    pub track_no: Option<u32>,
    pub disc_no: Option<u32>,
    pub year: Option<u32>,
    pub duration: f64,
    pub size: u64,
    pub file_name: String,
    /// Lo rellena el teléfono: `true` si ya tiene esta canción.
    #[serde(default)]
    pub on_device: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RemotePlaylist {
    pub id: i64,
    pub name: String,
    pub track_ids: Vec<i64>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RemoteLibrary {
    pub name: String,
    pub tracks: Vec<RemoteTrack>,
    pub playlists: Vec<RemotePlaylist>,
}

impl RemoteLibrary {
    /// Marca qué canciones ya existen en la biblioteca local.
    pub fn mark_on_device(&mut self, local: &crate::Library) -> crate::Result<()> {
        let keys = local.local_keys()?;
        for t in &mut self.tracks {
            t.on_device = keys.contains_key(&t.key);
        }
        Ok(())
    }
}

#[derive(Debug, Clone, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SyncProgress {
    pub done: usize,
    pub total: usize,
    pub current: String,
    pub bytes_done: u64,
    pub bytes_total: u64,
    pub failed: usize,
}

#[derive(Debug, Clone, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SyncReport {
    pub downloaded: usize,
    /// Canciones elegidas que ya estaban en el dispositivo.
    pub skipped: usize,
    pub failed: Vec<String>,
    pub playlists: usize,
    pub cancelled: bool,
}

/// Descarga las canciones elegidas (más las de las playlists elegidas) que
/// aún no estén en el dispositivo y recrea esas playlists localmente.
#[allow(clippy::too_many_arguments)]
pub fn download_selection(
    address: &str,
    code: &str,
    remote: &RemoteLibrary,
    track_ids: &[i64],
    playlist_ids: &[i64],
    library: &Mutex<Library>,
    cancel: &AtomicBool,
    mut progress: impl FnMut(&SyncProgress),
) -> Result<SyncReport> {
    let lock_err = |e: std::sync::PoisonError<_>| Error::Network(format!("{e}"));
    let playlists: Vec<&RemotePlaylist> = remote
        .playlists
        .iter()
        .filter(|p| playlist_ids.contains(&p.id))
        .collect();

    let mut seen = HashSet::new();
    let wanted: Vec<&RemoteTrack> = track_ids
        .iter()
        .chain(playlists.iter().flat_map(|p| p.track_ids.iter()))
        .filter(|id| seen.insert(**id))
        .filter_map(|id| remote.tracks.iter().find(|t| t.id == *id))
        .collect();

    let local = library.lock().map_err(lock_err)?.local_keys()?;
    let (present, missing): (Vec<&RemoteTrack>, Vec<&RemoteTrack>) =
        wanted.into_iter().partition(|t| local.contains_key(&t.key));

    let mut report = SyncReport {
        skipped: present.len(),
        ..Default::default()
    };
    let mut state = SyncProgress {
        total: missing.len(),
        bytes_total: missing.iter().map(|t| t.size).sum(),
        ..Default::default()
    };
    let mut last_emit = Instant::now() - Duration::from_secs(1);
    let mut consecutive_failures = 0;
    progress(&state);

    for t in missing {
        if cancel.load(Ordering::Relaxed) {
            report.cancelled = true;
            break;
        }
        state.current = format!("{} · {}", t.title, t.artist);
        progress(&state);
        let before = state.bytes_done;
        let result = client::download_track(address, code, t, library, cancel, |n| {
            state.bytes_done += n;
            if last_emit.elapsed() > Duration::from_millis(150) {
                last_emit = Instant::now();
                progress(&state);
            }
        });
        match result {
            Ok(_) => {
                report.downloaded += 1;
                consecutive_failures = 0;
            }
            Err(e) => {
                if cancel.load(Ordering::Relaxed) {
                    report.cancelled = true;
                    break;
                }
                state.failed += 1;
                state.bytes_done = before + t.size;
                report.failed.push(format!("{}: {e}", t.title));
                consecutive_failures += 1;
                if consecutive_failures >= 3 {
                    report.failed.push("Se perdió la conexión con la PC".into());
                    break;
                }
            }
        }
        state.done += 1;
        progress(&state);
    }

    if !report.cancelled && !playlists.is_empty() {
        let lib = library.lock().map_err(lock_err)?;
        let local = lib.local_keys()?;
        for p in playlists {
            let ids: Vec<i64> = p
                .track_ids
                .iter()
                .filter_map(|id| remote.tracks.iter().find(|t| t.id == *id))
                .filter_map(|t| local.get(&t.key).copied())
                .collect();
            lib.upsert_playlist_by_name(&p.name, &ids)?;
            report.playlists += 1;
        }
    }
    Ok(report)
}

#[cfg(test)]
mod tests;
