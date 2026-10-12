use std::collections::{HashMap, HashSet};
use std::fs;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use rusqlite::{params, Connection, OptionalExtension, Row};
use walkdir::WalkDir;

use crate::metadata::{is_audio_file, read_meta};
use crate::model::{Folder, Playlist, ScanReport, Track, TrackMeta};
use crate::{Error, Result};

const SCHEMA: &str = r#"
CREATE TABLE IF NOT EXISTS tracks (
    id           INTEGER PRIMARY KEY,
    path         TEXT NOT NULL UNIQUE,
    title        TEXT NOT NULL,
    artist       TEXT NOT NULL,
    album        TEXT NOT NULL,
    album_artist TEXT NOT NULL,
    track_no     INTEGER,
    disc_no      INTEGER,
    year         INTEGER,
    genre        TEXT,
    duration     REAL NOT NULL DEFAULT 0,
    cover        TEXT,
    favorite     INTEGER NOT NULL DEFAULT 0,
    play_count   INTEGER NOT NULL DEFAULT 0,
    last_played  INTEGER,
    added_at     INTEGER NOT NULL,
    imported     INTEGER NOT NULL DEFAULT 0,
    mtime        INTEGER NOT NULL DEFAULT 0
);
CREATE TABLE IF NOT EXISTS folders (
    id   INTEGER PRIMARY KEY,
    path TEXT NOT NULL UNIQUE
);
CREATE TABLE IF NOT EXISTS playlists (
    id         INTEGER PRIMARY KEY,
    name       TEXT NOT NULL,
    created_at INTEGER NOT NULL
);
CREATE TABLE IF NOT EXISTS playlist_tracks (
    playlist_id INTEGER NOT NULL REFERENCES playlists(id) ON DELETE CASCADE,
    track_id    INTEGER NOT NULL REFERENCES tracks(id) ON DELETE CASCADE,
    position    INTEGER NOT NULL,
    PRIMARY KEY (playlist_id, position)
);
CREATE INDEX IF NOT EXISTS idx_playlist_tracks_track ON playlist_tracks(track_id);
"#;

const TRACK_COLUMNS: &str = "id, path, title, artist, album, album_artist, track_no, disc_no, \
     year, genre, duration, cover, favorite, play_count, last_played, added_at, imported";

/// Biblioteca musical local: base de datos + carpetas de datos de la app.
pub struct Library {
    conn: Connection,
    music_dir: PathBuf,
    covers_dir: PathBuf,
}

fn now() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

