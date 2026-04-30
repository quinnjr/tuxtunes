//! Minimal query helpers for the `tracks` table.

use prax_sqlite::raw::SqliteRawEngine;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct TrackRow {
    pub id: i64,
    pub title: String,
    pub artist: Option<String>,
    pub album: Option<String>,
    pub duration_ms: i64,
    pub file_path: String,
    pub sample_rate: Option<i64>,
    pub bit_depth: Option<i64>,
    pub kind: Option<String>,
    pub play_count: i64,
    pub skip_count: i64,
}

#[derive(Debug, thiserror::Error)]
pub enum TracksError {
    #[error("query failed: {0}")]
    Query(#[source] anyhow::Error),
}

pub async fn list(
    engine: &SqliteRawEngine,
    limit: i64,
    offset: i64,
) -> Result<Vec<TrackRow>, TracksError> {
    let sql = "SELECT id, title, artist, album, duration_ms, file_path, \
               sample_rate, bit_depth, kind, play_count, skip_count \
               FROM tracks \
               ORDER BY date_added DESC, id DESC \
               LIMIT ? OFFSET ?";
    let params = vec![
        prax_query::filter::FilterValue::Int(limit),
        prax_query::filter::FilterValue::Int(offset),
    ];
    let json_rows = engine
        .raw_sql_query(sql, &params)
        .await
        .map_err(|e| TracksError::Query(anyhow::Error::from(e)))?;
    let rows = json_rows
        .into_iter()
        .map(|r| serde_json::from_value(r.into_json()))
        .collect::<Result<Vec<TrackRow>, _>>()
        .map_err(|e| TracksError::Query(anyhow::Error::from(e)))?;
    Ok(rows)
}

pub async fn get(engine: &SqliteRawEngine, id: i64) -> Result<TrackRow, TracksError> {
    let sql = "SELECT id, title, artist, album, duration_ms, file_path, \
               sample_rate, bit_depth, kind, play_count, skip_count \
               FROM tracks WHERE id = ?";
    let params = vec![prax_query::filter::FilterValue::Int(id)];
    let json_row = engine
        .raw_sql_first(sql, &params)
        .await
        .map_err(|e| TracksError::Query(anyhow::Error::from(e)))?;
    serde_json::from_value(json_row.into_json())
        .map_err(|e| TracksError::Query(anyhow::Error::from(e)))
}

async fn bump_counter(
    engine: &SqliteRawEngine,
    id: i64,
    counter_col: &str,
    timestamp_col: &str,
) -> Result<(), TracksError> {
    let sql = format!(
        "UPDATE tracks SET {counter_col} = {counter_col} + 1, \
         {timestamp_col} = CURRENT_TIMESTAMP WHERE id = ?",
    );
    let params = vec![prax_query::filter::FilterValue::Int(id)];
    engine
        .raw_sql_execute(&sql, &params)
        .await
        .map(|_| ())
        .map_err(|e| TracksError::Query(anyhow::Error::from(e)))
}

pub async fn bump_play_count(engine: &SqliteRawEngine, id: i64) -> Result<(), TracksError> {
    bump_counter(engine, id, "play_count", "last_played").await
}

pub async fn bump_skip_count(engine: &SqliteRawEngine, id: i64) -> Result<(), TracksError> {
    bump_counter(engine, id, "skip_count", "last_skipped").await
}

/// Projection of an iTunes track normalized for insert/update. All
/// fields carry the SOURCE side of a conflict resolution; the caller
/// decides whether to apply each via `sync::conflict::resolve_*`.
#[derive(Debug, Clone, PartialEq)]
pub struct ItlTrackUpsert<'a> {
    pub persistent_id: u64,
    pub sync_source_id: i64,
    pub title: &'a str,
    pub artist: Option<&'a str>,
    pub album: Option<&'a str>,
    pub album_artist: Option<&'a str>,
    pub composer: Option<&'a str>,
    pub genre: Option<&'a str>,
    pub kind: Option<&'a str>,
    pub duration_ms: i64,
    pub size_bytes: i64,
    pub bit_rate: Option<i64>,
    pub sample_rate: Option<i64>,
    pub track_number: Option<i64>,
    pub disc_number: Option<i64>,
    pub year: Option<i64>,
    pub bpm: Option<i64>,
    pub rating: i64,
    pub play_count: i64,
    pub date_added_unix: i64,
    pub file_path: &'a str,
    pub original_path: Option<&'a str>,
}

