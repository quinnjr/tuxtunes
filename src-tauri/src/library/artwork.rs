//! Album artwork discovery.
//!
//! Nothing in the iTunes `.itl` carries usable cover images (its
//! `Album Artwork` cache is a proprietary `.itc` format), so covers are
//! recovered from the audio files themselves: the embedded picture tag
//! when present, else a `cover.jpg`-style sidecar in the track's
//! directory. Whatever is found is copied into a content-addressed
//! cache directory so the webview's asset scope can stay pinned to one
//! folder rather than the whole music tree.

use lofty::file::TaggedFileExt;
use lofty::picture::{MimeType, PictureType};
use lofty::probe::Probe;
use std::hash::Hasher;
use std::io;
use std::path::{Path, PathBuf};
use twox_hash::XxHash64;

/// Sidecar file names checked (case-sensitively, in order) next to a
/// track when it has no embedded picture.
const SIDECAR_STEMS: &[&str] = &[
    "cover", "Cover", "folder", "Folder", "front", "Front", "album",
];
const SIDECAR_EXTS: &[&str] = &["jpg", "jpeg", "png"];

/// How many of an album's tracks to probe before giving up. Albums
/// where the first few files lack art almost never have it later, and
/// each probe reads the file's tag block.
pub const MAX_PROBES_PER_ALBUM: usize = 4;

/// Raw image bytes plus the file extension they should be stored under.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ImageBytes {
    pub data: Vec<u8>,
    pub ext: &'static str,
}

fn ext_for_mime(mime: Option<&MimeType>) -> Option<&'static str> {
    match mime? {
        MimeType::Jpeg => Some("jpg"),
        MimeType::Png => Some("png"),
        MimeType::Gif => Some("gif"),
        MimeType::Bmp => Some("bmp"),
        MimeType::Tiff => Some("tiff"),
        MimeType::Unknown(_) => None,
        _ => None,
    }
}

/// Guess an image type from magic bytes when the tag's MIME is missing
/// or bogus (common in older iTunes-written MP4 `covr` atoms).
fn ext_from_magic(data: &[u8]) -> Option<&'static str> {
    if data.starts_with(&[0xFF, 0xD8, 0xFF]) {
        Some("jpg")
    } else if data.starts_with(&[0x89, b'P', b'N', b'G']) {
        Some("png")
    } else if data.starts_with(b"GIF8") {
        Some("gif")
    } else if data.starts_with(b"BM") {
        Some("bmp")
    } else {
        None
    }
}

/// Read the embedded cover from an audio file. Prefers a picture typed
/// `CoverFront`; falls back to the first picture of any type. Returns
/// None for files with no tags, no pictures, or unreadable images.
pub fn extract_embedded(path: &Path) -> Option<ImageBytes> {
    let tagged = Probe::open(path).ok()?.read().ok()?;
    let mut fallback: Option<ImageBytes> = None;
    for tag in tagged.tags() {
        for pic in tag.pictures() {
            let data = pic.data();
            if data.is_empty() {
                continue;
            }
            let Some(ext) = ext_from_magic(data).or_else(|| ext_for_mime(pic.mime_type())) else {
                continue;
            };
            let img = ImageBytes {
                data: data.to_vec(),
                ext,
            };
            if pic.pic_type() == PictureType::CoverFront {
                return Some(img);
            }
            if fallback.is_none() {
                fallback = Some(img);
            }
        }
    }
    fallback
}

/// Look for a conventional cover image next to `track_path`.
pub fn find_sidecar(track_path: &Path) -> Option<PathBuf> {
    let dir = track_path.parent()?;
    for stem in SIDECAR_STEMS {
        for ext in SIDECAR_EXTS {
            let candidate = dir.join(format!("{stem}.{ext}"));
            if candidate.is_file() {
                return Some(candidate);
            }
        }
    }
    None
}

/// Write `img` into `cache_dir` under a name derived from its content
/// hash, returning the final path. Idempotent: identical bytes map to
/// the same file, and an existing file is left untouched. Writes go
/// through a temp file + rename so a concurrent reader never sees a
/// partial image.
pub fn cache_image(cache_dir: &Path, img: &ImageBytes) -> io::Result<PathBuf> {
    std::fs::create_dir_all(cache_dir)?;
    let mut hasher = XxHash64::with_seed(0);
    hasher.write(&img.data);
    let name = format!("{:016x}.{}", hasher.finish(), img.ext);
    let final_path = cache_dir.join(&name);
    if final_path.is_file() {
        return Ok(final_path);
    }
    let tmp = cache_dir.join(format!(".{name}.{}.tmp", std::process::id()));
    std::fs::write(&tmp, &img.data)?;
    match std::fs::rename(&tmp, &final_path) {
        Ok(()) => Ok(final_path),
        Err(e) => {
            let _ = std::fs::remove_file(&tmp);
            // Lost a race to another writer with identical bytes.
            if final_path.is_file() {
                Ok(final_path)
            } else {
                Err(e)
            }
        }
    }
}

