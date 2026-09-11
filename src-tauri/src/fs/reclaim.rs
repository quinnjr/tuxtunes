//! "Reclaim space" pass: trash the source files that copy-on-add and
//! the consolidate pass left behind.
//!
//! Copying into the managed library — rather than moving — is the safe
//! default, but it doubles the disk a library occupies. Once the copy
//! is known good, the original is redundant.
//!
//! "Known good" is checked per file, not assumed: the managed copy and
//! the original must both exist, be different files, and hash
//! identically. Anything else is left alone. Originals go to the
//! system trash, never `unlink`, so a surprise is recoverable.

use crate::db::preferences;
use crate::fs::events::{ReclaimComplete, ReclaimProgress, RECLAIM_COMPLETE, RECLAIM_PROGRESS};
use crate::fs::hash;
use crate::fs::path;
use prax_query::filter::FilterValue;
use prax_sqlite::raw::SqliteRawEngine;
use std::path::{Path, PathBuf};
use tauri::{AppHandle, Emitter, Runtime};

/// Tracks between progress emits.
const PROGRESS_EVERY: u64 = 25;

#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct ReclaimStats {
    pub reclaimed: u64,
    pub bytes_freed: u64,
    pub skipped: u64,
    pub failed: u64,
}

/// One row's `original_path` and the managed file it was copied to.
struct Candidate {
    id: i64,
    managed: PathBuf,
    original: PathBuf,
}

/// How many originals are waiting to be reclaimed, and how much they
/// occupy. Cheap enough to run before showing the button, so the user
/// is told what it would do before doing it.
pub async fn pending(engine: &SqliteRawEngine) -> Result<(u64, u64), anyhow::Error> {
    let mut count = 0u64;
    let mut bytes = 0u64;
    for cand in candidates(engine).await? {
        if let Ok(meta) = std::fs::metadata(&cand.original) {
            count += 1;
            bytes += meta.len();
        }
    }
    Ok((count, bytes))
}

/// Trash every reclaimable original without UI progress events —
/// the headless half of [`reclaim_all`]. `tuxtunes-cli reclaim` runs
/// this directly so both paths share one verification policy.
pub async fn reclaim_all_headless(engine: &SqliteRawEngine) -> Result<ReclaimStats, anyhow::Error> {
    run_reclaim(engine, |_, _| {}).await
}

pub async fn reclaim_all<R: Runtime>(
    engine: &SqliteRawEngine,
    app: &AppHandle<R>,
) -> Result<ReclaimStats, anyhow::Error> {
    let stats = run_reclaim(engine, |current, total| {
        let _ = app.emit(RECLAIM_PROGRESS, ReclaimProgress { current, total });
    })
    .await?;

    let _ = app.emit(
        RECLAIM_COMPLETE,
        ReclaimComplete {
            reclaimed: stats.reclaimed,
            bytes_freed: stats.bytes_freed,
            skipped: stats.skipped,
            failed: stats.failed,
        },
    );
    Ok(stats)
}

/// The shared walk: every recorded original gets one `reclaim_one`
/// verdict, with a progress hook the GUI feeds and headless callers
/// ignore.
async fn run_reclaim(
    engine: &SqliteRawEngine,
    mut on_progress: impl FnMut(u64, u64),
) -> Result<ReclaimStats, anyhow::Error> {
    let root = preferences::get_library_root(engine).await?;
    let cands = candidates(engine).await?;
    let total = cands.len() as u64;

    let mut stats = ReclaimStats::default();
    for (seen, cand) in cands.into_iter().enumerate() {
        if (seen as u64).is_multiple_of(PROGRESS_EVERY) {
            on_progress(seen as u64, total);
        }
        reclaim_one(engine, &cand, &root, &mut stats).await;
    }
    Ok(stats)
}

/// Every row that records where its file was copied from.
async fn candidates(engine: &SqliteRawEngine) -> Result<Vec<Candidate>, anyhow::Error> {
    let rows = engine
        .raw_sql_query(
            "SELECT id, file_path, original_path FROM tracks \
             WHERE original_path IS NOT NULL AND original_path <> file_path \
             ORDER BY id",
            &[],
        )
        .await?;
    Ok(rows
        .into_iter()
        .filter_map(|row| {
            let json = row.into_json();
            let text = |k: &str| json.get(k).and_then(|v| v.as_str()).map(str::to_owned);
            Some(Candidate {
                id: json.get("id").and_then(|v| v.as_i64())?,
                managed: PathBuf::from(text("file_path")?),
                original: PathBuf::from(text("original_path")?),
            })
        })
        .collect())
}

