//! Backfill `track_number` / `disc_number` from each track's file tags.
//!
//! Rows imported before the probe read these fields — and Vorbis files
//! whose `DISCNUMBER` is the "current/total" form — carry `NULL` numbers
//! even though the file has them. This pass re-reads the tags and fills
//! only the columns still `NULL`: a value already on the row is never
//! overwritten, and a row the user has edited is left alone entirely.
//!
//! It is idempotent and meant to be re-runnable, not a one-shot
//! migration.

use crate::db::sync_util::opt_int;
use crate::library::ingest::{probe_numbers, IngestError};
use prax_query::filter::FilterValue;
use prax_sqlite::raw::SqliteRawEngine;
use std::path::{Path, PathBuf};
use std::time::Duration;

/// How many rows to pull per page. Matches `fs::verify`'s batch.
const PAGE: i64 = 500;

/// A single file probe reads only tags, so anything near this is a hung
/// mount rather than slow I/O. A timeout keeps one bad file from
/// stalling the whole pass.
const PROBE_TIMEOUT: Duration = Duration::from_secs(30);

/// Outcome of [`rescan_numbers_all`].
#[derive(Debug, Default, Clone, Copy)]
pub struct RescanStats {
    /// Rows walked.
    pub total: u64,
    /// Rows that gained at least one number.
    pub updated: u64,
    /// Rows already complete, or whose file carried nothing new.
    pub unchanged: u64,
    /// Rows whose file is not on disk.
    pub missing: u64,
    /// Rows whose file could not be read (probe error, panic, or timeout)
    /// or whose write failed.
    pub failed: u64,
    /// Rows left alone because they changed under the pass, or were
    /// malformed.
    pub skipped: u64,
    /// Rows skipped because the user edited them.
    pub user_edited: u64,
}

/// One row the pass may need to touch, decoded from the page query.
struct Pending {
    id: i64,
    file_path: String,
    track: Option<i64>,
    disc: Option<i64>,
    user_edited: bool,
}

/// Re-probe every track's file and fill the track/disc numbers it is
/// missing. Returns the counts for the caller to report.
pub async fn rescan_numbers_all(engine: &SqliteRawEngine) -> anyhow::Result<RescanStats> {
    let total: i64 = engine
        .raw_sql_scalar("SELECT COUNT(*) FROM tracks", &[])
        .await?;
    let mut stats = RescanStats {
        total: total.max(0) as u64,
        ..Default::default()
    };

    // Keyset pagination rather than OFFSET: rows deleted under the pass
    // cannot shift the window and make it skip a track. `id` is the
    // INTEGER PRIMARY KEY, so the cursor always advances.
    let mut last_id = 0i64;
    'pages: loop {
        // Only the columns this pass needs. It reads the table directly
        // rather than through `tracks::list` because `user_edited` is not
        // on `TrackRow`/`TRACK_ROW_COLUMNS`.
        let rows = engine
            .raw_sql_query(
                "SELECT id, file_path, track_number, disc_number, user_edited FROM tracks \
                 WHERE id > ?1 ORDER BY id LIMIT ?2",
                &[FilterValue::Int(last_id), FilterValue::Int(PAGE)],
            )
            .await?;
        if rows.is_empty() {
            break;
        }
        for row in rows {
            let json = row.into_json();
            // `id` is the primary key, so a missing one is a can't-happen:
            // stop rather than loop on the same page forever.
            let Some(id) = json.get("id").and_then(|v| v.as_i64()) else {
                log::warn!("rescan: stopping at a row with no id");
                stats.skipped += 1;
                break 'pages;
            };
            last_id = id;
            let Some(file_path) = json.get("file_path").and_then(|v| v.as_str()) else {
                log::warn!("rescan: track {id} has no file_path; left alone");
                stats.skipped += 1;
                continue;
            };
            let pending = Pending {
                id,
                file_path: file_path.to_string(),
                track: json.get("track_number").and_then(|v| v.as_i64()),
                disc: json.get("disc_number").and_then(|v| v.as_i64()),
                user_edited: json
                    .get("user_edited")
                    .and_then(|v| v.as_i64())
                    .unwrap_or(0)
                    != 0,
            };
            rescan_one(engine, &mut stats, pending).await?;
        }
    }
    Ok(stats)
}

async fn rescan_one(
    engine: &SqliteRawEngine,
    stats: &mut RescanStats,
    row: Pending,
) -> anyhow::Result<()> {
    rescan_one_with(engine, stats, row, probe_numbers).await
}

