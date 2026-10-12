use std::path::PathBuf;
use std::sync::{Mutex, MutexGuard};

use bluewhale_core::{Folder, Library, Playlist, ScanReport, Track};
use tauri::ipc::{InvokeBody, Request};
use tauri::{AppHandle, Manager, State};

struct AppState {
    library: Mutex<Library>,
}

impl AppState {
    fn lib(&self) -> Result<MutexGuard<'_, Library>, String> {
        self.library.lock().map_err(|e| e.to_string())
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

/// Recibe el archivo como bytes crudos (sin pasar por JSON) y lo copia
/// dentro de la app. El nombre llega en la cabecera `x-file-name`.
#[tauri::command]
async fn import_file(request: Request<'_>, state: State<'_, AppState>) -> CmdResult<Track> {
    let InvokeBody::Raw(bytes) = request.body() else {
        return Err("se esperaban bytes del archivo".into());
    };
    let name = request
        .headers()
        .get("x-file-name")
        .and_then(|v| v.to_str().ok())
        .map(|v| {
            percent_encoding::percent_decode_str(v)
                .decode_utf8_lossy()
                .into_owned()
        })
        .unwrap_or_else(|| "cancion.mp3".into());
    state.lib()?.import_bytes(&name, bytes).map_err(err)
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
                library: Mutex::new(library),
            });
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            is_mobile,
            get_tracks,
            import_file,
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
        ])
        .run(tauri::generate_context!())
        .expect("error al iniciar BlueWhale");
}
