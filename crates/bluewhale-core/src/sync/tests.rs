use std::sync::atomic::AtomicBool;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use super::*;
use crate::Library;

/// WAV PCM mínimo y válido (silencio).
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

#[test]
fn normalizes_addresses() {
    assert_eq!(normalize_address("192.168.1.5"), "192.168.1.5:47810");
    assert_eq!(
        normalize_address(" http://10.0.0.2:5000/ "),
        "10.0.0.2:5000"
    );
}

#[test]
fn pc_to_phone_sync() {
    let pc_dir = tempfile::tempdir().unwrap();
    let phone_dir = tempfile::tempdir().unwrap();

    // "PC" con 3 canciones y una playlist
    let pc = Library::open(pc_dir.path()).unwrap();
    let a = pc.import_bytes("Uno.wav", &wav(1000)).unwrap();
    let b = pc.import_bytes("Dos.wav", &wav(2000)).unwrap();
    let c = pc.import_bytes("Tres.wav", &wav(3000)).unwrap();
    let pl = pc.create_playlist("Viaje").unwrap();
    pc.set_playlist_tracks(pl.id, &[c.id, a.id]).unwrap();
    let pc = Arc::new(Mutex::new(pc));
    let server = ShareServer::start(pc.clone()).unwrap();
    let addr = format!("127.0.0.1:{}", server.port());
    assert_eq!(server.code().len(), 6);

    // Descubrimiento (por localhost: en el entorno de tests no hay broadcast)
    // (Los tests corren en paralelo: puede responder el servidor de otro test.)
    let peers = client::discover_at(&["127.0.0.1".into()], Duration::from_millis(800));
    assert!(!peers.is_empty(), "nadie respondió al descubrimiento");
    assert!(peers
        .iter()
        .all(|p| p.address.starts_with("127.0.0.1:") && !p.name.is_empty()));

    // Código incorrecto
    let err = fetch_library(&addr, "000000x").unwrap_err();
    assert!(err.to_string().contains("Código incorrecto"), "{err}");

    // "Teléfono" que ya tiene una de las canciones
    let phone = Library::open(phone_dir.path()).unwrap();
    phone.import_bytes("Uno.wav", &wav(1000)).unwrap();
    let phone = Mutex::new(phone);

    let mut remote = fetch_library(&addr, server.code()).unwrap();
    assert_eq!(remote.tracks.len(), 3);
    remote.mark_on_device(&phone.lock().unwrap()).unwrap();
    assert_eq!(remote.tracks.iter().filter(|t| t.on_device).count(), 1);

    // Elegimos la canción "Dos" y la playlist "Viaje" (Tres + Uno)
    let cancel = AtomicBool::new(false);
    let mut last = SyncProgress::default();
    let report = download_selection(
        &addr,
        server.code(),
        &remote,
        &[b.id],
        &[pl.id],
        &phone,
        &cancel,
        |p| last = p.clone(),
    )
    .unwrap();
    assert_eq!(report.downloaded, 2, "{report:?}");
    assert_eq!(report.skipped, 1);
    assert!(report.failed.is_empty());
    assert_eq!(last.done, 2);
    assert_eq!(last.bytes_done, last.bytes_total);
    assert_eq!(server.sent(), 2);

    let phone = phone.into_inner().unwrap();
    let tracks = phone.tracks().unwrap();
    assert_eq!(tracks.len(), 3);
    let playlists = phone.playlists().unwrap();
    assert_eq!(playlists.len(), 1);
    assert_eq!(playlists[0].name, "Viaje");
    let names: Vec<String> = playlists[0]
        .track_ids
        .iter()
        .map(|id| phone.track(*id).unwrap().title)
        .collect();
    assert_eq!(names, ["Tres", "Uno"]);
    // No quedan archivos temporales
    assert!(std::fs::read_dir(phone.music_dir()).unwrap().all(|e| !e
        .unwrap()
        .file_name()
        .to_string_lossy()
        .ends_with(".part")));

    // Repetir no descarga nada nuevo
    let phone = Mutex::new(phone);
    let again = download_selection(
        &addr,
        server.code(),
        &remote,
        &[a.id, b.id, c.id],
        &[],
        &phone,
        &cancel,
        |_| {},
    )
    .unwrap();
    assert_eq!((again.downloaded, again.skipped), (0, 3));
}

#[test]
fn locks_after_many_wrong_codes() {
    let dir = tempfile::tempdir().unwrap();
    let lib = Arc::new(Mutex::new(Library::open(dir.path()).unwrap()));
    let server = ShareServer::start(lib).unwrap();
    let addr = format!("127.0.0.1:{}", server.port());
    for _ in 0..MAX_FAILED_ATTEMPTS {
        let _ = fetch_library(&addr, "nope");
    }
    assert!(server.locked());
    let err = fetch_library(&addr, server.code()).unwrap_err();
    assert!(err.to_string().contains("Demasiados intentos"), "{err}");
}
