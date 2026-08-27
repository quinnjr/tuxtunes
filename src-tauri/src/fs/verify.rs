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
        // `dedupe_suffix_candidate` matches on filename shape alone, so
        // confirm the candidate actually holds this recording before
        // repointing the row at it (and clearing its hash).
        let candidate = match crate::fs::relink::dedupe_suffix_candidate(&path) {
            Some(base) => {
                let expected = row.duration_ms;
                tokio::task::spawn_blocking({
                    let base = base.clone();
                    move || crate::fs::relink::candidate_matches(&base, Some(expected), None)
                })
                .await?
                .then_some(base)
            }
            None => None,
        };
        match candidate {
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

    /// 44-byte header + 400 silent 8-bit mono samples at 8 kHz = 50 ms,
    /// close enough to the 1000 ms the test rows claim for
    /// `relink::candidate_matches` to accept it.
    fn write_wav(path: &Path, marker: u8) {
        const SAMPLES: u32 = 400;
        let mut bytes = Vec::with_capacity(44 + SAMPLES as usize);
        bytes.extend_from_slice(b"RIFF");
        bytes.extend_from_slice(&(36 + SAMPLES).to_le_bytes());
        bytes.extend_from_slice(b"WAVEfmt ");
        bytes.extend_from_slice(&16u32.to_le_bytes());
        bytes.extend_from_slice(&1u16.to_le_bytes());
        bytes.extend_from_slice(&1u16.to_le_bytes());
        bytes.extend_from_slice(&8000u32.to_le_bytes());
        bytes.extend_from_slice(&8000u32.to_le_bytes());
        bytes.extend_from_slice(&1u16.to_le_bytes());
        bytes.extend_from_slice(&8u16.to_le_bytes());
        bytes.extend_from_slice(b"data");
        bytes.extend_from_slice(&SAMPLES.to_le_bytes());
        bytes.extend(std::iter::repeat_n(marker, SAMPLES as usize));
        std::fs::write(path, &bytes).unwrap();
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
        let base_a = dir.path().join("01 Song.wav");
        write_wav(&base_a, 0x80);
        let relink_id = insert(&db.engine, "relink", &dir.path().join("01 Song 4.wav")).await;

        // Base file exists but its own row is present → the suffixed
        // row stays missing (sync merges those).
        let base_b = dir.path().join("02 Other.wav");
        write_wav(&base_b, 0x40);
        let owner_id = insert(&db.engine, "owner", &base_b).await;
        let dupe_id = insert(&db.engine, "dupe", &dir.path().join("02 Other 2.wav")).await;

        // Plain missing file.
        let gone_id = insert(&db.engine, "gone", &dir.path().join("03 Gone.wav")).await;

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

    #[tokio::test]
    async fn verify_refuses_to_relink_a_candidate_of_a_different_length() {
        let dir = tempfile::tempdir().unwrap();
        let db = Db::open(&dir.path().join("t.db")).await.unwrap();
        let app = tauri::test::mock_app();

        // "Sum 41.wav" is a title, not a collision suffix — the 50 ms
        // "Sum.wav" next to it is a different recording entirely.
        let unrelated = dir.path().join("Sum.wav");
        write_wav(&unrelated, 0x80);
        let id = db
            .engine
            .raw_sql_scalar::<i64>(
                "INSERT INTO tracks (title, duration_ms, size_bytes, file_path, playlist_ids) \
                 VALUES ('Fat Lip', 178000, 0, ?, '[]') RETURNING id",
                &[FV::String(
                    dir.path().join("Sum 41.wav").to_string_lossy().into_owned(),
                )],
            )
            .await
            .unwrap();

        let stats = verify_all(&db.engine, app.handle()).await.unwrap();
        assert_eq!(stats.relinked, 0, "duration mismatch must block the relink");
        assert_eq!(stats.missing, 1);
        let (p, st) = row(&db.engine, id).await;
        assert_eq!(st, "missing_source");
        assert!(p.ends_with("Sum 41.wav"), "path left alone: {p}");
    }
}
