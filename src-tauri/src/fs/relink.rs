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

/// Tolerance either side of the expected duration, in milliseconds.
/// Generous enough for encoder/container rounding, tight enough that a
/// different recording of the same nominal length is the only false
/// positive left.
const DURATION_TOLERANCE_MS: i64 = 2000;

/// Relative tolerance on file size, used only when no duration is known.
const SIZE_TOLERANCE_FRAC: f64 = 0.05;

/// Sanity-check a `dedupe_suffix_candidate` before relinking to it.
///
/// `dedupe_suffix_candidate` matches on filename shape alone, so
/// `"Intro 2.mp3"` happily resolves to an unrelated `"Intro.mp3"`, and
/// titles that genuinely end in a small number (`"Sum 41"`) get
/// repointed at whatever else lives next to them. Probe the candidate
/// and accept it only if it plausibly holds the same recording:
///
/// * expected duration known (> 0) → require |Δduration| ≤ 2 s;
/// * else expected size known (> 0) → require it within 5 %;
/// * else accept (the historical behaviour) and log that the relink
///   went unverified.
pub fn candidate_matches(
    candidate: &Path,
    expected_duration_ms: Option<i64>,
    expected_size_bytes: Option<i64>,
) -> bool {
    if let Some(expected) = expected_duration_ms.filter(|d| *d > 0) {
        let Some(actual) = probe_duration_ms(candidate) else {
            // Unreadable or duration-less file: we cannot confirm it is
            // the same recording, so don't repoint the row at it.
            log::info!(
                "relink candidate {} rejected: duration unreadable",
                candidate.display()
            );
            return false;
        };
        let ok = (actual - expected).abs() <= DURATION_TOLERANCE_MS;
        if !ok {
            log::info!(
                "relink candidate {} rejected: {actual} ms vs expected {expected} ms",
                candidate.display()
            );
        }
        return ok;
    }

    if let Some(expected) = expected_size_bytes.filter(|s| *s > 0) {
        let Ok(meta) = std::fs::metadata(candidate) else {
            return false;
        };
        let actual = meta.len() as i64;
        let ok = (actual - expected).abs() as f64 <= expected as f64 * SIZE_TOLERANCE_FRAC;
        if !ok {
            log::info!(
                "relink candidate {} rejected: {actual} bytes vs expected {expected}",
                candidate.display()
            );
        }
        return ok;
    }

    log::info!(
        "relink candidate {} accepted unverified (no expected duration or size)",
        candidate.display()
    );
    true
}

/// Blocking: read `path`'s audio duration with lofty. `None` when the
/// file cannot be parsed or reports a zero duration.
fn probe_duration_ms(path: &Path) -> Option<i64> {
    use lofty::file::AudioFile;
    let tagged = lofty::probe::Probe::open(path).ok()?.read().ok()?;
    let ms = tagged.properties().duration().as_millis() as i64;
    (ms > 0).then_some(ms)
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

    /// 44-byte WAV header + 400 silent 8-bit mono samples at 8 kHz =
    /// exactly 50 ms of audio, which lofty parses happily.
    fn write_wav_50ms(path: &Path) {
        const SAMPLES: u32 = 400;
        let mut bytes = Vec::with_capacity(44 + SAMPLES as usize);
        bytes.extend_from_slice(b"RIFF");
        bytes.extend_from_slice(&(36 + SAMPLES).to_le_bytes());
        bytes.extend_from_slice(b"WAVEfmt ");
        bytes.extend_from_slice(&16u32.to_le_bytes());
        bytes.extend_from_slice(&1u16.to_le_bytes()); // PCM
        bytes.extend_from_slice(&1u16.to_le_bytes()); // mono
        bytes.extend_from_slice(&8000u32.to_le_bytes()); // sample rate
        bytes.extend_from_slice(&8000u32.to_le_bytes()); // byte rate
        bytes.extend_from_slice(&1u16.to_le_bytes()); // block align
        bytes.extend_from_slice(&8u16.to_le_bytes()); // bits per sample
        bytes.extend_from_slice(b"data");
        bytes.extend_from_slice(&SAMPLES.to_le_bytes());
        bytes.extend(std::iter::repeat_n(0x80u8, SAMPLES as usize));
        std::fs::write(path, &bytes).unwrap();
    }

    #[test]
    fn candidate_matches_accepts_matching_duration_rejects_wrong_one() {
        let dir = tempfile::tempdir().unwrap();
        let wav = dir.path().join("01 Song.wav");
        write_wav_50ms(&wav);

        // Same recording (50 ms), well inside the 2 s tolerance.
        assert!(candidate_matches(&wav, Some(50), None));
        assert!(candidate_matches(&wav, Some(1200), None));
        // A minute-long track is definitely not this file.
        assert!(!candidate_matches(&wav, Some(60_000), None));
    }

    #[test]
    fn candidate_matches_falls_back_to_size_then_accepts_unverified() {
        let dir = tempfile::tempdir().unwrap();
        let wav = dir.path().join("01 Song.wav");
        write_wav_50ms(&wav);
        let size = std::fs::metadata(&wav).unwrap().len() as i64;

        // No duration known → size within 5 %.
        assert!(candidate_matches(&wav, None, Some(size)));
        assert!(candidate_matches(&wav, Some(0), Some(size)));
        assert!(!candidate_matches(&wav, None, Some(size * 4)));
        // Neither known → accept (unverified).
        assert!(candidate_matches(&wav, None, None));
        assert!(candidate_matches(&wav, Some(0), Some(0)));
    }

    #[test]
    fn candidate_matches_rejects_unreadable_file_when_duration_expected() {
        let dir = tempfile::tempdir().unwrap();
        let junk = dir.path().join("not-audio.mp3");
        std::fs::write(&junk, b"definitely not audio").unwrap();
        assert!(!candidate_matches(&junk, Some(180_000), None));
        // Missing file, size fallback.
        assert!(!candidate_matches(
            &dir.path().join("gone.mp3"),
            None,
            Some(1024)
        ));
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