async fn reclaim_one(
    engine: &SqliteRawEngine,
    cand: &Candidate,
    root: &Path,
    stats: &mut ReclaimStats,
) {
    // Nothing to reclaim if the original is already gone — clear the
    // column so the row stops being offered. A clear that itself fails
    // counts as failed: the row will be re-offered on every run until
    // the bookkeeping lands.
    let Ok(meta) = std::fs::metadata(&cand.original) else {
        if let Err(e) = forget_original(engine, cand.id).await {
            log::warn!(
                "reclaim: could not clear original_path for {}: {e}",
                cand.id
            );
            stats.failed += 1;
        }
        return;
    };

    // The copy has to be a copy: under the managed root, present, and
    // not the same file the row calls its original.
    if !path::is_under(root, &cand.managed)
        || !cand.managed.is_file()
        || path::same_file(&cand.managed, &cand.original)
    {
        stats.skipped += 1;
        return;
    }

    // Same size is a cheap disqualifier before hashing gigabytes.
    let managed_len = std::fs::metadata(&cand.managed).map(|m| m.len()).ok();
    if managed_len != Some(meta.len()) {
        stats.skipped += 1;
        return;
    }

    let hashes = tokio::task::spawn_blocking({
        let managed = cand.managed.clone();
        let original = cand.original.clone();
        move || (hash::hash_file(&managed), hash::hash_file(&original))
    })
    .await;
    let Ok((Ok(managed_hash), Ok(original_hash))) = hashes else {
        stats.failed += 1;
        return;
    };
    if managed_hash != original_hash {
        // The copy has diverged — tags written back to it, say. Not
        // ours to decide which one the user wants.
        stats.skipped += 1;
        return;
    }

    let path_str = cand.original.display().to_string();
    match trash::delete(&cand.original) {
        Ok(()) => {
            stats.reclaimed += 1;
            stats.bytes_freed += meta.len();
            if let Err(e) = forget_original(engine, cand.id).await {
                // The bytes are already trashed; what failed is the
                // bookkeeping, which re-offers the (now missing) row
                // next run until the clear lands.
                log::warn!(
                    "reclaim: could not clear original_path for {}: {e}",
                    cand.id
                );
                stats.failed += 1;
            }
        }
        Err(e) => {
            log::warn!("reclaim: could not trash {path_str}: {e}");
            stats.failed += 1;
        }
    }
}

