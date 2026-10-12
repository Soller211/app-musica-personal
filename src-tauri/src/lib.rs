use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::Duration;

use bluewhale_core::sync::{self, Peer, RemoteLibrary, ShareServer};
use bluewhale_core::{Folder, Library, PendingUpload, Playlist, ScanReport, Track};
use serde::Serialize;
use tauri::{AppHandle, Emitter, Manager, State};

/// Conexión del teléfono con una PC (tras introducir el código).
struct SyncSession {
    address: String,
    code: String,
    remote: RemoteLibrary,
}

struct AppState {
    library: Arc<Mutex<Library>>,
    share: Mutex<Option<ShareServer>>,
    session: Mutex<Option<SyncSession>>,
    syncing: Arc<AtomicBool>,
    cancel: Arc<AtomicBool>,
    link_busy: Arc<AtomicBool>,
    link_cancel: Arc<AtomicBool>,
    uploads: Mutex<HashMap<u64, PendingUpload>>,
    next_upload: AtomicU64,
}

impl AppState {
    fn lib(&self) -> Result<MutexGuard<'_, Library>, String> {
        self.library.lock().map_err(|e| e.to_string())
    }
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct ShareInfo {
    name: String,
    code: String,
    port: u16,
    addresses: Vec<String>,
    sent: u64,
    locked: bool,
}

impl From<&ShareServer> for ShareInfo {
    fn from(s: &ShareServer) -> Self {
        ShareInfo {
            name: s.name().to_string(),
            code: s.code().to_string(),
            port: s.port(),
            addresses: sync::local_ips(),
            sent: s.sent(),
            locked: s.locked(),
        }
    }
}

type CmdResult<T> = Result<T, String>;

fn err(e: impl ToString) -> String {
    e.to_string()
}

/// Permite al protocolo `asset:` servir archivos de una carpeta externa.
fn allow_dir(app: &AppHandle, path: &std::path::Path) {
    if let Err(e) = app.asset_protocol_scope().allow_directory(path, true) {
        eprintln!("no se pudo permitir {}: {e}", path.display());
    }
}

// --------------------------------------------------------------------- comandos

#[tauri::command]
fn is_mobile() -> bool {
    cfg!(mobile)
}

#[tauri::command]
async fn get_tracks(state: State<'_, AppState>) -> CmdResult<Vec<Track>> {
    state.lib()?.tracks().map_err(err)
}

/// Importar un archivo llega en tres pasos: `upload_begin`, varios
/// `upload_chunk` (trozos en base64) y `upload_finish`. Así funciona igual en
/// Android, donde el puente con la interfaz no admite binarios grandes.
#[tauri::command]
async fn upload_begin(state: State<'_, AppState>) -> CmdResult<u64> {
    let upload = state.lib()?.begin_upload().map_err(err)?;
    let id = state.next_upload.fetch_add(1, Ordering::SeqCst);
    state.uploads.lock().map_err(err)?.insert(id, upload);
    Ok(id)
}

#[tauri::command]
async fn upload_chunk(id: u64, data: String, state: State<'_, AppState>) -> CmdResult<()> {
    use base64::Engine;
    let bytes = base64::engine::general_purpose::STANDARD
        .decode(data.as_bytes())
        .map_err(|e| format!("trozo dañado: {e}"))?;
    let mut uploads = state.uploads.lock().map_err(err)?;
    let upload = uploads.get_mut(&id).ok_or("La importación ya no existe")?;
    if let Err(e) = upload.append(&bytes) {
        if let Some(u) = uploads.remove(&id) {
            u.abort();
        }
        return Err(err(e));
    }
    Ok(())
}

#[tauri::command]
async fn upload_finish(id: u64, name: String, state: State<'_, AppState>) -> CmdResult<Track> {
    let upload = state
        .uploads
        .lock()
        .map_err(err)?
        .remove(&id)
        .ok_or("La importación ya no existe")?;
    state.lib()?.finish_upload(upload, &name).map_err(err)
}

#[tauri::command]
async fn upload_abort(id: u64, state: State<'_, AppState>) -> CmdResult<()> {
    if let Some(u) = state.uploads.lock().map_err(err)?.remove(&id) {
        u.abort();
    }
    Ok(())
}

#[tauri::command]
async fn delete_track(id: i64, state: State<'_, AppState>) -> CmdResult<()> {
    state.lib()?.delete_track(id).map_err(err)
}

#[tauri::command]
async fn set_favorite(id: i64, favorite: bool, state: State<'_, AppState>) -> CmdResult<()> {
    state.lib()?.set_favorite(id, favorite).map_err(err)
}

