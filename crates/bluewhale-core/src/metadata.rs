use std::collections::hash_map::DefaultHasher;
use std::fs;
use std::hash::{Hash, Hasher};
use std::path::Path;

use lofty::picture::{MimeType, PictureType};
use lofty::prelude::*;
use lofty::tag::ItemKey;

use crate::model::TrackMeta;
use crate::{Error, Result};

/// Extensiones que la app reconoce como audio.
pub const AUDIO_EXTENSIONS: &[&str] = &[
    "mp3", "flac", "ogg", "oga", "opus", "m4a", "mp4", "aac", "wav", "aiff", "aif", "wma",
];

pub fn is_audio_file(path: &Path) -> bool {
    path.extension()
        .and_then(|e| e.to_str())
        .map(|e| AUDIO_EXTENSIONS.contains(&e.to_ascii_lowercase().as_str()))
        .unwrap_or(false)
}

/// Lee los metadatos de `path` y guarda su carátula (si la tiene) en `covers_dir`.
pub(crate) fn read_meta(path: &Path, covers_dir: &Path) -> Result<TrackMeta> {
    let tagged = lofty::read_from_path(path).map_err(|e| Error::Metadata {
        path: path.to_path_buf(),
        reason: e.to_string(),
    })?;

    let fallback_title = path
        .file_stem()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_else(|| "Sin título".into());

    let mut meta = TrackMeta {
        title: fallback_title,
        duration: tagged.properties().duration().as_secs_f64(),
        ..Default::default()
    };

    if let Some(tag) = tagged.primary_tag().or_else(|| tagged.first_tag()) {
        let clean = |s: Option<std::borrow::Cow<'_, str>>| {
            s.map(|v| v.trim().to_string()).filter(|v| !v.is_empty())
        };
        if let Some(t) = clean(tag.title()) {
            meta.title = t;
        }
        meta.artist = clean(tag.artist()).unwrap_or_default();
        meta.album = clean(tag.album()).unwrap_or_default();
        meta.album_artist = tag
            .get_string(ItemKey::AlbumArtist)
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty())
            .unwrap_or_default();
        meta.genre = clean(tag.genre());
        meta.track_no = tag.track();
        meta.disc_no = tag.disk();
        meta.year = tag.date().map(|d| u32::from(d.year));

        let picture = tag
            .pictures()
            .iter()
            .find(|p| p.pic_type() == PictureType::CoverFront)
            .or_else(|| tag.pictures().first());
        if let Some(pic) = picture {
            meta.cover = save_cover(pic.data(), pic.mime_type(), covers_dir).ok();
        }
    }

    if meta.artist.is_empty() {
        meta.artist = "Artista desconocido".into();
    }
    if meta.album.is_empty() {
        meta.album = "Álbum desconocido".into();
    }
    if meta.album_artist.is_empty() {
        meta.album_artist = meta.artist.clone();
    }
    Ok(meta)
}

/// Guarda la carátula con un nombre derivado de su contenido, de modo que
/// todas las canciones de un mismo álbum compartan un único archivo.
fn save_cover(data: &[u8], mime: Option<&MimeType>, covers_dir: &Path) -> Result<String> {
    let ext = match mime {
        Some(MimeType::Png) => "png",
        Some(MimeType::Gif) => "gif",
        Some(MimeType::Bmp) => "bmp",
        _ => "jpg",
    };
    let mut hasher = DefaultHasher::new();
    data.hash(&mut hasher);
    let file = covers_dir.join(format!("{:016x}.{ext}", hasher.finish()));
    if !file.exists() {
        fs::create_dir_all(covers_dir)?;
        fs::write(&file, data)?;
    }
    Ok(file.to_string_lossy().into_owned())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn detects_audio_extensions() {
        assert!(is_audio_file(Path::new("/a/b/canción.MP3")));
        assert!(is_audio_file(Path::new("x.flac")));
        assert!(!is_audio_file(Path::new("cover.jpg")));
        assert!(!is_audio_file(Path::new("sin_extension")));
    }
}