/// Look up a track by `(sync_source_id, persistent_id)`. Returns the
/// local row's internal id + every user-state field needed for conflict
/// resolution.
#[derive(Debug, Clone)]
pub struct LocalTrackForSync {
    pub id: i64,
    pub rating: i64,
    pub play_count: i64,
    pub skip_count: i64,
    pub last_played: Option<i64>,
    pub last_skipped: Option<i64>,
    pub loved: bool,
    pub original_path: Option<String>,
}

pub async fn by_persistent_id(
    engine: &SqliteRawEngine,
    sync_source_id: i64,
    persistent_id_hex: &str,
) -> Result<Option<LocalTrackForSync>, TracksError> {
    let sql = "SELECT id, rating, play_count, skip_count, last_played, \
               last_skipped, loved, original_path \
               FROM tracks WHERE sync_source_id = ? AND persistent_id = ?";
    let params = vec![
        prax_query::filter::FilterValue::Int(sync_source_id),
        prax_query::filter::FilterValue::String(persistent_id_hex.to_string()),
    ];
    let json_row = match engine.raw_sql_optional(sql, &params).await {
        Ok(Some(r)) => r,
        Ok(None) => return Ok(None),
        Err(e) => return Err(TracksError::Query(anyhow::Error::from(e))),
    };
    let v: serde_json::Value = json_row.into_json();
    Ok(Some(LocalTrackForSync {
        id: v.get("id").and_then(|v| v.as_i64()).unwrap_or(0),
        rating: v.get("rating").and_then(|v| v.as_i64()).unwrap_or(0),
        play_count: v.get("play_count").and_then(|v| v.as_i64()).unwrap_or(0),
        skip_count: v.get("skip_count").and_then(|v| v.as_i64()).unwrap_or(0),
        last_played: v.get("last_played").and_then(|v| v.as_i64()),
        last_skipped: v.get("last_skipped").and_then(|v| v.as_i64()),
        loved: v.get("loved").and_then(|v| v.as_i64()).unwrap_or(0) != 0,
        original_path: v
            .get("original_path")
            .and_then(|v| v.as_str())
            .map(|s| s.to_string()),
    }))
}

/// Insert a new track from an ITL upsert record. Returns the local id.
pub async fn insert_from_itl(
    engine: &SqliteRawEngine,
    t: &ItlTrackUpsert<'_>,
) -> Result<i64, TracksError> {
    let pid_hex = format!("{:016x}", t.persistent_id);
    let sql = "INSERT INTO tracks ( \
        persistent_id, sync_source_id, title, artist, album, album_artist, \
        composer, genre, kind, duration_ms, size_bytes, bit_rate, sample_rate, \
        track_number, disc_number, year, bpm, rating, play_count, \
        date_added, file_path, original_path, playlist_ids) \
        VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, \
                datetime(?, 'unixepoch'), ?, ?, '[]') RETURNING id";
    use prax_query::filter::FilterValue as FV;
    let params = vec![
        FV::String(pid_hex),
        FV::Int(t.sync_source_id),
        FV::String(t.title.to_string()),
        t.artist.map(|s| FV::String(s.into())).unwrap_or(FV::Null),
        t.album.map(|s| FV::String(s.into())).unwrap_or(FV::Null),
        t.album_artist
            .map(|s| FV::String(s.into()))
            .unwrap_or(FV::Null),
        t.composer.map(|s| FV::String(s.into())).unwrap_or(FV::Null),
        t.genre.map(|s| FV::String(s.into())).unwrap_or(FV::Null),
        t.kind.map(|s| FV::String(s.into())).unwrap_or(FV::Null),
        FV::Int(t.duration_ms),
        FV::Int(t.size_bytes),
        t.bit_rate.map(FV::Int).unwrap_or(FV::Null),
        t.sample_rate.map(FV::Int).unwrap_or(FV::Null),
        t.track_number.map(FV::Int).unwrap_or(FV::Null),
        t.disc_number.map(FV::Int).unwrap_or(FV::Null),
        t.year.map(FV::Int).unwrap_or(FV::Null),
        t.bpm.map(FV::Int).unwrap_or(FV::Null),
        FV::Int(t.rating),
        FV::Int(t.play_count),
        FV::Int(t.date_added_unix),
        FV::String(t.file_path.to_string()),
        t.original_path
            .map(|s| FV::String(s.into()))
            .unwrap_or(FV::Null),
    ];
    let json_row = engine
        .raw_sql_first(sql, &params)
        .await
        .map_err(|e| TracksError::Query(anyhow::Error::from(e)))?;
    Ok(json_row
        .into_json()
        .get("id")
        .and_then(|v| v.as_i64())
        .unwrap_or(-1))
}

