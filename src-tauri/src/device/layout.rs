//! Rendering a track's path on the device.
//!
//! Reuses the library's template vocabulary from [`crate::fs::path`],
//! then applies a stricter per-segment sanitiser: devices are usually
//! FAT32 or exFAT, which reject characters and name shapes that ext4
//! accepts happily, and MTP stacks truncate long names without warning.

use super::transport::{Capabilities, DevicePath};
use crate::fs::path::{PathRenderError, TrackFields};

/// Characters FAT and Windows reject outright. `/` and `\` are handled
/// separately, becoming `-` so an "AC/DC" artist stays readable.
const ILLEGAL: &[char] = &[':', '*', '?', '"', '<', '>', '|'];

/// Device names that are reserved on Windows regardless of extension.
const RESERVED: &[&str] = &[
    "CON", "PRN", "AUX", "NUL", "COM1", "COM2", "COM3", "COM4", "COM5", "COM6", "COM7", "COM8",
    "COM9", "LPT1", "LPT2", "LPT3", "LPT4", "LPT5", "LPT6", "LPT7", "LPT8", "LPT9",
];

#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum LayoutError {
    #[error(transparent)]
    Render(#[from] PathRenderError),
    #[error("template rendered to an empty path")]
    Empty,
}

/// Sanitise one path segment for `max_bytes` on a device filesystem.
///
/// Order matters: separators become dashes before illegal characters
/// are dropped, so `AC/DC` survives as `AC-DC`; trailing dots and
/// spaces go last, because stripping an illegal character can expose a
/// new one.
pub fn sanitize_segment(raw: &str, max_bytes: usize) -> String {
    let mut out = String::with_capacity(raw.len());
    let mut last_was_space = false;
    for ch in raw.chars() {
        let mapped = match ch {
            '/' | '\\' => '-',
            c if c.is_control() => continue,
            c if ILLEGAL.contains(&c) => continue,
            c => c,
        };
        let is_space = mapped.is_whitespace();
        if is_space && last_was_space {
            continue;
        }
        out.push(mapped);
        last_was_space = is_space;
    }

    let mut s = out
        .trim_matches(|c: char| c == '.' || c.is_whitespace())
        .to_string();

    if s.is_empty() {
        return "_".to_string();
    }

    // A reserved name is reserved with or without an extension, so test
    // the stem rather than the whole segment.
    let stem = s.split('.').next().unwrap_or(&s);
    if RESERVED.iter().any(|r| r.eq_ignore_ascii_case(stem)) {
        s.push('_');
    }

    truncate_bytes(&s, max_bytes)
}

/// Truncate to at most `max_bytes`, never splitting a character.
fn truncate_bytes(s: &str, max_bytes: usize) -> String {
    if s.len() <= max_bytes {
        return s.to_string();
    }
    let mut end = max_bytes;
    while end > 0 && !s.is_char_boundary(end) {
        end -= 1;
    }
    // Re-trim: the cut can expose a trailing dot or space, which FAT
    // would reject just as surely as the original.
    s[..end]
        .trim_end_matches(|c: char| c == '.' || c.is_whitespace())
        .to_string()
}

