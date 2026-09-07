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

/// Tracks between progress emits. A 51K-track library would otherwise
/// push 51K events at the webview.
const PROGRESS_EVERY: u64 = 25;

#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct ConsolidateStats {
    pub total: u64,
    pub moved: u64,
    pub copied: u64,
    pub in_place: u64,
    /// Rows whose file is not on disk. An import can carry in
    /// thousands of these — iTunes collision-suffix entries for files
    /// that were already gone — and calling them failures is alarming
    /// and useless.
    pub missing: u64,
    pub failed: u64,
}

pub async fn consolidate_all<R: Runtime>(
    engine: &SqliteRawEngine,
    app: &AppHandle<R>,
) -> Result<ConsolidateStats, anyhow::Error> {
    // The full id list up front, rather than paging by OFFSET: the pass
    // runs for minutes while adds and deletes continue from other
    // tasks, and an OFFSET window over `date_added DESC` shifts under
    // every insert — skipping a row per insert and re-visiting one per
    // delete. Ids are cheap (8 bytes each, ~400 KB at 51K tracks) and
    // fix the work set at the moment the user asked for it.
    let ids: Vec<i64> = engine
        .raw_sql_query("SELECT id FROM tracks ORDER BY id", &[])
        .await?
        .into_iter()
        .filter_map(|row| row.into_json().get("id").and_then(|v| v.as_i64()))
        .collect();
    let total = ids.len() as u64;

    let root = preferences::get_library_root(engine).await?;
    let scheme = preferences::get_organize_scheme(engine).await?;

    let mut stats = ConsolidateStats {
        total,
        ..Default::default()
    };

    for (seen, id) in ids.into_iter().enumerate() {
        if (seen as u64).is_multiple_of(PROGRESS_EVERY) {
            let _ = app.emit(
                CONSOLIDATE_PROGRESS,
                ConsolidateProgress {
                    current: seen as u64,
                    total,
                },
            );
        }
        // Re-read each row as it comes up: the pass is slow enough that
        // a metadata edit mid-walk should be reflected in the path we
        // render, and a row deleted in the meantime is simply skipped.
        match tracks::get(engine, id).await {
            Ok(row) => consolidate_one(engine, app, &row, &root, &scheme, &mut stats).await,
            Err(_) => stats.total -= 1,
        }
    }

    let _ = app.emit(
        CONSOLIDATE_COMPLETE,
        ConsolidateComplete {
            total: stats.total,
            moved: stats.moved,
            copied: stats.copied,
            in_place: stats.in_place,
            missing: stats.missing,
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

    // A row whose file is not there has nothing to consolidate. This
    // is the common shape of an imported library, not an error.
    if !current.is_file() {
        stats.missing += 1;
        return;
    }

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