/// Update an existing track's descriptive fields (called on sync when
/// persistent_id already exists locally). User-state fields (rating,
/// play_count, etc.) should have already been resolved by the caller
/// through `sync::conflict::resolve_*`.
pub async fn update_descriptive_fields(
    engine: &SqliteRawEngine,
    local_id: i64,
    t: &ItlTrackUpsert<'_>,
    resolved_rating: i64,
    resolved_play_count: i64,
    file_path: &str,
) -> Result<(), TracksError> {
    let sql = "UPDATE tracks SET \
        title = ?, artist = ?, album = ?, album_artist = ?, composer = ?, \
        genre = ?, kind = ?, duration_ms = ?, size_bytes = ?, bit_rate = ?, \
        sample_rate = ?, track_number = ?, disc_number = ?, year = ?, bpm = ?, \
        rating = ?, play_count = ?, file_path = ? \
        WHERE id = ?";
    use prax_query::filter::FilterValue as FV;
    let params = vec![
        FV::String(t.title.to_string()),
        t.artist.map(|s| FV::String(s.into())).unwrap_or(FV::Null),
        t.album.map(|s| FV::String(s.into())).unwrap_or(FV::Null),
        t.album_artist
            .map(|s| FV::String(s.into()))
            .unwrap_or(FV::Null),
        t.composer.map(|s| FV::String(s.into())).unwrap_or(FV::Null),
        t.genre.map(|s| FV::String(s.into())).unwrap_or(FV::Null),
        t.kind.map(|s| FV::String(s.into())).unwrap_or(FV::Null),
        FV::Int(t.duration_ms),
        FV::Int(t.size_bytes),
        t.bit_rate.map(FV::Int).unwrap_or(FV::Null),
        t.sample_rate.map(FV::Int).unwrap_or(FV::Null),
        t.track_number.map(FV::Int).unwrap_or(FV::Null),
        t.disc_number.map(FV::Int).unwrap_or(FV::Null),
        t.year.map(FV::Int).unwrap_or(FV::Null),
        t.bpm.map(FV::Int).unwrap_or(FV::Null),
        FV::Int(resolved_rating),
        FV::Int(resolved_play_count),
        FV::String(file_path.to_string()),
        FV::Int(local_id),
    ];
    engine
        .raw_sql_execute(sql, &params)
        .await
        .map(|_| ())
        .map_err(|e| TracksError::Query(anyhow::Error::from(e)))
}