fn mtime_of(path: &Path) -> i64 {
    fs::metadata(path)
        .and_then(|m| m.modified())
        .ok()
        .and_then(|t| t.duration_since(UNIX_EPOCH).ok())
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

fn row_to_track(row: &Row<'_>) -> rusqlite::Result<Track> {
    Ok(Track {
        id: row.get(0)?,
        path: row.get(1)?,
        title: row.get(2)?,
        artist: row.get(3)?,
        album: row.get(4)?,
        album_artist: row.get(5)?,
        track_no: row.get(6)?,
        disc_no: row.get(7)?,
        year: row.get(8)?,
        genre: row.get(9)?,
        duration: row.get(10)?,
        cover: row.get(11)?,
        favorite: row.get(12)?,
        play_count: row.get(13)?,
        last_played: row.get(14)?,
        added_at: row.get(15)?,
        imported: row.get(16)?,
    })
}

/// Deja solo caracteres seguros para un nombre de archivo.
fn sanitize_file_name(name: &str) -> String {
    let base = Path::new(name)
        .file_name()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_default();
    let cleaned: String = base
        .chars()
        .map(|c| match c {
            '/' | '\\' | ':' | '*' | '?' | '"' | '<' | '>' | '|' => '_',
            c if c.is_control() => '_',
            c => c,
        })
        .collect();
    let cleaned = cleaned.trim_matches(|c: char| c == '.' || c.is_whitespace());
    if cleaned.is_empty() {
        "cancion".into()
    } else {
        cleaned.to_string()
    }
}

impl Library {
    /// Abre (o crea) la biblioteca dentro de `data_dir`.
    ///
    /// Estructura:
    /// - `data_dir/library.db`  base de datos
    /// - `data_dir/music/`      canciones importadas (copiadas a la app)
    /// - `data_dir/covers/`     carátulas extraídas
    pub fn open(data_dir: &Path) -> Result<Self> {
        fs::create_dir_all(data_dir)?;
        let conn = Connection::open(data_dir.join("library.db"))?;
        Self::with_connection(conn, data_dir)
    }

    fn with_connection(conn: Connection, data_dir: &Path) -> Result<Self> {
        conn.execute_batch("PRAGMA foreign_keys = ON; PRAGMA journal_mode = WAL;")?;
        conn.execute_batch(SCHEMA)?;
        let music_dir = data_dir.join("music");
        let covers_dir = data_dir.join("covers");
        fs::create_dir_all(&music_dir)?;
        fs::create_dir_all(&covers_dir)?;
        Ok(Self {
            conn,
            music_dir,
            covers_dir,
        })
    }

    pub fn music_dir(&self) -> &Path {
        &self.music_dir
    }

    // ---------------------------------------------------------------- canciones

    pub fn tracks(&self) -> Result<Vec<Track>> {
        let mut stmt = self.conn.prepare(&format!(
            "SELECT {TRACK_COLUMNS} FROM tracks
             ORDER BY album_artist COLLATE NOCASE, album COLLATE NOCASE,
                      disc_no, track_no, title COLLATE NOCASE"
        ))?;
        let rows = stmt.query_map([], row_to_track)?;
        Ok(rows.collect::<rusqlite::Result<_>>()?)
    }

    pub fn track(&self, id: i64) -> Result<Track> {
        self.conn
            .query_row(
                &format!("SELECT {TRACK_COLUMNS} FROM tracks WHERE id = ?1"),
                [id],
                row_to_track,
            )
            .optional()?
            .ok_or_else(|| Error::NotFound(format!("canción {id}")))
    }

    fn track_by_path(&self, path: &str) -> Result<Option<Track>> {
        Ok(self
            .conn
            .query_row(
                &format!("SELECT {TRACK_COLUMNS} FROM tracks WHERE path = ?1"),
                [path],
                row_to_track,
            )
            .optional()?)
    }

    /// Inserta o actualiza la canción en `path`. Devuelve `(track, era_nueva)`.
    fn upsert_file(&self, path: &Path, imported: bool) -> Result<(Track, bool)> {
        let meta: TrackMeta = read_meta(path, &self.covers_dir)?;
        let path_str = path.to_string_lossy().into_owned();
        let existed = self.track_by_path(&path_str)?.is_some();
        self.conn.execute(
            "INSERT INTO tracks (path, title, artist, album, album_artist, track_no, disc_no,
                                 year, genre, duration, cover, added_at, imported, mtime)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14)
             ON CONFLICT(path) DO UPDATE SET
                title = excluded.title, artist = excluded.artist, album = excluded.album,
                album_artist = excluded.album_artist, track_no = excluded.track_no,
                disc_no = excluded.disc_no, year = excluded.year, genre = excluded.genre,
                duration = excluded.duration, cover = excluded.cover, mtime = excluded.mtime",
            params![
                path_str,
                meta.title,
                meta.artist,
                meta.album,
                meta.album_artist,
                meta.track_no,
                meta.disc_no,
                meta.year,
                meta.genre,
                meta.duration,
                meta.cover,
                now(),
                imported,
                mtime_of(path),
            ],
        )?;
        let track = self
            .track_by_path(&path_str)?
            .ok_or_else(|| Error::NotFound(path_str.clone()))?;
        Ok((track, !existed))
    }

    /// Copia un archivo de audio dentro de la app (útil en el teléfono, donde
    /// la música queda guardada en el almacenamiento privado y siempre
    /// disponible offline) y lo añade a la biblioteca.
    pub fn import_bytes(&self, file_name: &str, bytes: &[u8]) -> Result<Track> {
        let dest = self.unique_dest(file_name)?;
        fs::write(&dest, bytes)?;
        self.register_imported(&dest)
    }

    /// Mueve a la biblioteca un archivo ya descargado dentro de `music_dir`
    /// (por ejemplo, desde la sincronización por Wi‑Fi) sin volver a copiarlo.
    pub fn adopt_file(&self, tmp: &Path, file_name: &str) -> Result<Track> {
        let dest = match self.unique_dest(file_name) {
            Ok(d) => d,
            Err(e) => {
                let _ = fs::remove_file(tmp);
                return Err(e);
            }
        };
        fs::rename(tmp, &dest)?;
        self.register_imported(&dest)
    }

    /// Ruta libre dentro de `music_dir` para un archivo con ese nombre.
    fn unique_dest(&self, file_name: &str) -> Result<PathBuf> {
        let name = sanitize_file_name(file_name);
        if !is_audio_file(Path::new(&name)) {
            return Err(Error::Unsupported(name));
        }
        let stem = Path::new(&name)
            .file_stem()
            .map(|s| s.to_string_lossy().into_owned())
            .unwrap_or_default();
        let ext = Path::new(&name)
            .extension()
            .map(|s| s.to_string_lossy().into_owned())
            .unwrap_or_default();

        let mut dest = self.music_dir.join(&name);
        let mut n = 1;
        while dest.exists() {
            dest = self.music_dir.join(format!("{stem} ({n}).{ext}"));
            n += 1;
        }
        Ok(dest)
    }

    fn register_imported(&self, dest: &Path) -> Result<Track> {
        match self.upsert_file(dest, true) {
            Ok((track, _)) => Ok(track),
            Err(e) => {
                let _ = fs::remove_file(dest);
                Err(e)
            }
        }
    }

    /// Clave de identidad de cada canción local (ver [`crate::sync::track_key`]),
    /// usada para saber qué canciones de la PC ya están en este dispositivo.
    pub fn local_keys(&self) -> Result<HashMap<String, i64>> {
        Ok(self
            .tracks()?
            .into_iter()
            .map(|t| {
                let size = fs::metadata(&t.path).map(|m| m.len()).unwrap_or(0);
                (
                    crate::sync::track_key(&t.title, &t.artist, &t.album, size),
                    t.id,
                )
            })
            .collect())
    }

    /// Importa (copiando) un archivo que ya está en disco.
    pub fn import_path(&self, src: &Path) -> Result<Track> {
        let name = src
            .file_name()
            .map(|s| s.to_string_lossy().into_owned())
            .unwrap_or_default();
        let bytes = fs::read(src)?;
        self.import_bytes(&name, &bytes)
    }

    /// Elimina la canción de la biblioteca. Si fue importada, borra también
    /// el archivo de la app; los archivos de carpetas externas no se tocan.
    pub fn delete_track(&self, id: i64) -> Result<()> {
        let track = self.track(id)?;
        self.conn
            .execute("DELETE FROM tracks WHERE id = ?1", [id])?;
        if track.imported {
            let _ = fs::remove_file(&track.path);
        }
        self.compact_all_playlists()?;
        Ok(())
    }

    pub fn set_favorite(&self, id: i64, favorite: bool) -> Result<()> {
        let n = self.conn.execute(
            "UPDATE tracks SET favorite = ?2 WHERE id = ?1",
            params![id, favorite],
        )?;
        if n == 0 {
            return Err(Error::NotFound(format!("canción {id}")));
        }
        Ok(())
    }

    pub fn record_play(&self, id: i64) -> Result<()> {
        self.conn.execute(
            "UPDATE tracks SET play_count = play_count + 1, last_played = ?2 WHERE id = ?1",
            params![id, now()],
        )?;
        Ok(())
    }

    // ------------------------------------------------------------------ carpetas

    pub fn folders(&self) -> Result<Vec<Folder>> {
        let mut stmt = self
            .conn
            .prepare("SELECT id, path FROM folders ORDER BY path")?;
        let rows = stmt.query_map([], |r| {
            Ok(Folder {
                id: r.get(0)?,
                path: r.get(1)?,
            })
        })?;
        Ok(rows.collect::<rusqlite::Result<_>>()?)
    }

    /// Registra una carpeta externa y la escanea.
    pub fn add_folder(&self, path: &Path) -> Result<ScanReport> {
        if !path.is_dir() {
            return Err(Error::NotFound(path.display().to_string()));
        }
        self.conn.execute(
            "INSERT OR IGNORE INTO folders (path) VALUES (?1)",
            [path.to_string_lossy()],
        )?;
        self.scan_folder(path)
    }

    /// Quita la carpeta y todas sus canciones de la biblioteca (sin borrar archivos).
    pub fn remove_folder(&self, id: i64) -> Result<()> {
        let path: String = self
            .conn
            .query_row("SELECT path FROM folders WHERE id = ?1", [id], |r| r.get(0))
            .optional()?
            .ok_or_else(|| Error::NotFound(format!("carpeta {id}")))?;
        let tx = self.conn.unchecked_transaction()?;
        tx.execute("DELETE FROM folders WHERE id = ?1", [id])?;
        let ids = self.track_ids_under(&path)?;
        for track_id in ids {
            tx.execute(
                "DELETE FROM tracks WHERE id = ?1 AND imported = 0",
                [track_id],
            )?;
        }
        tx.commit()?;
        self.compact_all_playlists()?;
        Ok(())
    }

    fn track_ids_under(&self, folder: &str) -> Result<Vec<i64>> {
        let folder = Path::new(folder);
        let mut stmt = self
            .conn
            .prepare("SELECT id, path FROM tracks WHERE imported = 0")?;
        let rows = stmt.query_map([], |r| Ok((r.get::<_, i64>(0)?, r.get::<_, String>(1)?)))?;
        let mut ids = Vec::new();
        for row in rows {
            let (id, path) = row?;
            if Path::new(&path).starts_with(folder) {
                ids.push(id);
            }
        }
        Ok(ids)
    }

    /// Re-escanea todas las carpetas registradas y limpia canciones importadas
    /// cuyo archivo haya desaparecido.
    pub fn rescan_all(&self) -> Result<ScanReport> {
        let mut total = ScanReport::default();
        for folder in self.folders()? {
            let r = self.scan_folder(Path::new(&folder.path))?;
            total.added += r.added;
            total.updated += r.updated;
            total.removed += r.removed;
            total.unchanged += r.unchanged;
            total.errors.extend(r.errors);
        }
        let mut stmt = self
            .conn
            .prepare("SELECT id, path FROM tracks WHERE imported = 1")?;
        let missing: Vec<i64> = stmt
            .query_map([], |r| Ok((r.get::<_, i64>(0)?, r.get::<_, String>(1)?)))?
            .filter_map(|r| r.ok())
            .filter(|(_, p)| !Path::new(p).exists())
            .map(|(id, _)| id)
            .collect();
        for id in &missing {
            self.conn
                .execute("DELETE FROM tracks WHERE id = ?1", [id])?;
        }
        total.removed += missing.len();
        self.compact_all_playlists()?;
        Ok(total)
    }

    fn scan_folder(&self, folder: &Path) -> Result<ScanReport> {
        let mut report = ScanReport::default();
        let mut seen = HashSet::new();

        for entry in WalkDir::new(folder)
            .follow_links(true)
            .into_iter()
            .filter_map(|e| e.ok())
        {
            let path = entry.path();
            if !entry.file_type().is_file() || !is_audio_file(path) {
                continue;
            }
            let path_str = path.to_string_lossy().into_owned();
            seen.insert(path_str.clone());

            let unchanged = self
                .conn
                .query_row(
                    "SELECT mtime FROM tracks WHERE path = ?1",
                    [&path_str],
                    |r| r.get::<_, i64>(0),
                )
                .optional()?
                .is_some_and(|m| m == mtime_of(path));
            if unchanged {
                report.unchanged += 1;
                continue;
            }
            match self.upsert_file(path, false) {
                Ok((_, true)) => report.added += 1,
                Ok((_, false)) => report.updated += 1,
                Err(e) => report.errors.push(e.to_string()),
            }
        }

        let folder_str = folder.to_string_lossy();
        let mut stmt = self
            .conn
            .prepare("SELECT id, path FROM tracks WHERE imported = 0")?;
        let stale: Vec<i64> = stmt
            .query_map([], |r| Ok((r.get::<_, i64>(0)?, r.get::<_, String>(1)?)))?
            .filter_map(|r| r.ok())
            .filter(|(_, p)| Path::new(p).starts_with(&*folder_str) && !seen.contains(p))
            .map(|(id, _)| id)
            .collect();
        for id in &stale {
            self.conn
                .execute("DELETE FROM tracks WHERE id = ?1", [id])?;
        }
        report.removed = stale.len();
        if !stale.is_empty() {
            self.compact_all_playlists()?;
        }
        Ok(report)
    }

    // ----------------------------------------------------------------- playlists

    pub fn playlists(&self) -> Result<Vec<Playlist>> {
        let mut stmt = self
            .conn
            .prepare("SELECT id, name, created_at FROM playlists ORDER BY name COLLATE NOCASE")?;
        let lists: Vec<(i64, String, i64)> = stmt
            .query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))?
            .collect::<rusqlite::Result<_>>()?;
        lists
            .into_iter()
            .map(|(id, name, created_at)| {
                Ok(Playlist {
                    id,
                    name,
                    created_at,
                    track_ids: self.playlist_track_ids(id)?,
                })
            })
            .collect()
    }

    fn playlist_track_ids(&self, playlist_id: i64) -> Result<Vec<i64>> {
        let mut stmt = self.conn.prepare(
            "SELECT track_id FROM playlist_tracks WHERE playlist_id = ?1 ORDER BY position",
        )?;
        let ids = stmt.query_map([playlist_id], |r| r.get(0))?;
        Ok(ids.collect::<rusqlite::Result<_>>()?)
    }

    pub fn create_playlist(&self, name: &str) -> Result<Playlist> {
        let name = name.trim();
        let name = if name.is_empty() {
            "Nueva playlist"
        } else {
            name
        };
        let created_at = now();
        self.conn.execute(
            "INSERT INTO playlists (name, created_at) VALUES (?1, ?2)",
            params![name, created_at],
        )?;
        Ok(Playlist {
            id: self.conn.last_insert_rowid(),
            name: name.to_string(),
            created_at,
            track_ids: vec![],
        })
    }

    pub fn rename_playlist(&self, id: i64, name: &str) -> Result<()> {
        self.conn.execute(
            "UPDATE playlists SET name = ?2 WHERE id = ?1",
            params![id, name.trim()],
        )?;
        Ok(())
    }

    pub fn delete_playlist(&self, id: i64) -> Result<()> {
        self.conn
            .execute("DELETE FROM playlists WHERE id = ?1", [id])?;
        Ok(())
    }

    /// Reemplaza el contenido completo de la playlist (sirve para añadir,
    /// quitar y reordenar desde la interfaz).
    pub fn set_playlist_tracks(&self, id: i64, track_ids: &[i64]) -> Result<()> {
        let tx = self.conn.unchecked_transaction()?;
        tx.execute("DELETE FROM playlist_tracks WHERE playlist_id = ?1", [id])?;
        {
            let mut stmt = tx.prepare(
                "INSERT INTO playlist_tracks (playlist_id, track_id, position) VALUES (?1, ?2, ?3)",
            )?;
            for (pos, track_id) in track_ids.iter().enumerate() {
                stmt.execute(params![id, track_id, pos as i64])?;
            }
        }
        tx.commit()?;
        Ok(())
    }

    /// Crea la playlist `name` o, si ya existe una con ese nombre, reemplaza
    /// su contenido. Devuelve su id.
    pub fn upsert_playlist_by_name(&self, name: &str, track_ids: &[i64]) -> Result<i64> {
        let existing: Option<i64> = self
            .conn
            .query_row(
                "SELECT id FROM playlists WHERE name = ?1 COLLATE NOCASE ORDER BY id LIMIT 1",
                [name.trim()],
                |r| r.get(0),
            )
            .optional()?;
        let id = match existing {
            Some(id) => id,
            None => self.create_playlist(name)?.id,
        };
        self.set_playlist_tracks(id, track_ids)?;
        Ok(id)
    }

    pub fn add_to_playlist(&self, id: i64, track_ids: &[i64]) -> Result<()> {
        let mut ids = self.playlist_track_ids(id)?;
        ids.extend_from_slice(track_ids);
        self.set_playlist_tracks(id, &ids)
    }

    /// Reindexa las posiciones tras borrar canciones (el ON DELETE CASCADE
    /// puede dejar huecos).
    fn compact_all_playlists(&self) -> Result<()> {
        let ids: Vec<i64> = self
            .conn
            .prepare("SELECT id FROM playlists")?
            .query_map([], |r| r.get(0))?
            .collect::<rusqlite::Result<_>>()?;
        for id in ids {
            let tracks = self.playlist_track_ids(id)?;
            self.set_playlist_tracks(id, &tracks)?;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// WAV PCM mínimo y válido (silencio) para poder probar sin archivos externos.
    fn tiny_wav(samples: u32) -> Vec<u8> {
        let data_len = samples * 2;
        let mut v = Vec::new();
        v.extend_from_slice(b"RIFF");
        v.extend_from_slice(&(36 + data_len).to_le_bytes());
        v.extend_from_slice(b"WAVEfmt ");
        v.extend_from_slice(&16u32.to_le_bytes());
        v.extend_from_slice(&1u16.to_le_bytes()); // PCM
        v.extend_from_slice(&1u16.to_le_bytes()); // mono
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
    fn sanitizes_names() {
        assert_eq!(sanitize_file_name("../../etc/passwd"), "passwd");
        assert_eq!(sanitize_file_name("a:b?.mp3"), "a_b_.mp3");
        assert_eq!(sanitize_file_name("..."), "cancion");
    }

    #[test]
    fn import_favorite_and_delete() {
        let dir = tempfile::tempdir().unwrap();
        let lib = Library::open(dir.path()).unwrap();

        let t = lib.import_bytes("Mi canción.wav", &tiny_wav(8000)).unwrap();
        assert_eq!(t.title, "Mi canción");
        assert!(t.imported);
        assert!((t.duration - 1.0).abs() < 0.05);
        assert!(Path::new(&t.path).starts_with(lib.music_dir()));

        // Mismo nombre: no pisa el archivo anterior.
        let t2 = lib.import_bytes("Mi canción.wav", &tiny_wav(800)).unwrap();
        assert_ne!(t.path, t2.path);

        lib.set_favorite(t.id, true).unwrap();
        lib.record_play(t.id).unwrap();
        let t = lib.track(t.id).unwrap();
        assert!(t.favorite);
        assert_eq!(t.play_count, 1);

        lib.delete_track(t.id).unwrap();
        assert!(!Path::new(&t.path).exists());
        assert_eq!(lib.tracks().unwrap().len(), 1);
    }

    #[test]
    fn rejects_non_audio() {
        let dir = tempfile::tempdir().unwrap();
        let lib = Library::open(dir.path()).unwrap();
        assert!(lib.import_bytes("foto.jpg", b"123").is_err());
        assert!(lib.import_bytes("roto.mp3", b"no es audio").is_err());
        assert_eq!(fs::read_dir(lib.music_dir()).unwrap().count(), 0);
    }

    #[test]
    fn folders_and_playlists() {
        let data = tempfile::tempdir().unwrap();
        let music = tempfile::tempdir().unwrap();
        fs::create_dir(music.path().join("sub")).unwrap();
        fs::write(music.path().join("a.wav"), tiny_wav(100)).unwrap();
        fs::write(music.path().join("sub/b.wav"), tiny_wav(100)).unwrap();
        fs::write(music.path().join("notas.txt"), b"hola").unwrap();

        let lib = Library::open(data.path()).unwrap();
        let r = lib.add_folder(music.path()).unwrap();
        assert_eq!(r.added, 2);

        let r = lib.rescan_all().unwrap();
        assert_eq!((r.added, r.unchanged), (0, 2));

        let ids: Vec<i64> = lib.tracks().unwrap().iter().map(|t| t.id).collect();
        let p = lib.create_playlist("  Rock  ").unwrap();
        assert_eq!(p.name, "Rock");
        lib.add_to_playlist(p.id, &ids).unwrap();
        lib.add_to_playlist(p.id, &ids[..1]).unwrap();
        assert_eq!(lib.playlists().unwrap()[0].track_ids.len(), 3);

        fs::remove_file(music.path().join("a.wav")).unwrap();
        let r = lib.rescan_all().unwrap();
        assert_eq!(r.removed, 1);
        assert_eq!(lib.playlists().unwrap()[0].track_ids.len(), 1);

        let folder = lib.folders().unwrap()[0].id;
        lib.remove_folder(folder).unwrap();
        assert!(lib.tracks().unwrap().is_empty());
        assert!(music.path().join("sub/b.wav").exists());
    }
}
