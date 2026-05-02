//! "Verify Library" walk. Re-hashes every managed track, flags
//! mismatches as `missing_source`, refreshes `verified_at` for all.

use crate::db::tracks::{self, TrackRow};
use crate::fs::events::{VerifyComplete, VerifyProgress, VERIFY_COMPLETE, VERIFY_PROGRESS};
use crate::fs::hash;
use prax_sqlite::raw::SqliteRawEngine;
use std::path::Path;
use tauri::{AppHandle, Emitter, Runtime};

const PAGE: i64 = 500;

#[derive(Debug, Default, Clone, Copy)]
pub struct VerifyStats {
    pub total: u64,
    pub verified: u64,
    pub missing: u64,
    pub mismatched: u64,
}

pub async fn verify_all<R: Runtime>(
    engine: &SqliteRawEngine,
    app: &AppHandle<R>,
) -> Result<VerifyStats, anyhow::Error> {
    let total: i64 = engine
        .raw_sql_scalar("SELECT COUNT(*) FROM tracks", &[])
        .await?;
    let total = total.max(0) as u64;

    let mut stats = VerifyStats {
        total,
        ..Default::default()
    };
    let mut offset = 0i64;
    loop {
        let batch = tracks::list(engine, PAGE, offset, &Default::default(), None).await?;
        if batch.is_empty() {
            break;
        }
        for (i, row) in batch.iter().enumerate() {
            let seen = (offset as u64) + (i as u64);
            if seen.is_multiple_of(50) {
                let _ = app.emit(
                    VERIFY_PROGRESS,
                    VerifyProgress {
                        current: seen,
                        total,
                    },
                );
            }
            verify_one(engine, row, &mut stats).await?;
        }
        offset += batch.len() as i64;
    }

    let _ = app.emit(
        VERIFY_COMPLETE,
        VerifyComplete {
            total: stats.total,
            verified: stats.verified,
            missing: stats.missing,
            mismatched: stats.mismatched,
        },
    );
    Ok(stats)
}

async fn verify_one(
    engine: &SqliteRawEngine,
    row: &TrackRow,
    stats: &mut VerifyStats,
) -> Result<(), anyhow::Error> {
    let path = Path::new(&row.file_path);
    if !path.exists() {
        tracks::mark_missing_source(engine, row.id).await?;
        stats.missing += 1;
        return Ok(());
    }
    let fresh = match tokio::task::spawn_blocking({
        let p = path.to_path_buf();
        move || hash::hash_file(&p)
    })
    .await?
    {
        Ok(h) => h,
        Err(_) => {
            tracks::mark_missing_source(engine, row.id).await?;
            stats.missing += 1;
            return Ok(());
        }
    };
    let fresh_hex = hash::hash_hex(fresh);

    // A stored hash that doesn't match fresh content = file was modified
    // out-of-band. Every other case (matching hash, no prior hash) is a
    // successful verify.
    let mismatch = row
        .file_hash
        .as_deref()
        .is_some_and(|stored| stored != fresh_hex);
    if mismatch {
        tracks::mark_missing_source(engine, row.id).await?;
        stats.mismatched += 1;
    } else {
        tracks::set_file_hash(engine, row.id, &fresh_hex).await?;
        stats.verified += 1;
    }
    Ok(())
}
