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

pub async fn reclaim_all<R: Runtime>(
    engine: &SqliteRawEngine,
    app: &AppHandle<R>,
) -> Result<ReclaimStats, anyhow::Error> {
    let root = preferences::get_library_root(engine).await?;
    let cands = candidates(engine).await?;
    let total = cands.len() as u64;

    let mut stats = ReclaimStats::default();
    for (seen, cand) in cands.into_iter().enumerate() {
        if (seen as u64).is_multiple_of(PROGRESS_EVERY) {
            let _ = app.emit(
                RECLAIM_PROGRESS,
                ReclaimProgress {
                    current: seen as u64,
                    total,
                },
            );
        }
        reclaim_one(engine, &cand, &root, &mut stats).await;
    }

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
    // column so the row stops being offered.
    let Ok(meta) = std::fs::metadata(&cand.original) else {
        forget_original(engine, cand.id).await;
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
            forget_original(engine, cand.id).await;
        }
        Err(e) => {
            log::warn!("reclaim: could not trash {path_str}: {e}");
            stats.failed += 1;
        }
    }
}

/// Clear `original_path` so a second run does not re-offer the row.
async fn forget_original(engine: &SqliteRawEngine, track_id: i64) {
    if let Err(e) = engine
        .raw_sql_execute(
            "UPDATE tracks SET original_path = NULL WHERE id = ?",
            &[FilterValue::Int(track_id)],
        )
        .await
    {
        log::warn!("reclaim: could not clear original_path for {track_id}: {e}");
    }
}
