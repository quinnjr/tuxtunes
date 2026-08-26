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
    /// Entries whose ` N`-suffixed file was gone but whose base file
    /// existed and was unclaimed, now pointed at that file.
    pub relinked: u64,
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
            relinked: stats.relinked,
        },
    );
    Ok(stats)
}

async fn verify_one(
    engine: &SqliteRawEngine,
    row: &TrackRow,
    stats: &mut VerifyStats,
) -> Result<(), anyhow::Error> {
    let mut path = Path::new(&row.file_path).to_path_buf();
    if !path.exists() {
        // iTunes collision-suffix entry whose base file survived? Take
        // it over unless another row already owns that file (then the
        // sync's dedup pass is the right place to merge the two).
        match crate::fs::relink::dedupe_suffix_candidate(&path) {
            Some(base) if !tracks::path_in_use(engine, &base.to_string_lossy()).await? => {
                tracks::relink_file_path(engine, row.id, &base.to_string_lossy()).await?;
                stats.relinked += 1;
                path = base;
            }
            _ => {
                tracks::mark_missing_source(engine, row.id).await?;
                stats.missing += 1;
                return Ok(());
            }
        }
    }
    let path = path.as_path();
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::Db;
    use prax_query::filter::FilterValue as FV;

    async fn insert(engine: &SqliteRawEngine, title: &str, path: &Path) -> i64 {
        engine
            .raw_sql_scalar(
                "INSERT INTO tracks (title, duration_ms, size_bytes, file_path, playlist_ids) \
                 VALUES (?, 1000, 0, ?, '[]') RETURNING id",
                &[
                    FV::String(title.to_string()),
                    FV::String(path.to_string_lossy().into_owned()),
                ],
            )
            .await
            .unwrap()
    }

    async fn row(engine: &SqliteRawEngine, id: i64) -> (String, String) {
        let t = tracks::get(engine, id).await.unwrap();
        (t.file_path, t.import_status)
    }

    #[tokio::test]
    async fn verify_relinks_suffixed_entry_when_base_is_free_and_marks_missing_otherwise() {
        let dir = tempfile::tempdir().unwrap();
        let db = Db::open(&dir.path().join("t.db")).await.unwrap();
        let app = tauri::test::mock_app();

        // Base file exists, only the " 4" entry references it → relink.
        let base_a = dir.path().join("01 Song.mp3");
        std::fs::write(&base_a, b"audio").unwrap();
        let relink_id = insert(&db.engine, "relink", &dir.path().join("01 Song 4.mp3")).await;

        // Base file exists but its own row is present → the suffixed
        // row stays missing (sync merges those).
        let base_b = dir.path().join("02 Other.mp3");
        std::fs::write(&base_b, b"audio2").unwrap();
        let owner_id = insert(&db.engine, "owner", &base_b).await;
        let dupe_id = insert(&db.engine, "dupe", &dir.path().join("02 Other 2.mp3")).await;

        // Plain missing file.
        let gone_id = insert(&db.engine, "gone", &dir.path().join("03 Gone.mp3")).await;

        let stats = verify_all(&db.engine, app.handle()).await.unwrap();
        assert_eq!(stats.total, 4);
        assert_eq!(stats.relinked, 1);
        assert_eq!(stats.missing, 2);
        assert_eq!(stats.verified, 2, "owner + relinked row both hash clean");

        let (p, st) = row(&db.engine, relink_id).await;
        assert_eq!(Path::new(&p), base_a.as_path());
        assert_eq!(st, "ok");
        assert_eq!(row(&db.engine, owner_id).await.1, "ok");
        assert_eq!(row(&db.engine, dupe_id).await.1, "missing_source");
        assert_eq!(row(&db.engine, gone_id).await.1, "missing_source");
    }
}
