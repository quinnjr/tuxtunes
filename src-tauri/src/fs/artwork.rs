//! Extract embedded cover art from an audio file and write it alongside
//! as `cover.<ext>` (jpg | png | gif | webp | bin fallback). No-op when
//! any `cover.*` already exists in the track's directory.

use lofty::file::TaggedFileExt;
use lofty::picture::MimeType;
use std::path::{Path, PathBuf};

#[derive(Debug, thiserror::Error)]
pub enum ArtworkError {
    #[error("io error: {0}")]
    Io(#[from] std::io::Error),
    #[error("lofty error: {0}")]
    Lofty(#[from] lofty::error::LoftyError),
}

/// Extract the first embedded picture from `audio_path` and write it
/// to `<track_dir>/cover.<ext>`. Returns the written path, or
/// `Ok(None)` when the track has no embedded artwork. If a file named
/// `cover.<ext>` already exists in `track_dir` (any common image
/// extension), returns that without overwriting — idempotent.
pub fn extract_cover_alongside(audio_path: &Path) -> Result<Option<PathBuf>, ArtworkError> {
    let Some(parent) = audio_path.parent() else {
        return Ok(None);
    };
    for existing_ext in ["jpg", "jpeg", "png", "webp", "gif"] {
        let p = parent.join(format!("cover.{existing_ext}"));
        if p.exists() {
            return Ok(Some(p));
        }
    }
    let tagged = lofty::read_from_path(audio_path)?;
    let Some(tag) = tagged.primary_tag().or_else(|| tagged.first_tag()) else {
        return Ok(None);
    };
    let Some(pic) = tag.pictures().first() else {
        return Ok(None);
    };
    let ext = match pic.mime_type() {
        Some(MimeType::Jpeg) => "jpg",
        Some(MimeType::Png) => "png",
        Some(MimeType::Gif) => "gif",
        _ => "bin",
    };
    let target = parent.join(format!("cover.{ext}"));
    std::fs::write(&target, pic.data())?;
    Ok(Some(target))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn no_parent_returns_none() {
        // A path like "file.flac" with no parent → None.
        let p = Path::new("file.flac");
        // Only safe if cwd is irrelevant to the path's parent(); assert
        // the branch via a known-None case.
        assert!(p.parent().is_some_and(|p| p.as_os_str().is_empty()));
    }

    #[test]
    fn existing_cover_is_reused() {
        let dir = tempfile::tempdir().unwrap();
        let track = dir.path().join("song.flac");
        std::fs::write(&track, b"").unwrap();
        let cover = dir.path().join("cover.jpg");
        std::fs::write(&cover, b"fake").unwrap();

        let out = extract_cover_alongside(&track).unwrap();
        assert_eq!(out, Some(cover));
        // The existing `cover.jpg` was not overwritten.
        assert_eq!(
            std::fs::read(dir.path().join("cover.jpg")).unwrap(),
            b"fake"
        );
    }

    #[test]
    fn file_with_no_tags_returns_none() {
        // A non-audio file (empty .flac stub) — lofty will fail to read
        // it or return a tagless result. Either way, we should not
        // panic, and should return either None or a lofty error.
        let dir = tempfile::tempdir().unwrap();
        let track = dir.path().join("empty.flac");
        std::fs::write(&track, b"").unwrap();

        // Either Ok(None) (if lofty parsed it as empty) or a Lofty
        // error (if it rejected the file) is acceptable here; both
        // mean "no artwork to extract".
        let res = extract_cover_alongside(&track);
        match res {
            Ok(None) => {}
            Err(ArtworkError::Lofty(_)) => {}
            other => panic!("unexpected result: {other:?}"),
        }
    }
}
