//! Write user-edited metadata into the audio file's own tags, so an
//! edit made in TuxTunes survives outside it (other players, future
//! re-imports).

use crate::db::tracks::MetadataEdit;
use lofty::config::WriteOptions;
use lofty::file::TaggedFileExt;
use lofty::picture::{MimeType, Picture, PictureType};
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

/// Embed `image` as the file's front cover, replacing any picture
/// already there. `image_path` is read for its bytes; its extension
/// decides the MIME type recorded in the tag.
///
/// A cover TuxTunes resolved (from a sidecar, or from a sibling track
/// on the same album) lives only in its own cache until this puts it in
/// the file, where every other player can see it.
pub fn write_cover(path: &Path, image_path: &Path) -> Result<(), TagsError> {
    if !path.exists() {
        return Err(TagsError::NotFound(path.display().to_string()));
    }
    let data = std::fs::read(image_path).map_err(|e| TagsError::Write(anyhow::Error::from(e)))?;
    let mime = mime_for(image_path);

    let tagged =
        lofty::read_from_path(path).map_err(|err| TagsError::Write(anyhow::Error::from(err)))?;
    let mut tag = match tagged.primary_tag() {
        Some(t) => t.clone(),
        None => Tag::new(tagged.primary_tag_type()),
    };

    // One front cover, not a pile of them: drop what is there before
    // inserting, or a repeated write-back would stack duplicates.
    while tag
        .pictures()
        .iter()
        .any(|p| p.pic_type() == PictureType::CoverFront)
    {
        let Some(i) = tag
            .pictures()
            .iter()
            .position(|p| p.pic_type() == PictureType::CoverFront)
        else {
            break;
        };
        tag.remove_picture(i);
    }

    tag.push_picture(Picture::new_unchecked(
        PictureType::CoverFront,
        Some(mime),
        None,
        data,
    ));

    tag.save_to_path(path, WriteOptions::default())
        .map_err(|err| TagsError::Write(anyhow::Error::from(err)))
}

/// Whether `path` already carries an embedded picture.
pub fn has_embedded_cover(path: &Path) -> bool {
    crate::library::artwork::extract_embedded(path).is_some()
}

fn mime_for(image_path: &Path) -> MimeType {
    match image_path
        .extension()
        .and_then(|e| e.to_str())
        .map(str::to_ascii_lowercase)
        .as_deref()
    {
        Some("png") => MimeType::Png,
        Some("gif") => MimeType::Gif,
        Some("bmp") => MimeType::Bmp,
        // Lofty has no WebP variant; the raw type keeps the bytes
        // readable to anything that sniffs the magic number.
        Some("webp") => MimeType::Unknown("image/webp".to_string()),
        _ => MimeType::Jpeg,
    }
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

    /// Smallest valid PNG: an 8-bit 1x1 image.
    const PNG_1PX: &[u8] = &[
        0x89, b'P', b'N', b'G', 0x0d, 0x0a, 0x1a, 0x0a, 0, 0, 0, 0x0d, b'I', b'H', b'D', b'R', 0,
        0, 0, 1, 0, 0, 0, 1, 8, 6, 0, 0, 0, 0x1f, 0x15, 0xc4, 0x89, 0, 0, 0, 0x0a, b'I', b'D',
        b'A', b'T', 0x78, 0x9c, 0x63, 0, 1, 0, 0, 5, 0, 1, 0x0d, 0x0a, 0x2d, 0xb4, 0, 0, 0, 0,
        b'I', b'E', b'N', b'D', 0xae, 0x42, 0x60, 0x82,
    ];

    #[test]
    fn write_cover_embeds_a_front_cover_that_is_readable_back() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("t.wav");
        write_minimal_wav(&file);
        let art = dir.path().join("cover.png");
        std::fs::write(&art, PNG_1PX).unwrap();

        assert!(!has_embedded_cover(&file), "fixture starts with no picture");
        write_cover(&file, &art).unwrap();

        assert!(has_embedded_cover(&file));
        let found = crate::library::artwork::extract_embedded(&file).unwrap();
        assert_eq!(found.data, PNG_1PX);
        assert_eq!(found.ext, "png");
    }

    #[test]
    fn write_cover_replaces_rather_than_stacks_front_covers() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("t.wav");
        write_minimal_wav(&file);
        let art = dir.path().join("cover.png");
        std::fs::write(&art, PNG_1PX).unwrap();

        write_cover(&file, &art).unwrap();
        write_cover(&file, &art).unwrap();

        let tagged = lofty::read_from_path(&file).unwrap();
        let fronts = tagged
            .primary_tag()
            .unwrap()
            .pictures()
            .iter()
            .filter(|p| p.pic_type() == PictureType::CoverFront)
            .count();
        assert_eq!(fronts, 1, "a repeated write-back stacked duplicate covers");
    }

    #[test]
    fn write_cover_reports_a_missing_audio_file() {
        let dir = tempfile::tempdir().unwrap();
        let art = dir.path().join("cover.png");
        std::fs::write(&art, PNG_1PX).unwrap();
        let err = write_cover(Path::new("/nonexistent/x.wav"), &art).unwrap_err();
        assert!(matches!(err, TagsError::NotFound(_)), "{err}");
    }

    #[test]
    fn missing_file_is_a_not_found_error() {
        let err = write_metadata(Path::new("/nonexistent/x.wav"), &edit()).unwrap_err();
        assert!(matches!(err, TagsError::NotFound(_)), "{err}");
    }
}