#[tauri::command]
async fn record_play(id: i64, state: State<'_, AppState>) -> CmdResult<()> {
    state.lib()?.record_play(id).map_err(err)
}

#[tauri::command]
async fn get_folders(state: State<'_, AppState>) -> CmdResult<Vec<Folder>> {
    state.lib()?.folders().map_err(err)
}

/// Abre el selector de carpetas (solo escritorio) y escanea la elegida.
#[tauri::command]
async fn pick_folder(app: AppHandle, state: State<'_, AppState>) -> CmdResult<Option<ScanReport>> {
    #[cfg(desktop)]
    {
        use tauri_plugin_dialog::DialogExt;
        let Some(picked) = app.dialog().file().blocking_pick_folder() else {
            return Ok(None);
        };
        let path: PathBuf = picked.into_path().map_err(err)?;
        allow_dir(&app, &path);
        state.lib()?.add_folder(&path).map(Some).map_err(err)
    }
    #[cfg(mobile)]
    {
        let _ = (app, state);
        Err("En el teléfono usa «Importar canciones»".into())
    }
}

#[tauri::command]
async fn add_folder(
    path: String,
    app: AppHandle,
    state: State<'_, AppState>,
) -> CmdResult<ScanReport> {
    let path = PathBuf::from(path);
    allow_dir(&app, &path);
    state.lib()?.add_folder(&path).map_err(err)
}

#[tauri::command]
async fn remove_folder(id: i64, state: State<'_, AppState>) -> CmdResult<()> {
    state.lib()?.remove_folder(id).map_err(err)
}

#[tauri::command]
async fn rescan(state: State<'_, AppState>) -> CmdResult<ScanReport> {
    state.lib()?.rescan_all().map_err(err)
}

#[tauri::command]
async fn get_playlists(state: State<'_, AppState>) -> CmdResult<Vec<Playlist>> {
    state.lib()?.playlists().map_err(err)
}

#[tauri::command]
async fn create_playlist(name: String, state: State<'_, AppState>) -> CmdResult<Playlist> {
    state.lib()?.create_playlist(&name).map_err(err)
}

#[tauri::command]
async fn rename_playlist(id: i64, name: String, state: State<'_, AppState>) -> CmdResult<()> {
    state.lib()?.rename_playlist(id, &name).map_err(err)
}

#[tauri::command]
async fn delete_playlist(id: i64, state: State<'_, AppState>) -> CmdResult<()> {
    state.lib()?.delete_playlist(id).map_err(err)
}

#[tauri::command]
async fn add_to_playlist(
    id: i64,
    track_ids: Vec<i64>,
    state: State<'_, AppState>,
) -> CmdResult<()> {
    state.lib()?.add_to_playlist(id, &track_ids).map_err(err)
}

#[tauri::command]
async fn set_playlist_tracks(
    id: i64,
    track_ids: Vec<i64>,
    state: State<'_, AppState>,
) -> CmdResult<()> {
    state
        .lib()?
        .set_playlist_tracks(id, &track_ids)
        .map_err(err)
}

// ------------------------------------------------------ importar desde enlace

/// Descarga un enlace directo (audio o ZIP) y lo añade a la biblioteca.
/// El progreso llega como eventos `link-progress`.
#[tauri::command]
async fn import_url(
    url: String,
    app: AppHandle,
    state: State<'_, AppState>,
) -> CmdResult<Vec<Track>> {
    if state.link_busy.swap(true, Ordering::SeqCst) {
        return Err("Ya hay una descarga en curso".into());
    }
    state.link_cancel.store(false, Ordering::SeqCst);
    let (library, cancel, busy) = (
        state.library.clone(),
        state.link_cancel.clone(),
        state.link_busy.clone(),
    );
    let result = tauri::async_runtime::spawn_blocking(move || {
        let r = bluewhale_core::link::import_url(&url, &library, &cancel, |p| {
            let _ = app.emit("link-progress", p);
        });
        busy.store(false, Ordering::SeqCst);
        r
    })
    .await
    .map_err(err)?;
    result.map_err(err)
}

#[tauri::command]
fn import_url_cancel(state: State<'_, AppState>) {
    state.link_cancel.store(true, Ordering::SeqCst);
}

// ------------------------------------------------------- Wi‑Fi: lado de la PC

#[tauri::command]
async fn share_start(state: State<'_, AppState>) -> CmdResult<ShareInfo> {
    let mut share = state.share.lock().map_err(err)?;
    // Reiniciar genera un código nuevo (y desbloquea tras intentos fallidos).
    *share = None;
    let server = ShareServer::start(state.library.clone()).map_err(err)?;
    let info = ShareInfo::from(&server);
    *share = Some(server);
    Ok(info)
}

