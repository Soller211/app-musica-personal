//! Núcleo de la app de música personal.
//!
//! Todo funciona offline: los metadatos se leen de los propios archivos de
//! audio y se guardan en una base de datos SQLite local junto con las
//! playlists, favoritos y estadísticas de reproducción.

mod db;
mod error;
pub mod link;
mod metadata;
mod model;
pub mod sync;

pub use db::{Library, PendingUpload};
pub use error::{Error, Result};
pub use metadata::{is_audio_file, AUDIO_EXTENSIONS};
pub use model::{Folder, Playlist, ScanReport, Track};
