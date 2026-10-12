use std::path::PathBuf;

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("error de base de datos: {0}")]
    Db(#[from] rusqlite::Error),
    #[error("error de E/S: {0}")]
    Io(#[from] std::io::Error),
    #[error("no se pudieron leer los metadatos de {path}: {reason}")]
    Metadata { path: PathBuf, reason: String },
    #[error("«{0}» no es un formato de audio compatible (MP3, M4A, FLAC, OGG, OPUS, WAV…)")]
    Unsupported(String),
    #[error("{0}")]
    Network(String),
    #[error("no encontrado: {0}")]
    NotFound(String),
}

impl serde::Serialize for Error {
    fn serialize<S: serde::Serializer>(&self, s: S) -> std::result::Result<S::Ok, S::Error> {
        s.serialize_str(&self.to_string())
    }
}

pub type Result<T> = std::result::Result<T, Error>;