/// Resolve artwork for an album given its tracks' file paths, probing
/// at most [`MAX_PROBES_PER_ALBUM`] files. Embedded art wins over a
/// sidecar for the same file; the first hit in track order is used.
/// Returns the cached image path, or None when nothing was found.
pub fn resolve_for_files(cache_dir: &Path, track_paths: &[PathBuf]) -> io::Result<Option<PathBuf>> {
    for path in track_paths.iter().take(MAX_PROBES_PER_ALBUM) {
        if !path.is_file() {
            continue;
        }
        if let Some(img) = extract_embedded(path) {
            return cache_image(cache_dir, &img).map(Some);
        }
        if let Some(sidecar) = find_sidecar(path) {
            let Ok(data) = std::fs::read(&sidecar) else {
                continue;
            };
            let ext = ext_from_magic(&data).or_else(|| {
                match sidecar.extension().and_then(|e| e.to_str()) {
                    Some("png") => Some("png"),
                    Some("jpg") | Some("jpeg") => Some("jpg"),
                    _ => None,
                }
            });
            if let Some(ext) = ext {
                return cache_image(cache_dir, &ImageBytes { data, ext }).map(Some);
            }
        }
    }
    Ok(None)
}

#[cfg(test)]
mod tests {
    use super::*;
    use lofty::config::WriteOptions;
    use lofty::picture::Picture;
    use lofty::tag::{Tag, TagExt, TagType};

    const PNG: &[u8] = &[
        0x89, b'P', b'N', b'G', 0x0D, 0x0A, 0x1A, 0x0A, 0, 0, 0, 0x0D, b'I', b'H', b'D', b'R',
    ];
    const JPG: &[u8] = &[0xFF, 0xD8, 0xFF, 0xE0, 0x00, 0x10, b'J', b'F', b'I', b'F'];

    fn write_minimal_wav(path: &Path) {
        // 44-byte header + two silent 8-bit mono samples at 8 kHz. The
        // data chunk is even-sized so a tag chunk appended after it
        // stays RIFF-aligned (odd chunks need a pad byte).
        let bytes: &[u8] = &[
            b'R', b'I', b'F', b'F', 0x26, 0, 0, 0, b'W', b'A', b'V', b'E', b'f', b'm', b't', b' ',
            0x10, 0, 0, 0, 0x01, 0, 0x01, 0, 0x40, 0x1f, 0, 0, 0x40, 0x1f, 0, 0, 0x01, 0, 0x08, 0,
            b'd', b'a', b't', b'a', 0x02, 0, 0, 0, 0x80, 0x80,
        ];
        std::fs::write(path, bytes).unwrap();
    }

    fn write_wav_with_cover(path: &Path, pic_type: PictureType, data: &[u8]) {
        write_minimal_wav(path);
        let mut tag = Tag::new(TagType::Id3v2);
        tag.push_picture(Picture::new_unchecked(pic_type, None, None, data.to_vec()));
        tag.save_to_path(path, WriteOptions::default()).unwrap();
    }

    #[test]
    fn ext_detection_prefers_magic_bytes_over_mime() {
        assert_eq!(ext_from_magic(JPG), Some("jpg"));
        assert_eq!(ext_from_magic(PNG), Some("png"));
        assert_eq!(ext_from_magic(b"nope"), None);
        assert_eq!(ext_for_mime(Some(&MimeType::Jpeg)), Some("jpg"));
        assert_eq!(ext_for_mime(Some(&MimeType::Unknown("x/y".into()))), None);
        assert_eq!(ext_for_mime(None), None);
    }

    #[test]
    fn extract_embedded_prefers_front_cover() {
        let dir = tempfile::tempdir().unwrap();
        let wav = dir.path().join("t.wav");
        write_minimal_wav(&wav);
        let mut tag = Tag::new(TagType::Id3v2);
        tag.push_picture(Picture::new_unchecked(
            PictureType::CoverBack,
            None,
            None,
            PNG.to_vec(),
        ));
        tag.push_picture(Picture::new_unchecked(
            PictureType::CoverFront,
            None,
            None,
            JPG.to_vec(),
        ));
        tag.save_to_path(&wav, WriteOptions::default()).unwrap();

        let img = extract_embedded(&wav).expect("cover");
        assert_eq!(img.ext, "jpg");
        assert_eq!(img.data, JPG);
    }

    #[test]
    fn extract_embedded_falls_back_to_any_picture_and_none_without() {
        let dir = tempfile::tempdir().unwrap();
        let wav = dir.path().join("t.wav");
        write_wav_with_cover(&wav, PictureType::Other, PNG);
        assert_eq!(extract_embedded(&wav).unwrap().ext, "png");

        let bare = dir.path().join("bare.wav");
        write_minimal_wav(&bare);
        assert!(extract_embedded(&bare).is_none());
        assert!(extract_embedded(&dir.path().join("missing.wav")).is_none());
    }