/// Body of [`rescan_one`], with the probe injectable so a panicking
/// reader can be tested without a file that actually panics lofty.
async fn rescan_one_with<F>(
    engine: &SqliteRawEngine,
    stats: &mut RescanStats,
    row: Pending,
    probe: F,
) -> anyhow::Result<()>
where
    F: FnOnce(&Path) -> Result<(Option<i64>, Option<i64>), IngestError> + Send + 'static,
{
    if row.user_edited {
        stats.user_edited += 1;
        return Ok(());
    }
    if row.track.is_some() && row.disc.is_some() {
        stats.unchanged += 1;
        return Ok(());
    }
    let path = PathBuf::from(&row.file_path);
    if !path.is_file() {
        stats.missing += 1;
        return Ok(());
    }
    let attempt = tokio::time::timeout(
        PROBE_TIMEOUT,
        tokio::task::spawn_blocking(move || probe(&path)),
    )
    .await;
    let probed = match attempt {
        Ok(Ok(result)) => result,
        Ok(Err(e)) => {
            // The reader panicked on this file. One bad file must not
            // abort the whole backfill.
            log::warn!("rescan: reader panicked for {}: {e}", row.file_path);
            stats.failed += 1;
            return Ok(());
        }
        Err(_) => {
            log::warn!("rescan: timed out reading {}", row.file_path);
            stats.failed += 1;
            return Ok(());
        }
    };
    let (found_track, found_disc) = match probed {
        Ok(numbers) => numbers,
        Err(e) => {
            log::warn!("rescan: could not read {}: {e}", row.file_path);
            stats.failed += 1;
            return Ok(());
        }
    };
    let new_track = row.track.or(found_track);
    let new_disc = row.disc.or(found_disc);
    if new_track == row.track && new_disc == row.disc {
        stats.unchanged += 1;
        return Ok(());
    }
    let changed = match engine
        .raw_sql_execute(
            "UPDATE tracks SET track_number = ?, disc_number = ?, \
             date_modified = CURRENT_TIMESTAMP WHERE id = ? AND user_edited = 0",
            &[
                opt_int(new_track),
                opt_int(new_disc),
                FilterValue::Int(row.id),
            ],
        )
        .await
    {
        Ok(n) => n,
        Err(e) => {
            // One row's write failing must not discard the whole pass.
            log::warn!("rescan: write failed for track {}: {e}", row.id);
            stats.failed += 1;
            return Ok(());
        }
    };
    if changed == 0 {
        // Deleted, or edited, between the read and the write.
        log::warn!(
            "rescan: track {} changed under the pass; left alone",
            row.id
        );
        stats.skipped += 1;
    } else {
        stats.updated += 1;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::Db;
    use crate::library::test_support::{tagged_wav, write_minimal_flac, write_taggable_wav};

    async fn insert_row(
        engine: &SqliteRawEngine,
        file_path: &str,
        track: Option<i64>,
        disc: Option<i64>,
        user_edited: bool,
    ) -> i64 {
        engine
            .raw_sql_scalar::<i64>(
                "INSERT INTO tracks (title, duration_ms, size_bytes, file_path, \
                 track_number, disc_number, user_edited, playlist_ids) \
                 VALUES ('row', 1000, 0, ?, ?, ?, ?, '[]') RETURNING id",
                &[
                    FilterValue::String(file_path.to_string()),
                    track.map(FilterValue::Int).unwrap_or(FilterValue::Null),
                    disc.map(FilterValue::Int).unwrap_or(FilterValue::Null),
                    FilterValue::Int(i64::from(user_edited)),
                ],
            )
            .await
            .unwrap()
    }

    async fn numbers(engine: &SqliteRawEngine, id: i64) -> (Option<i64>, Option<i64>) {
        let row = engine
            .raw_sql_first(
                "SELECT track_number, disc_number FROM tracks WHERE id = ?1",
                &[FilterValue::Int(id)],
            )
            .await
            .unwrap()
            .into_json();
        (
            row.get("track_number").and_then(|v| v.as_i64()),
            row.get("disc_number").and_then(|v| v.as_i64()),
        )
    }

    /// Every row the walk saw must land in exactly one bucket.
    fn assert_buckets_sum(stats: &RescanStats) {
        assert_eq!(
            stats.total,
            stats.updated
                + stats.unchanged
                + stats.missing
                + stats.failed
                + stats.skipped
                + stats.user_edited,
            "row buckets do not add up to the total: {stats:?}"
        );
    }

    #[tokio::test]
    async fn fills_null_numbers_and_keeps_values_already_set() {
        let tmp = tempfile::tempdir().unwrap();
        let db = Db::open(&tmp.path().join("t.db")).await.unwrap();
        let wav = tmp.path().join("a.wav");
        tagged_wav(&wav);
        let wav2 = tmp.path().join("b.wav");
        tagged_wav(&wav2);

        let empty = insert_row(&db.engine, &wav.display().to_string(), None, None, false).await;
        // Track already known (7) but disc missing: only the disc is filled.
        let partial = insert_row(
            &db.engine,
            &wav2.display().to_string(),
            Some(7),
            None,
            false,
        )
        .await;

        let stats = rescan_numbers_all(&db.engine).await.unwrap();
        assert_buckets_sum(&stats);
        assert_eq!(stats.total, 2);
        assert_eq!(stats.updated, 2);
        assert_eq!(stats.unchanged, 0);
        assert_eq!(stats.missing, 0);
        assert_eq!(stats.failed, 0);

        assert_eq!(numbers(&db.engine, empty).await, (Some(1), Some(2)));
        assert_eq!(numbers(&db.engine, partial).await, (Some(7), Some(2)));
    }

    /// The bug this feature exists for: a Vorbis file whose `DISCNUMBER`
    /// is the "current/total" form, backfilled from the file.
    #[tokio::test]
    async fn fills_a_vorbis_pair_form_disc_number() {
        let tmp = tempfile::tempdir().unwrap();
        let db = Db::open(&tmp.path().join("t.db")).await.unwrap();
        let flac = tmp.path().join("pair.flac");
        write_minimal_flac(
            &flac,
            &["TITLE=Minimal", "TRACKNUMBER=5/12", "DISCNUMBER=2/3"],
        );

        let id = insert_row(&db.engine, &flac.display().to_string(), None, None, false).await;

        let stats = rescan_numbers_all(&db.engine).await.unwrap();
        assert_buckets_sum(&stats);
        assert_eq!(stats.updated, 1);
        assert_eq!(numbers(&db.engine, id).await, (Some(5), Some(2)));
    }

    #[tokio::test]
    async fn skips_user_edited_rows_and_missing_files() {
        let tmp = tempfile::tempdir().unwrap();
        let db = Db::open(&tmp.path().join("t.db")).await.unwrap();
        let wav = tmp.path().join("a.wav");
        tagged_wav(&wav);

        let edited = insert_row(&db.engine, &wav.display().to_string(), None, None, true).await;
        let gone = insert_row(&db.engine, "/no/such/file.flac", None, None, false).await;

        let stats = rescan_numbers_all(&db.engine).await.unwrap();
        assert_buckets_sum(&stats);
        assert_eq!(stats.total, 2);
        assert_eq!(stats.user_edited, 1);
        assert_eq!(stats.missing, 1);
        assert_eq!(stats.updated, 0);

        assert_eq!(numbers(&db.engine, edited).await, (None, None));
        assert_eq!(numbers(&db.engine, gone).await, (None, None));
    }

    #[tokio::test]
    async fn leaves_a_complete_row_alone() {
        let tmp = tempfile::tempdir().unwrap();
        let db = Db::open(&tmp.path().join("t.db")).await.unwrap();
        let wav = tmp.path().join("a.wav");
        tagged_wav(&wav);

        let id = insert_row(
            &db.engine,
            &wav.display().to_string(),
            Some(9),
            Some(4),
            false,
        )
        .await;

        let stats = rescan_numbers_all(&db.engine).await.unwrap();
        assert_buckets_sum(&stats);
        assert_eq!(stats.total, 1);
        assert_eq!(stats.unchanged, 1);
        assert_eq!(stats.updated, 0);
        assert_eq!(numbers(&db.engine, id).await, (Some(9), Some(4)));
    }

    /// A readable file whose tags carry no numbers: the pass must not
    /// count it `updated` nor stamp `date_modified` on a no-op write.
    #[tokio::test]
    async fn leaves_a_readable_file_with_no_numbers_unchanged() {
        let tmp = tempfile::tempdir().unwrap();
        let db = Db::open(&tmp.path().join("t.db")).await.unwrap();
        let wav = tmp.path().join("plain.wav");
        write_taggable_wav(&wav);

        let id = insert_row(&db.engine, &wav.display().to_string(), None, None, false).await;

        let stats = rescan_numbers_all(&db.engine).await.unwrap();
        assert_buckets_sum(&stats);
        assert_eq!(stats.unchanged, 1);
        assert_eq!(stats.updated, 0);
        assert_eq!(stats.failed, 0);
        assert_eq!(numbers(&db.engine, id).await, (None, None));
    }

    /// A file that exists but lofty cannot parse is counted `failed` — the
    /// signal the CLI turns into a non-zero exit.
    #[tokio::test]
    async fn counts_unreadable_files_as_failed() {
        let tmp = tempfile::tempdir().unwrap();
        let db = Db::open(&tmp.path().join("t.db")).await.unwrap();
        let broken = tmp.path().join("broken.flac");
        std::fs::write(&broken, b"not actually audio").unwrap();

        let id = insert_row(&db.engine, &broken.display().to_string(), None, None, false).await;

        let stats = rescan_numbers_all(&db.engine).await.unwrap();
        assert_buckets_sum(&stats);
        assert_eq!(stats.total, 1);
        assert_eq!(stats.failed, 1);
        assert_eq!(stats.updated, 0);
        assert_eq!(stats.missing, 0);
        assert_eq!(numbers(&db.engine, id).await, (None, None));
    }

    /// A reader panic must be counted, not abort the pass.
    #[tokio::test]
    async fn counts_a_panicking_reader_as_failed() {
        let tmp = tempfile::tempdir().unwrap();
        let db = Db::open(&tmp.path().join("t.db")).await.unwrap();
        let wav = tmp.path().join("a.wav");
        tagged_wav(&wav);
        let id = insert_row(&db.engine, &wav.display().to_string(), None, None, false).await;

        let mut stats = RescanStats::default();
        rescan_one_with(
            &db.engine,
            &mut stats,
            Pending {
                id,
                file_path: wav.display().to_string(),
                track: None,
                disc: None,
                user_edited: false,
            },
            |_path| panic!("boom"),
        )
        .await
        .unwrap();

        assert_eq!(stats.failed, 1);
        assert_eq!(stats.updated, 0);
        assert_eq!(numbers(&db.engine, id).await, (None, None));
    }

    /// More rows than one page: the walk must reach the rows after the
    /// first `PAGE`, or the backfill silently truncates a real library.
    #[tokio::test]
    async fn pages_through_more_than_one_batch() {
        let tmp = tempfile::tempdir().unwrap();
        let db = Db::open(&tmp.path().join("t.db")).await.unwrap();

        for i in 0..PAGE {
            insert_row(&db.engine, &format!("/no/such/{i}.flac"), None, None, false).await;
        }
        // Ordered by id, this row lands on the second page.
        let wav = tmp.path().join("last.wav");
        tagged_wav(&wav);
        let last = insert_row(&db.engine, &wav.display().to_string(), None, None, false).await;

        let stats = rescan_numbers_all(&db.engine).await.unwrap();
        assert_buckets_sum(&stats);
        assert_eq!(stats.total, PAGE as u64 + 1);
        assert_eq!(stats.missing, PAGE as u64);
        assert_eq!(stats.updated, 1);
        assert_eq!(numbers(&db.engine, last).await, (Some(1), Some(2)));
    }

    /// The row is edited between the read and the write; the SQL guard
    /// must reject the update and the pass must not claim it as updated.
    #[tokio::test]
    async fn a_row_edited_under_the_pass_is_left_alone() {
        let tmp = tempfile::tempdir().unwrap();
        let db = Db::open(&tmp.path().join("t.db")).await.unwrap();
        let wav = tmp.path().join("a.wav");
        tagged_wav(&wav);
        let id = insert_row(&db.engine, &wav.display().to_string(), None, None, false).await;

        // Simulate the concurrent edit: call `rescan_one` with the stale
        // `user_edited = false` the earlier read would have seen.
        db.engine
            .raw_sql_execute(
                "UPDATE tracks SET user_edited = 1 WHERE id = ?",
                &[FilterValue::Int(id)],
            )
            .await
            .unwrap();
        let mut stats = RescanStats::default();
        rescan_one(
            &db.engine,
            &mut stats,
            Pending {
                id,
                file_path: wav.display().to_string(),
                track: None,
                disc: None,
                user_edited: false,
            },
        )
        .await
        .unwrap();

        assert_eq!(stats.skipped, 1);
        assert_eq!(stats.failed, 0);
        assert_eq!(stats.updated, 0);
        assert_eq!(numbers(&db.engine, id).await, (None, None));
    }
}