/// Render `template` for `t` and anchor the result under `root`.
///
/// Every segment is sanitised individually, so no field value can
/// introduce a separator or a `..` and escape `root`.
pub fn render(
    template: &str,
    root: &DevicePath,
    t: &TrackFields<'_>,
    caps: &Capabilities,
) -> Result<DevicePath, LayoutError> {
    let expanded = crate::fs::path::expand_tokens(template, t)?;
    let mut out = root.clone();
    let mut any = false;
    for segment in expanded.split('/') {
        let clean = sanitize_segment(segment, caps.max_path_bytes);
        // `sanitize_segment` never returns empty, but an empty input
        // segment (a `//` in the template) should still be skipped.
        if segment.trim().is_empty() {
            continue;
        }
        out = out.join(&clean);
        any = true;
    }
    if !any {
        return Err(LayoutError::Empty);
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::device::transport::FilesystemKind;

    fn caps() -> Capabilities {
        Capabilities {
            playlist_objects: false,
            free_space: true,
            rename: true,
            max_path_bytes: 255,
            filesystem: FilesystemKind::ExFat,
        }
    }

    fn fields() -> TrackFields<'static> {
        TrackFields {
            title: "Kerala",
            artist: Some("Bonobo"),
            album_artist: Some("Bonobo"),
            album: Some("Migration"),
            genre: None,
            track_number: Some(2),
            track_count: None,
            disc_number: Some(1),
            disc_count: Some(2),
            year: None,
            ext: "flac",
            fallback_stem: "",
        }
    }

    #[test]
    fn strips_fat_illegal_characters() {
        assert_eq!(
            sanitize_segment(r#"AC/DC: Back? "In" Black|"#, 255),
            "AC-DC Back In Black"
        );
    }

    #[test]
    fn strips_trailing_dots_and_spaces() {
        assert_eq!(sanitize_segment("Album. ", 255), "Album");
        assert_eq!(sanitize_segment("...", 255), "_");
        assert_eq!(sanitize_segment("   ", 255), "_");
    }

    #[test]
    fn suffixes_reserved_dos_names() {
        assert_eq!(sanitize_segment("CON", 255), "CON_");
        assert_eq!(sanitize_segment("com1", 255), "com1_");
        assert_eq!(sanitize_segment("NUL.flac", 255), "NUL.flac_");
        assert_eq!(
            sanitize_segment("CONCERT", 255),
            "CONCERT",
            "a longer name that merely starts with a reserved word is fine"
        );
    }

    #[test]
    fn truncates_on_a_utf8_boundary() {
        let s = sanitize_segment(&"é".repeat(200), 255);
        assert!(s.len() <= 255);
        assert!(s.is_char_boundary(s.len()));
        assert_eq!(s.chars().count(), 127, "255 bytes holds 127 two-byte chars");
    }

    #[test]
    fn truncation_does_not_leave_a_trailing_dot() {
        // Cutting at 5 bytes would land right after the dot.
        assert_eq!(sanitize_segment("abcd.efg", 5), "abcd");
    }

    #[test]
    fn render_places_a_track_under_the_root() {
        let got = render(
            "{album_artist}/{album}/{disc:02}-{track:02} {title}.{ext}",
            &DevicePath::new("/Music"),
            &fields(),
            &caps(),
        )
        .unwrap();
        assert_eq!(
            got.as_str(),
            "/Music/Bonobo/Migration/01-02 Kerala.flac"
        );
    }

    #[test]
    fn render_omits_disc_for_single_disc_albums() {
        let mut f = fields();
        f.disc_count = Some(1);
        f.disc_number = Some(1);
        let got = render(
            "{album_artist}/{album}/{disc:02}-{track:02} {title}.{ext}",
            &DevicePath::new("/Music"),
            &f,
            &caps(),
        )
        .unwrap();
        assert_eq!(got.as_str(), "/Music/Bonobo/Migration/02 Kerala.flac");
    }

    #[test]
    fn render_never_escapes_the_root() {
        let mut f = fields();
        f.album_artist = Some("../../etc");
        let got = render(
            "{album_artist}/{title}.{ext}",
            &DevicePath::new("/Music"),
            &f,
            &caps(),
        )
        .unwrap();
        assert!(got.as_str().starts_with("/Music/"));
        // The separators in the field value collapse into the segment
        // itself, so it can never act as a parent reference.
        assert!(
            got.as_str()
                .split('/')
                .all(|s| s != ".." && s != "."),
            "no segment may be a parent reference: {}",
            got.as_str()
        );
        assert_eq!(got.as_str(), "/Music/-..-etc/Kerala.flac");
    }

    #[test]
    fn a_field_that_is_only_dots_collapses_away() {
        let mut f = fields();
        f.album_artist = Some("..");
        let got = render(
            "{album_artist}/{title}.{ext}",
            &DevicePath::new("/Music"),
            &f,
            &caps(),
        )
        .unwrap();
        assert_eq!(
            got.as_str(),
            "/Music/Kerala.flac",
            "an all-dots field yields no directory at all, rather than a junk one"
        );
    }

    #[test]
    fn render_honours_the_device_name_length_cap() {
        let mut caps = caps();
        caps.max_path_bytes = 8;
        let got = render("{album}/{title}.{ext}", &DevicePath::new("/M"), &fields(), &caps)
            .unwrap();
        for segment in got.as_str().split('/').filter(|s| !s.is_empty()) {
            assert!(segment.len() <= 8, "segment {segment:?} exceeds the cap");
        }
    }

    #[test]
    fn render_rejects_an_empty_template() {
        let err = render("", &DevicePath::new("/Music"), &fields(), &caps()).unwrap_err();
        assert_eq!(err, LayoutError::Empty);
    }
}
