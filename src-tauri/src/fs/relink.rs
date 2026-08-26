//! Recover tracks whose file path carries an iTunes collision suffix.
//!
//! When iTunes copies a file whose name already exists it appends a
//! counter — `01 Song 1.mp3`, `01 Song 2.mp3` — and the library keeps
//! separate entries for each. After the media folder is consolidated
//! or copied to another machine those suffixed files are frequently
//! gone while the base `01 Song.mp3` survives, leaving thousands of
//! playlist entries pointing at nothing. This module finds that base
//! file so callers can relink (or merge) the entry instead of marking
//! it missing.

use std::path::{Path, PathBuf};

/// If `path` doesn't exist but a sibling without a trailing ` N`
/// counter (1–2 digits) does, return that sibling. Returns None when
/// `path` itself exists, has no such suffix, or the sibling is absent.
pub fn dedupe_suffix_candidate(path: &Path) -> Option<PathBuf> {
    if path.exists() {
        return None;
    }
    let stem = path.file_stem()?.to_str()?;
    let ext = path.extension()?.to_str()?;
    let base = strip_counter(stem)?;
    let candidate = path.with_file_name(format!("{base}.{ext}"));
    candidate.is_file().then_some(candidate)
}

/// `"01 Song 4"` → `Some("01 Song")`; `"01 Song"` → None; `"Track 12"`
/// → `Some("Track")`. Requires a single space before 1–2 digits, so
/// `"Symphony No. 9"` (dot before digit) and years like `"Live 1999"`
/// (four digits) are left alone.
fn strip_counter(stem: &str) -> Option<&str> {
    let (base, tail) = stem.rsplit_once(' ')?;
    if base.is_empty() || tail.is_empty() || tail.len() > 2 {
        return None;
    }
    if !tail.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    // A dot right before the number ("Symphony No. 9") is a title, not
    // a collision counter; so is a doubled space.
    if base.ends_with(' ') || base.ends_with('.') {
        return None;
    }
    Some(base)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn strip_counter_rules() {
        assert_eq!(strip_counter("01 Song 4"), Some("01 Song"));
        assert_eq!(strip_counter("Track 12"), Some("Track"));
        assert_eq!(strip_counter("01 Song"), None);
        assert_eq!(strip_counter("Live 1999"), None);
        assert_eq!(strip_counter("Symphony No. 9"), None);
        assert_eq!(strip_counter("4"), None);
        assert_eq!(strip_counter("Song  4"), None);
    }

    #[test]
    fn candidate_only_when_missing_and_sibling_exists() {
        let dir = tempfile::tempdir().unwrap();
        let base = dir.path().join("01 Song.mp3");
        let dupe = dir.path().join("01 Song 4.mp3");
        // Neither exists yet.
        assert_eq!(dedupe_suffix_candidate(&dupe), None);
        std::fs::write(&base, b"x").unwrap();
        assert_eq!(dedupe_suffix_candidate(&dupe), Some(base.clone()));
        // The suffixed file itself exists → nothing to relink.
        std::fs::write(&dupe, b"y").unwrap();
        assert_eq!(dedupe_suffix_candidate(&dupe), None);
        // Plain missing file without a counter → None.
        assert_eq!(
            dedupe_suffix_candidate(&dir.path().join("02 Other.mp3")),
            None
        );
        // Directory sibling doesn't count.
        std::fs::create_dir(dir.path().join("Folder.m4a")).unwrap();
        assert_eq!(
            dedupe_suffix_candidate(&dir.path().join("Folder 1.m4a")),
            None
        );
    }
}