    #[test]
    fn find_sidecar_checks_conventional_names() {
        let dir = tempfile::tempdir().unwrap();
        let track = dir.path().join("01 song.mp3");
        std::fs::write(&track, b"").unwrap();
        assert!(find_sidecar(&track).is_none());
        std::fs::write(dir.path().join("folder.png"), PNG).unwrap();
        assert_eq!(find_sidecar(&track).unwrap(), dir.path().join("folder.png"));
        // `cover.*` outranks `folder.*`.
        std::fs::write(dir.path().join("cover.jpg"), JPG).unwrap();
        assert_eq!(find_sidecar(&track).unwrap(), dir.path().join("cover.jpg"));
    }

    #[test]
    fn cache_image_is_content_addressed_and_idempotent() {
        let dir = tempfile::tempdir().unwrap();
        let cache = dir.path().join("artwork");
        let img = ImageBytes {
            data: JPG.to_vec(),
            ext: "jpg",
        };
        let a = cache_image(&cache, &img).unwrap();
        let b = cache_image(&cache, &img).unwrap();
        assert_eq!(a, b);
        assert!(a.starts_with(&cache));
        assert_eq!(a.extension().unwrap(), "jpg");
        assert_eq!(std::fs::read(&a).unwrap(), JPG);
        let other = cache_image(
            &cache,
            &ImageBytes {
                data: PNG.to_vec(),
                ext: "png",
            },
        )
        .unwrap();
        assert_ne!(a, other);
        // No temp files left behind.
        let leftovers: Vec<_> = std::fs::read_dir(&cache)
            .unwrap()
            .filter_map(|e| e.ok())
            .filter(|e| e.file_name().to_string_lossy().ends_with(".tmp"))
            .collect();
        assert!(leftovers.is_empty());
    }

    #[test]
    fn resolve_for_files_uses_first_hit_and_sidecar_fallback() {
        let dir = tempfile::tempdir().unwrap();
        let cache = dir.path().join("artwork");
        let bare = dir.path().join("01.wav");
        write_minimal_wav(&bare);
        let with_art = dir.path().join("02.wav");
        write_wav_with_cover(&with_art, PictureType::CoverFront, JPG);

        let hit = resolve_for_files(
            &cache,
            &[dir.path().join("missing.wav"), bare.clone(), with_art],
        )
        .unwrap()
        .expect("embedded art from second file");
        assert_eq!(std::fs::read(hit).unwrap(), JPG);

        // Only a bare file: nothing.
        assert!(resolve_for_files(&cache, std::slice::from_ref(&bare))
            .unwrap()
            .is_none());

        // Sidecar next to the bare file gets copied into the cache.
        std::fs::write(dir.path().join("cover.png"), PNG).unwrap();
        let side = resolve_for_files(&cache, &[bare])
            .unwrap()
            .expect("sidecar");
        assert!(side.starts_with(&cache));
        assert_eq!(std::fs::read(side).unwrap(), PNG);
    }

    #[test]
    fn resolve_for_files_skips_unreadable_sidecar_and_falls_through() {
        use std::os::unix::fs::PermissionsExt;

        let dir = tempfile::tempdir().unwrap();
        let cache = dir.path().join("artwork");

        // Track 1: a bare file next to an unreadable `cover.jpg`.
        let track1 = dir.path().join("01.wav");
        write_minimal_wav(&track1);
        let sidecar = dir.path().join("cover.jpg");
        std::fs::write(&sidecar, JPG).unwrap();
        std::fs::set_permissions(&sidecar, std::fs::Permissions::from_mode(0o000)).unwrap();
        let unreadable = std::fs::read(&sidecar).is_err();

        // Track 2: has embedded art.
        let track2 = dir.path().join("02.wav");
        write_wav_with_cover(&track2, PictureType::CoverFront, PNG);

        let result = resolve_for_files(&cache, &[track1, track2]);

        // Restore permissions so tempdir cleanup can remove the file.
        std::fs::set_permissions(&sidecar, std::fs::Permissions::from_mode(0o644)).unwrap();

        if !unreadable {
            // Running as root (or on a fs that ignores mode bits):
            // the sidecar read succeeds after all, so either art is a
            // valid outcome — just don't assert on which.
            return;
        }

        let hit = result.unwrap().expect("falls through to track 2's art");
        assert_eq!(std::fs::read(hit).unwrap(), PNG);
    }

    #[test]
    fn resolve_for_files_stops_after_probe_budget() {
        let dir = tempfile::tempdir().unwrap();
        let cache = dir.path().join("artwork");
        let mut paths = Vec::new();
        for i in 0..MAX_PROBES_PER_ALBUM {
            let p = dir.path().join(format!("{i}.wav"));
            write_minimal_wav(&p);
            paths.push(p);
        }
        let late = dir.path().join("late.wav");
        write_wav_with_cover(&late, PictureType::CoverFront, JPG);
        paths.push(late);
        assert!(resolve_for_files(&cache, &paths).unwrap().is_none());
    }
}