#[tauri::command]
async fn share_stop(state: State<'_, AppState>) -> CmdResult<()> {
    *state.share.lock().map_err(err)? = None;
    Ok(())
}

#[tauri::command]
async fn share_status(state: State<'_, AppState>) -> CmdResult<Option<ShareInfo>> {
    Ok(state
        .share
        .lock()
        .map_err(err)?
        .as_ref()
        .map(ShareInfo::from))
}

// --------------------------------------------------- Wi‑Fi: lado del teléfono

#[tauri::command]
async fn sync_discover() -> CmdResult<Vec<Peer>> {
    tauri::async_runtime::spawn_blocking(|| sync::discover(Duration::from_millis(2500)))
        .await
        .map_err(err)
}

#[tauri::command]
async fn sync_connect(
    address: String,
    code: String,
    state: State<'_, AppState>,
) -> CmdResult<RemoteLibrary> {
    let (addr, c) = (address.clone(), code.clone());
    let mut remote = tauri::async_runtime::spawn_blocking(move || sync::fetch_library(&addr, &c))
        .await
        .map_err(err)?
        .map_err(err)?;
    remote.mark_on_device(&*state.lib()?).map_err(err)?;
    *state.session.lock().map_err(err)? = Some(SyncSession {
        address: sync::normalize_address(&address),
        code,
        remote: remote.clone(),
    });
    Ok(remote)
}

/// Empieza a descargar en segundo plano. El progreso llega como eventos
/// `sync-progress` y el resultado final como `sync-finished` (o `sync-error`).
#[tauri::command]
async fn sync_start(
    track_ids: Vec<i64>,
    playlist_ids: Vec<i64>,
    app: AppHandle,
    state: State<'_, AppState>,
) -> CmdResult<()> {
    let (address, code, remote) = {
        let session = state.session.lock().map_err(err)?;
        let s = session.as_ref().ok_or("Primero conecta con tu PC")?;
        (s.address.clone(), s.code.clone(), s.remote.clone())
    };
    if state.syncing.swap(true, Ordering::SeqCst) {
        return Err("Ya hay una descarga en curso".into());
    }
    state.cancel.store(false, Ordering::SeqCst);
    let (library, syncing, cancel) = (
        state.library.clone(),
        state.syncing.clone(),
        state.cancel.clone(),
    );

    std::thread::spawn(move || {
        let result = sync::download_selection(
            &address,
            &code,
            &remote,
            &track_ids,
            &playlist_ids,
            &library,
            &cancel,
            |p| {
                let _ = app.emit("sync-progress", p);
            },
        );
        syncing.store(false, Ordering::SeqCst);
        let _ = match result {
            Ok(report) => app.emit("sync-finished", report),
            Err(e) => app.emit("sync-error", e.to_string()),
        };
    });
    Ok(())
}

#[tauri::command]
fn sync_cancel(state: State<'_, AppState>) {
    state.cancel.store(true, Ordering::SeqCst);
}

// ------------------------------------------------------------------------ app

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_dialog::init())
        .setup(|app| {
            let data_dir = app.path().app_data_dir()?;
            let library = Library::open(&data_dir)?;
            for folder in library.folders()? {
                allow_dir(app.handle(), std::path::Path::new(&folder.path));
            }
            app.manage(AppState {
                library: Arc::new(Mutex::new(library)),
                share: Mutex::new(None),
                session: Mutex::new(None),
                syncing: Arc::new(AtomicBool::new(false)),
                cancel: Arc::new(AtomicBool::new(false)),
                link_busy: Arc::new(AtomicBool::new(false)),
                link_cancel: Arc::new(AtomicBool::new(false)),
                uploads: Mutex::new(HashMap::new()),
                next_upload: AtomicU64::new(1),
            });
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            is_mobile,
            get_tracks,
            upload_begin,
            upload_chunk,
            upload_finish,
            upload_abort,
            delete_track,
            set_favorite,
            record_play,
            get_folders,
            pick_folder,
            add_folder,
            remove_folder,
            rescan,
            get_playlists,
            create_playlist,
            rename_playlist,
            delete_playlist,
            add_to_playlist,
            set_playlist_tracks,
            import_url,
            import_url_cancel,
            share_start,
            share_stop,
            share_status,
            sync_discover,
            sync_connect,
            sync_start,
            sync_cancel,
        ])
        .run(tauri::generate_context!())
        .expect("error al iniciar BlueWhale");
}
