//! Write user-edited metadata into the audio file's own tags, so an
//! edit made in TuxTunes survives outside it (other players, future
//! re-imports).

use crate::db::tracks::MetadataEdit;
use lofty::config::WriteOptions;
use lofty::file::TaggedFileExt;
use lofty::tag::{Accessor, ItemKey, Tag, TagExt};
use std::path::Path;

#[derive(Debug, thiserror::Error)]
pub enum TagsError {
    #[error("file not found: {0}")]
    NotFound(String),
    #[error("tag write failed: {0}")]
    Write(#[source] anyhow::Error),
}

/// Write the edit's fields into `path`'s primary tag (created in the
/// format's default tag type when the file has none yet). A `None`
/// field clears the corresponding tag entry.
pub fn write_metadata(path: &Path, e: &MetadataEdit<'_>) -> Result<(), TagsError> {
    if !path.exists() {
        return Err(TagsError::NotFound(path.display().to_string()));
    }
    let tagged =
        lofty::read_from_path(path).map_err(|err| TagsError::Write(anyhow::Error::from(err)))?;
    let mut tag = match tagged.primary_tag() {
        Some(t) => t.clone(),
        None => Tag::new(tagged.primary_tag_type()),
    };

    tag.set_title(e.title.trim().to_string());
    set_or_remove_text(&mut tag, ItemKey::TrackArtist, e.artist);
    set_or_remove_text(&mut tag, ItemKey::AlbumTitle, e.album);
    set_or_remove_text(&mut tag, ItemKey::AlbumArtist, e.album_artist);
    set_or_remove_text(&mut tag, ItemKey::Genre, e.genre);
    match e.year {
        Some(y) => tag.set_year(y as u32),
        None => tag.remove_year(),
    }
    match e.track_number {
        Some(n) => tag.set_track(n as u32),
        None => tag.remove_track(),
    }
    match e.disc_number {
        Some(n) => tag.set_disk(n as u32),
        None => tag.remove_disk(),
    }

    tag.save_to_path(path, WriteOptions::default())
        .map_err(|err| TagsError::Write(anyhow::Error::from(err)))
}

fn set_or_remove_text(tag: &mut Tag, key: ItemKey, value: Option<&str>) {
    match value {
        Some(v) if !v.trim().is_empty() => {
            tag.insert_text(key, v.trim().to_string());
        }
        _ => {
            tag.remove_key(&key);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn write_minimal_wav(path: &Path) {
        let bytes: &[u8] = &[
            b'R', b'I', b'F', b'F', 0x26, 0, 0, 0, b'W', b'A', b'V', b'E', b'f', b'm', b't', b' ',
            0x10, 0, 0, 0, 0x01, 0, 0x01, 0, 0x40, 0x1f, 0, 0, 0x40, 0x1f, 0, 0, 0x01, 0, 0x08, 0,
            b'd', b'a', b't', b'a', 0x02, 0, 0, 0, 0x80, 0x80,
        ];
        std::fs::write(path, bytes).unwrap();
    }

    fn edit() -> MetadataEdit<'static> {
        MetadataEdit {
            title: "Anthem, Pt. 2",
            artist: Some("blink-182"),
            album: Some("Take Off Your Pants and Jacket"),
            album_artist: Some("blink-182"),
            genre: Some("Punk"),
            year: Some(2001),
            track_number: Some(1),
            disc_number: None,
        }
    }

    #[test]
    fn writes_and_reads_back_all_fields() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("t.wav");
        write_minimal_wav(&file);
        write_metadata(&file, &edit()).unwrap();

        let tagged = lofty::read_from_path(&file).unwrap();
        let tag = tagged.primary_tag().expect("tag written");
        assert_eq!(tag.title().as_deref(), Some("Anthem, Pt. 2"));
        assert_eq!(tag.artist().as_deref(), Some("blink-182"));
        assert_eq!(
            tag.album().as_deref(),
            Some("Take Off Your Pants and Jacket")
        );
        assert_eq!(tag.get_string(&ItemKey::AlbumArtist), Some("blink-182"));
        assert_eq!(tag.genre().as_deref(), Some("Punk"));
        assert_eq!(tag.year(), Some(2001));
        assert_eq!(tag.track(), Some(1));
        assert_eq!(tag.disk(), None);
    }

    #[test]
    fn clearing_a_field_removes_it_from_the_tag() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("t.wav");
        write_minimal_wav(&file);
        write_metadata(&file, &edit()).unwrap();
        let cleared = MetadataEdit {
            genre: None,
            ..edit()
        };
        write_metadata(&file, &cleared).unwrap();
        let tagged = lofty::read_from_path(&file).unwrap();
        let tag = tagged.primary_tag().unwrap();
        assert_eq!(tag.genre(), None);
        assert_eq!(tag.artist().as_deref(), Some("blink-182"));
    }

    #[test]
    fn missing_file_is_a_not_found_error() {
        let err = write_metadata(Path::new("/nonexistent/x.wav"), &edit()).unwrap_err();
        assert!(matches!(err, TagsError::NotFound(_)), "{err}");
    }
}
