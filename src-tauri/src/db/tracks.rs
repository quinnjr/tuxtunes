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
}