/// Clear `original_path` so a second run does not re-offer the row.
async fn forget_original(engine: &SqliteRawEngine, track_id: i64) -> anyhow::Result<()> {
    engine
        .raw_sql_execute(
            "UPDATE tracks SET original_path = NULL WHERE id = ?",
            &[FilterValue::Int(track_id)],
        )
        .await
        .map(|_| ())
        .map_err(anyhow::Error::from)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::Db;

    /// A tiny synthetic WAV that lofty will happily parse.
    fn write_minimal_wav(path: &Path) {
        let header: &[u8] = &[
            b'R', b'I', b'F', b'F', 0x25, 0x00, 0x00, 0x00, b'W', b'A', b'V', b'E', b'f', b'm',
            b't', b' ', 0x10, 0x00, 0x00, 0x00, 0x01, 0x00, 0x01, 0x00, 0x40, 0x1f, 0x00, 0x00,
            0x40, 0x1f, 0x00, 0x00, 0x01, 0x00, 0x08, 0x00, b'd', b'a', b't', b'a', 0x01, 0x00,
            0x00, 0x00, 0x80,
        ];
        std::fs::write(path, header).unwrap();
    }

    async fn db_with_root(tmp: &std::path::Path) -> (Db, PathBuf, PathBuf) {
        let db = Db::open(&tmp.join("t.db")).await.unwrap();
        let root = tmp.join("managed");
        crate::db::preferences::set_library_root(&db.engine, &root)
            .await
            .unwrap();
        let incoming = tmp.join("incoming");
        std::fs::create_dir_all(&incoming).unwrap();
        (db, root, incoming)
    }

    /// Register a row whose managed copy is `managed` and whose
    /// recorded original is `original`.
    async fn add_copied_track(db: &Db, managed: &Path, original: &Path) -> i64 {
        let id = crate::library::ingest::probe_and_add(&db.engine, original)
            .await
            .unwrap();
        crate::db::tracks::set_file_paths(
            &db.engine,
            id,
            &managed.display().to_string(),
            Some(&original.display().to_string()),
            "00",
            None,
        )
        .await
        .unwrap();
        id
    }

    #[tokio::test]
    async fn reclaim_all_headless_trashes_a_verified_original() {
        let tmp = tempfile::tempdir().unwrap();
        let (db, root, incoming) = db_with_root(tmp.path()).await;

        let original = incoming.join("a.wav");
        write_minimal_wav(&original);
        let managed = root.join("a.wav");
        std::fs::create_dir_all(&root).unwrap();
        std::fs::copy(&original, &managed).unwrap();
        let wav_len = std::fs::metadata(&original).unwrap().len();
        add_copied_track(&db, &managed, &original).await;

        let stats = reclaim_all_headless(&db.engine).await.unwrap();
        assert_eq!(stats.reclaimed, 1);
        assert_eq!(stats.bytes_freed, wav_len);
        assert_eq!(stats.skipped, 0);
        assert_eq!(stats.failed, 0);
        assert!(!original.exists(), "original must be trashed");
        assert!(managed.is_file(), "managed copy must survive");

        // A second run finds nothing to do.
        let again = reclaim_all_headless(&db.engine).await.unwrap();
        assert_eq!(again, ReclaimStats::default());
    }

    #[tokio::test]
    async fn reclaim_all_headless_skips_a_diverged_copy() {
        let tmp = tempfile::tempdir().unwrap();
        let (db, root, incoming) = db_with_root(tmp.path()).await;

        let original = incoming.join("b.wav");
        write_minimal_wav(&original);
        let managed = root.join("b.wav");
        std::fs::create_dir_all(&root).unwrap();
        std::fs::copy(&original, &managed).unwrap();
        // Same size, different bytes: the copy has diverged.
        let mut bytes = std::fs::read(&managed).unwrap();
        let last = bytes.len() - 1;
        bytes[last] ^= 0xFF;
        std::fs::write(&managed, bytes).unwrap();
        add_copied_track(&db, &managed, &original).await;

        let stats = reclaim_all_headless(&db.engine).await.unwrap();
        assert_eq!(stats.reclaimed, 0);
        assert_eq!(stats.skipped, 1);
        assert_eq!(stats.failed, 0);
        assert!(original.is_file(), "diverged original must be kept");
    }

    #[tokio::test]
    async fn reclaim_all_headless_forgets_a_missing_original() {
        let tmp = tempfile::tempdir().unwrap();
        let (db, root, incoming) = db_with_root(tmp.path()).await;

        let original = incoming.join("gone.wav");
        let managed = root.join("gone.wav");
        std::fs::create_dir_all(&root).unwrap();
        write_minimal_wav(&managed);
        // Probe a sibling so the row exists, then point it at the
        // managed file with a recorded original that was never there.
        let sibling = incoming.join("sibling.wav");
        write_minimal_wav(&sibling);
        let id = crate::library::ingest::probe_and_add(&db.engine, &sibling)
            .await
            .unwrap();
        crate::db::tracks::set_file_paths(
            &db.engine,
            id,
            &managed.display().to_string(),
            Some(&original.display().to_string()),
            "00",
            None,
        )
        .await
        .unwrap();

        let stats = reclaim_all_headless(&db.engine).await.unwrap();
        assert_eq!(stats, ReclaimStats::default());
        let (count, _) = pending(&db.engine).await.unwrap();
        assert_eq!(count, 0, "missing original must stop being offered");
    }

    #[tokio::test]
    async fn reclaim_all_headless_skips_a_size_mismatch() {
        let tmp = tempfile::tempdir().unwrap();
        let (db, root, incoming) = db_with_root(tmp.path()).await;

        let original = incoming.join("c.wav");
        write_minimal_wav(&original);
        let managed = root.join("c.wav");
        std::fs::create_dir_all(&root).unwrap();
        std::fs::copy(&original, &managed).unwrap();
        // Different size: the cheap precheck skips before hashing.
        let mut bytes = std::fs::read(&managed).unwrap();
        bytes.push(0x00);
        std::fs::write(&managed, bytes).unwrap();
        add_copied_track(&db, &managed, &original).await;

        let stats = reclaim_all_headless(&db.engine).await.unwrap();
        assert_eq!(stats.reclaimed, 0);
        assert_eq!(stats.skipped, 1);
        assert_eq!(stats.failed, 0);
        assert!(original.is_file(), "size-mismatched original must be kept");
    }

    #[tokio::test]
    async fn reclaim_all_headless_skips_a_copy_outside_the_root() {
        let tmp = tempfile::tempdir().unwrap();
        let (db, _root, incoming) = db_with_root(tmp.path()).await;

        let original = incoming.join("d.wav");
        write_minimal_wav(&original);
        // Identical bytes, but the "managed" file lives outside the
        // library root: not ours to trash against.
        let elsewhere = tmp.path().join("elsewhere");
        std::fs::create_dir_all(&elsewhere).unwrap();
        let managed = elsewhere.join("d.wav");
        std::fs::copy(&original, &managed).unwrap();
        add_copied_track(&db, &managed, &original).await;

        let stats = reclaim_all_headless(&db.engine).await.unwrap();
        assert_eq!(stats.reclaimed, 0);
        assert_eq!(stats.skipped, 1);
        assert_eq!(stats.failed, 0);
        assert!(original.is_file() && managed.is_file());
    }

    #[tokio::test]
    async fn reclaim_all_headless_skips_same_file_original() {
        let tmp = tempfile::tempdir().unwrap();
        let (db, root, incoming) = db_with_root(tmp.path()).await;

        // The "original" is a symlink to the managed file: distinct
        // path strings that canonicalize alike. Trashing it would
        // delete the managed file itself.
        let managed = root.join("e.wav");
        std::fs::create_dir_all(&root).unwrap();
        write_minimal_wav(&managed);
        let original = incoming.join("e.wav");
        std::os::unix::fs::symlink(&managed, &original).unwrap();
        add_copied_track(&db, &managed, &original).await;

        let stats = reclaim_all_headless(&db.engine).await.unwrap();
        assert_eq!(stats.reclaimed, 0);
        assert_eq!(stats.skipped, 1);
        assert_eq!(stats.failed, 0);
        assert!(
            managed.is_file(),
            "managed file must survive its own original"
        );
    }

    /// chmod-based failure injection below assumes a non-root user (the
    /// developer workstation and CI); root ignores permission bits.
    #[tokio::test]
    async fn reclaim_all_headless_counts_a_hash_failure() {
        use std::os::unix::fs::PermissionsExt;

        let tmp = tempfile::tempdir().unwrap();
        let (db, root, incoming) = db_with_root(tmp.path()).await;

        let original = incoming.join("f.wav");
        write_minimal_wav(&original);
        let managed = root.join("f.wav");
        std::fs::create_dir_all(&root).unwrap();
        std::fs::copy(&original, &managed).unwrap();
        add_copied_track(&db, &managed, &original).await;

        // Unreadable managed copy: present and sized, but unhashable.
        std::fs::set_permissions(&managed, std::fs::Permissions::from_mode(0o000)).unwrap();
        let stats = reclaim_all_headless(&db.engine).await.unwrap();
        std::fs::set_permissions(&managed, std::fs::Permissions::from_mode(0o644)).unwrap();

        assert_eq!(stats.reclaimed, 0);
        assert_eq!(stats.skipped, 0);
        assert_eq!(stats.failed, 1);
        assert!(original.is_file(), "nothing trashed on hash failure");
    }

    #[tokio::test]
    async fn reclaim_all_headless_counts_a_trash_failure() {
        use std::os::unix::fs::PermissionsExt;

        let tmp = tempfile::tempdir().unwrap();
        let (db, root, incoming) = db_with_root(tmp.path()).await;

        let original = incoming.join("g.wav");
        write_minimal_wav(&original);
        let managed = root.join("g.wav");
        std::fs::create_dir_all(&root).unwrap();
        std::fs::copy(&original, &managed).unwrap();
        add_copied_track(&db, &managed, &original).await;

        // Read-only source directory: the verified bytes cannot be
        // moved to the trash (rename out fails, delete fails).
        std::fs::set_permissions(&incoming, std::fs::Permissions::from_mode(0o555)).unwrap();
        let stats = reclaim_all_headless(&db.engine).await.unwrap();
        std::fs::set_permissions(&incoming, std::fs::Permissions::from_mode(0o755)).unwrap();

        assert_eq!(stats.reclaimed, 0);
        assert_eq!(stats.failed, 1);
        assert!(original.is_file(), "untrashable original must be kept");
    }
}
