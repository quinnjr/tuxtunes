//! Remap Windows-style paths in an iTunes library to Linux paths.

use percent_encoding::percent_decode_str;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct PathMapping {
    /// Source prefix (e.g., `D:/Users/Joseph/`). Match is case-insensitive
    /// and done after URL-decoding.
    pub from: String,
    /// Replacement (e.g., `/run/media/joseph/Local Disk/Users/Joseph/`).
    pub to: String,
}

#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum PathMapError {
    #[error("unmappable path: {0}")]
    Unmappable(String),
}

/// Normalize one iTunes track path:
/// - Strip the `file://localhost/` prefix if present.
/// - Percent-decode (`%20` → space, etc.).
/// - Convert Windows-style backslashes to forward slashes.
/// - Apply the longest-matching [`PathMapping`] prefix from `mappings`
///   (case-insensitive).
/// - Return the remapped path.
pub fn remap(raw: &str, mappings: &[PathMapping]) -> Result<String, PathMapError> {
    let stripped = strip_file_url(raw);
    let decoded = percent_decode_str(&stripped)
        .decode_utf8_lossy()
        .into_owned();
    let normalized = decoded.replace('\\', "/");

    // Longest-prefix match, case-insensitive.
    let lower = normalized.to_ascii_lowercase();
    let mut best: Option<(&PathMapping, usize)> = None;
    for m in mappings {
        let from_lower = m.from.to_ascii_lowercase();
        if lower.starts_with(&from_lower) {
            let len = from_lower.len();
            if best.map(|(_, l)| l < len).unwrap_or(true) {
                best = Some((m, len));
            }
        }
    }

    match best {
        Some((m, len)) => {
            let suffix = &normalized[len..];
            let mut out = m.to.clone();
            // Normalize joins — avoid `/foo//bar`.
            if out.ends_with('/') && suffix.starts_with('/') {
                out.push_str(&suffix[1..]);
            } else if !out.ends_with('/') && !suffix.starts_with('/') {
                out.push('/');
                out.push_str(suffix);
            } else {
                out.push_str(suffix);
            }
            Ok(out)
        }
        None => Err(PathMapError::Unmappable(normalized)),
    }
}

/// Strip the common `file://` / `file://localhost/` prefix used by iTunes
/// on Windows-stored libraries. Also handles `file:///D:/...`.
fn strip_file_url(s: &str) -> String {
    let trimmed = s
        .strip_prefix("file://localhost/")
        .or_else(|| s.strip_prefix("file:///"))
        .or_else(|| s.strip_prefix("file://"))
        .unwrap_or(s);
    trimmed.to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn mapping(from: &str, to: &str) -> PathMapping {
        PathMapping {
            from: from.into(),
            to: to.into(),
        }
    }

    #[test]
    fn remaps_simple_drive_letter_path() {
        let m = [mapping("D:/", "/mnt/d/")];
        let out = remap("file://localhost/D:/Music/song.flac", &m).unwrap();
        assert_eq!(out, "/mnt/d/Music/song.flac");
    }

    #[test]
    fn percent_decodes_spaces() {
        let m = [mapping("D:/", "/mnt/d/")];
        let out = remap("file://localhost/D:/My%20Music/a%20b.flac", &m).unwrap();
        assert_eq!(out, "/mnt/d/My Music/a b.flac");
    }

    #[test]
    fn case_insensitive_drive_letter() {
        let m = [mapping("d:/", "/mnt/d/")];
        let out = remap("file://localhost/D:/Music/a.flac", &m).unwrap();
        assert_eq!(out, "/mnt/d/Music/a.flac");
    }

    #[test]
    fn longest_prefix_wins() {
        // D:/Users/Joseph/ → /home/joseph (more specific) beats D:/ → /mnt/d
        let m = [
            mapping("D:/", "/mnt/d/"),
            mapping("D:/Users/Joseph/", "/home/joseph/"),
        ];
        let out = remap("file://localhost/D:/Users/Joseph/Music/a.flac", &m).unwrap();
        assert_eq!(out, "/home/joseph/Music/a.flac");
    }

    #[test]
    fn unmappable_returns_error() {
        let m = [mapping("D:/", "/mnt/d/")];
        let err = remap("file://localhost/C:/Windows/file.flac", &m).unwrap_err();
        assert!(matches!(err, PathMapError::Unmappable(_)));
    }

    #[test]
    fn backslashes_normalize_to_forward_slashes() {
        let m = [mapping("D:/", "/mnt/d/")];
        let out = remap(r"file://localhost/D:\Music\a.flac", &m).unwrap();
        assert_eq!(out, "/mnt/d/Music/a.flac");
    }

    #[test]
    fn handles_missing_file_url_prefix() {
        let m = [mapping("D:/", "/mnt/d/")];
        let out = remap("D:/Music/a.flac", &m).unwrap();
        assert_eq!(out, "/mnt/d/Music/a.flac");
    }

    #[test]
    fn no_double_slash_at_join() {
        let m = [mapping("D:", "/mnt/d")];
        let out = remap("D:/Music/a.flac", &m).unwrap();
        assert_eq!(out, "/mnt/d/Music/a.flac");
    }

    #[test]
    fn join_strips_leading_slash_when_target_ends_with_slash() {
        // Both sides have a slash at the join point — must not produce
        // a doubled separator. Exercises the `out.ends_with('/') &&
        // suffix.starts_with('/')` branch.
        let m = [mapping("D:", "/mnt/d/")];
        let out = remap("D:/Music/a.flac", &m).unwrap();
        assert_eq!(out, "/mnt/d/Music/a.flac");
    }

    #[test]
    fn join_inserts_separator_when_neither_side_has_one() {
        // The "from" prefix has no trailing slash and the "suffix" has
        // no leading slash, so remap must insert one. This relies on
        // the `from` matching everything up through `D:` followed
        // immediately by `Music` (no separator).
        let m = [mapping("D:", "/mnt/d")];
        let out = remap("D:Music/a.flac", &m).unwrap();
        assert_eq!(out, "/mnt/d/Music/a.flac");
    }
}
