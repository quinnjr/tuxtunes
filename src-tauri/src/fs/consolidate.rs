//! "Consolidate library" pass: bring every track to the path its
//! `organize_scheme` asks for under the library root.
//!
//! Per track, one of three things happens:
//!   * it is already there → left alone;
//!   * it lives elsewhere under the root → renamed into place by the
//!     organize worker's `organize_one`;
//!   * it lives outside the root → copied in by `ingest_one`, which
//!     leaves the original where it is (same contract as copy-on-add).
//!
//! Runs on the ingest worker's queue, so it can never race the
//! copy-on-add work it shares a table with.

use crate::db::preferences;
use crate::db::tracks::{self, TrackRow};
use crate::fs::events::{
    ConsolidateComplete, ConsolidateProgress, CONSOLIDATE_COMPLETE, CONSOLIDATE_PROGRESS,
};
use crate::fs::path::{self, render, TrackFields};
use prax_sqlite::raw::SqliteRawEngine;
use std::path::Path;
use tauri::{AppHandle, Emitter, Runtime};

/// Rows per query. Matches the verify walk.
const PAGE: i64 = 500;

/// Tracks between progress emits. A 51K-track library would otherwise
/// push 51K events at the webview.
const PROGRESS_EVERY: u64 = 25;

#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct ConsolidateStats {
    pub total: u64,
    pub moved: u64,
    pub copied: u64,
    pub in_place: u64,
    pub failed: u64,
}

pub async fn consolidate_all<R: Runtime>(
    engine: &SqliteRawEngine,
    app: &AppHandle<R>,
) -> Result<ConsolidateStats, anyhow::Error> {
    let total: i64 = engine
        .raw_sql_scalar("SELECT COUNT(*) FROM tracks", &[])
        .await?;
    let total = total.max(0) as u64;

    let root = preferences::get_library_root(engine).await?;
    let scheme = preferences::get_organize_scheme(engine).await?;

    let mut stats = ConsolidateStats {
        total,
        ..Default::default()
    };

    // Paged by offset like the verify walk. Rows that move are still
    // ordered the same way (the sort is not on `file_path`), so the
    // page window stays stable as paths change underneath it.
    let mut offset = 0i64;
    loop {
        let batch = tracks::list(engine, PAGE, offset, &Default::default(), None).await?;
        if batch.is_empty() {
            break;
        }
        for (i, row) in batch.iter().enumerate() {
            let seen = (offset as u64) + (i as u64);
            if seen.is_multiple_of(PROGRESS_EVERY) {
                let _ = app.emit(
                    CONSOLIDATE_PROGRESS,
                    ConsolidateProgress {
                        current: seen,
                        total,
                    },
                );
            }
            consolidate_one(engine, app, row, &root, &scheme, &mut stats).await;
        }
        offset += batch.len() as i64;
    }

    let _ = app.emit(
        CONSOLIDATE_COMPLETE,
        ConsolidateComplete {
            total: stats.total,
            moved: stats.moved,
            copied: stats.copied,
            in_place: stats.in_place,
            failed: stats.failed,
        },
    );
    Ok(stats)
}

/// One track's share of the walk. Per-track failures are counted, not
/// propagated — one unreadable file must not abandon the other 50,000.
async fn consolidate_one<R: Runtime>(
    engine: &SqliteRawEngine,
    app: &AppHandle<R>,
    row: &TrackRow,
    root: &Path,
    scheme: &str,
    stats: &mut ConsolidateStats,
) {
    let current = Path::new(&row.file_path);

    let rel = match render(scheme, &TrackFields::from_track_row(row, current)) {
        Ok(rel) => rel,
        Err(e) => {
            log::warn!(
                "consolidate: cannot render a path for track {}: {e}",
                row.id
            );
            stats.failed += 1;
            return;
        }
    };

    if path::same_file(&root.join(&rel), current) {
        stats.in_place += 1;
        return;
    }

    // Inside the root, just at the wrong path: a rename, which keeps
    // the library one copy of each file. Anything outside is copied,
    // leaving the user's original alone.
    let (result, counter) = if path::is_under(root, current) {
        (
            crate::fs::organize::organize_one(engine, app, row.id).await,
            &mut stats.moved,
        )
    } else {
        (
            crate::fs::ingest::ingest_one(engine, app, row.id, current).await,
            &mut stats.copied,
        )
    };

    match result {
        Ok(()) => *counter += 1,
        Err(e) => {
            log::warn!("consolidate: track {} left where it is: {e}", row.id);
            stats.failed += 1;
        }
    }
}