/// Delete tracks in `sync_source_id` whose `persistent_id` is not in `keep`.
pub async fn delete_missing(
    engine: &SqliteRawEngine,
    sync_source_id: i64,
    keep_hex: &[String],
) -> Result<u64, TracksError> {
    if keep_hex.is_empty() {
        let sql = "DELETE FROM tracks WHERE sync_source_id = ?";
        let params = vec![prax_query::filter::FilterValue::Int(sync_source_id)];
        return engine
            .raw_sql_execute(sql, &params)
            .await
            .map_err(|e| TracksError::Query(anyhow::Error::from(e)));
    }

    // Stage the keep set in a temp table, then DELETE by anti-join. This
    // avoids SQLite's per-statement parameter/length limits when the keep
    // set is large (40K+ tracks is typical for iTunes libraries).
    engine
        .raw_sql_execute(
            "CREATE TEMP TABLE IF NOT EXISTS _sync_keep \
             (persistent_id TEXT PRIMARY KEY) WITHOUT ROWID",
            &[],
        )
        .await
        .map_err(|e| TracksError::Query(anyhow::Error::from(e)))?;
    engine
        .raw_sql_execute("DELETE FROM _sync_keep", &[])
        .await
        .map_err(|e| TracksError::Query(anyhow::Error::from(e)))?;
    for hex in keep_hex {
        engine
            .raw_sql_execute(
                "INSERT INTO _sync_keep (persistent_id) VALUES (?)",
                &[prax_query::filter::FilterValue::String(hex.clone())],
            )
            .await
            .map_err(|e| TracksError::Query(anyhow::Error::from(e)))?;
    }
    engine
        .raw_sql_execute(
            "DELETE FROM tracks WHERE sync_source_id = ? \
             AND persistent_id NOT IN (SELECT persistent_id FROM _sync_keep)",
            &[prax_query::filter::FilterValue::Int(sync_source_id)],
        )
        .await
        .map_err(|e| TracksError::Query(anyhow::Error::from(e)))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::Db;

    async fn tmp_db() -> Db {
        let tmp = tempfile::NamedTempFile::new().unwrap();
        Db::open(tmp.path()).await.unwrap()
    }

    async fn insert_fixture(engine: &SqliteRawEngine, title: &str, path: &str) -> i64 {
        let sql = "INSERT INTO tracks (title, duration_ms, size_bytes, file_path, playlist_ids) \
                   VALUES (?, 0, 0, ?, '[]') RETURNING id";
        let params = vec![
            prax_query::filter::FilterValue::String(title.into()),
            prax_query::filter::FilterValue::String(path.into()),
        ];
        let json_row = engine.raw_sql_first(sql, &params).await.unwrap();
        let row: serde_json::Value = json_row.into_json();
        row.get("id").and_then(|v| v.as_i64()).unwrap()
    }

    #[tokio::test]
    async fn list_returns_tracks_newest_first() {
        let db = tmp_db().await;
        let a = insert_fixture(&db.engine, "Alpha", "/tmp/a.flac").await;
        let b = insert_fixture(&db.engine, "Bravo", "/tmp/b.flac").await;
        let rows = list(&db.engine, 10, 0).await.unwrap();
        assert_eq!(rows.len(), 2);
        // newest first — Bravo was inserted second → has the higher id
        assert_eq!(rows[0].id, b);
        assert_eq!(rows[1].id, a);
    }

    #[tokio::test]
    async fn get_returns_the_requested_track() {
        let db = tmp_db().await;
        let id = insert_fixture(&db.engine, "Charlie", "/tmp/c.flac").await;
        let row = get(&db.engine, id).await.unwrap();
        assert_eq!(row.title, "Charlie");
        assert_eq!(row.file_path, "/tmp/c.flac");
    }

    #[tokio::test]
    async fn bump_play_count_increments() {
        let db = tmp_db().await;
        let id = insert_fixture(&db.engine, "Delta", "/tmp/d.flac").await;
        bump_play_count(&db.engine, id).await.unwrap();
        bump_play_count(&db.engine, id).await.unwrap();
        let row = get(&db.engine, id).await.unwrap();
        assert_eq!(row.play_count, 2);
        assert_eq!(row.skip_count, 0);
    }

    #[tokio::test]
    async fn bump_skip_count_increments() {
        let db = tmp_db().await;
        let id = insert_fixture(&db.engine, "Echo", "/tmp/e.flac").await;
        bump_skip_count(&db.engine, id).await.unwrap();
        let row = get(&db.engine, id).await.unwrap();
        assert_eq!(row.skip_count, 1);
        assert_eq!(row.play_count, 0);
    }

    #[test]
    fn tracks_error_display_is_stable() {
        // Exercise TracksError to keep the variant non-dead.
        let e = TracksError::Query(anyhow::anyhow!("whatever"));
        assert!(e.to_string().contains("whatever"));
    }

    #[test]
    fn track_row_roundtrips_through_serde() {
        // Exercises Serialize + Deserialize on TrackRow.
        let row = TrackRow {
            id: 42,
            title: "Test Track".into(),
            artist: Some("Test Artist".into()),
            album: Some("Test Album".into()),
            duration_ms: 180_000,
            file_path: "/test/path.flac".into(),
            sample_rate: Some(44_100),
            bit_depth: Some(16),
            kind: Some("flac".into()),
            play_count: 5,
            skip_count: 2,
        };
        let json = serde_json::to_string(&row).unwrap();
        let back: TrackRow = serde_json::from_str(&json).unwrap();
        assert_eq!(row, back);
    }

    #[tokio::test]
    async fn itl_insert_and_by_persistent_id_roundtrip() {
        let db = tmp_db().await;
        let source_id = 1_i64;
        // Create a stub sync_source row to satisfy FK.
        db.engine
            .raw_sql_execute(
                "INSERT INTO sync_sources (id, name, source_path, path_mappings, \
                 conflict_rules, kind) VALUES (1, 'x', '/x', '[]', '{}', 'itunes_itl')",
                &[],
            )
            .await
            .unwrap();

        let upsert = ItlTrackUpsert {
            persistent_id: 0xDEADBEEF_DEADBEEF,
            sync_source_id: source_id,
            title: "Foxtrot",
            artist: Some("Genesis"),
            album: Some("Foxtrot"),
            album_artist: Some("Genesis"),
            composer: None,
            genre: Some("Rock"),
            kind: Some("FLAC"),
            duration_ms: 600_000,
            size_bytes: 40_000_000,
            bit_rate: Some(1000),
            sample_rate: Some(96000),
            track_number: Some(1),
            disc_number: Some(1),
            year: Some(1972),
            bpm: None,
            rating: 80,
            play_count: 12,
            date_added_unix: 1_700_000_000,
            file_path: "/mnt/d/music/foxtrot.flac",
            original_path: Some("D:\\music\\foxtrot.flac"),
        };

        let id = insert_from_itl(&db.engine, &upsert).await.unwrap();
        assert!(id > 0);

        let hex = format!("{:016x}", upsert.persistent_id);
        let found = by_persistent_id(&db.engine, source_id, &hex)
            .await
            .unwrap()
            .expect("track exists");
        assert_eq!(found.id, id);
        assert_eq!(found.rating, 80);
        assert_eq!(found.play_count, 12);
        assert_eq!(
            found.original_path.as_deref(),
            Some("D:\\music\\foxtrot.flac")
        );
    }

    #[tokio::test]
    async fn delete_missing_removes_only_unlisted_tracks() {
        let db = tmp_db().await;
        db.engine
            .raw_sql_execute(
                "INSERT INTO sync_sources (id, name, source_path, path_mappings, \
                 conflict_rules, kind) VALUES (1, 'x', '/x', '[]', '{}', 'itunes_itl')",
                &[],
            )
            .await
            .unwrap();

        let mk = |pid: u64, title: &'static str, path: &'static str| ItlTrackUpsert {
            persistent_id: pid,
            sync_source_id: 1,
            title,
            artist: None,
            album: None,
            album_artist: None,
            composer: None,
            genre: None,
            kind: None,
            duration_ms: 1000,
            size_bytes: 100,
            bit_rate: None,
            sample_rate: None,
            track_number: None,
            disc_number: None,
            year: None,
            bpm: None,
            rating: 0,
            play_count: 0,
            date_added_unix: 0,
            file_path: path,
            original_path: None,
        };
        insert_from_itl(&db.engine, &mk(1, "A", "/tmp/a"))
            .await
            .unwrap();
        insert_from_itl(&db.engine, &mk(2, "B", "/tmp/b"))
            .await
            .unwrap();
        insert_from_itl(&db.engine, &mk(3, "C", "/tmp/c"))
            .await
            .unwrap();

        let keep = vec![format!("{:016x}", 1u64), format!("{:016x}", 3u64)];
        let deleted = delete_missing(&db.engine, 1, &keep).await.unwrap();
        assert_eq!(deleted, 1);

        let remaining: i64 = db
            .engine
            .raw_sql_scalar("SELECT COUNT(*) FROM tracks", &[])
            .await
            .unwrap();
        assert_eq!(remaining, 2);
    }
}
