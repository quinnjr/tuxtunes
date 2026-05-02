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
    pub file_hash: Option<String>,
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
    search: Option<&str>,
) -> Result<Vec<TrackRow>, TracksError> {
    use prax_query::filter::FilterValue as FV;

    let trimmed = search.map(str::trim).filter(|s| !s.is_empty());

    let (where_clause, mut params) = match trimmed {
        Some(q) => {
            let pattern = format!("%{}%", escape_like(q));
            (
                "WHERE (title LIKE ? ESCAPE '\\' \
                  OR artist LIKE ? ESCAPE '\\' \
                  OR album LIKE ? ESCAPE '\\' \
                  OR album_artist LIKE ? ESCAPE '\\')",
                vec![
                    FV::String(pattern.clone()),
                    FV::String(pattern.clone()),
                    FV::String(pattern.clone()),
                    FV::String(pattern),
                ],
            )
        }
        None => ("", Vec::new()),
    };

    let sql = format!(
        "SELECT id, title, artist, album, duration_ms, file_path, file_hash, \
         sample_rate, bit_depth, kind, play_count, skip_count \
         FROM tracks {where_clause} \
         ORDER BY date_added DESC, id DESC \
         LIMIT ? OFFSET ?"
    );
    params.push(FV::Int(limit));
    params.push(FV::Int(offset));

    let json_rows = engine
        .raw_sql_query(&sql, &params)
        .await
        .map_err(|e| TracksError::Query(anyhow::Error::from(e)))?;
    let rows = json_rows
        .into_iter()
        .map(|r| serde_json::from_value(r.into_json()))
        .collect::<Result<Vec<TrackRow>, _>>()
        .map_err(|e| TracksError::Query(anyhow::Error::from(e)))?;
    Ok(rows)
}

/// Escape SQL LIKE wildcards so a user-typed `%` or `_` doesn't expand
/// into a wildcard match. Pairs with `ESCAPE '\\'` in the LIKE clause.
fn escape_like(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for ch in s.chars() {
        if matches!(ch, '\\' | '%' | '_') {
            out.push('\\');
        }
        out.push(ch);
    }
    out
}

pub async fn get(engine: &SqliteRawEngine, id: i64) -> Result<TrackRow, TracksError> {
    let sql = "SELECT id, title, artist, album, duration_ms, file_path, file_hash, \
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

/// Local-side view of a track used for conflict resolution: row id +
/// every user-state field needed by `sync::conflict::resolve_*`.
#[derive(Debug, Clone, Deserialize)]
pub struct LocalTrackForSync {
    pub id: i64,
    pub rating: i64,
    pub play_count: i64,
    pub skip_count: i64,
    pub last_played: Option<i64>,
    pub last_skipped: Option<i64>,
    #[serde(deserialize_with = "crate::db::sync_util::sqlite_bool")]
    pub loved: bool,
    pub original_path: Option<String>,
}

const SELECT_LOCAL_TRACK_FIELDS: &str = "id, rating, play_count, skip_count, last_played, \
     last_skipped, loved, original_path";

/// Bulk-load every synced track's user-state into a `persistent_id
/// (u64) → LocalTrackForSync` map. Replaces per-track
/// `by_persistent_id` SELECTs during reconcile (O(n) round-trips → 1).
pub async fn load_local_state_map(
    engine: &SqliteRawEngine,
    sync_source_id: i64,
) -> Result<std::collections::HashMap<u64, LocalTrackForSync>, TracksError> {
    let sql = format!(
        "SELECT {SELECT_LOCAL_TRACK_FIELDS}, persistent_id FROM tracks \
         WHERE sync_source_id = ? AND persistent_id IS NOT NULL"
    );
    let rows = engine
        .raw_sql_query(
            &sql,
            &[prax_query::filter::FilterValue::Int(sync_source_id)],
        )
        .await
        .map_err(|e| TracksError::Query(anyhow::Error::from(e)))?;
    let mut out = std::collections::HashMap::with_capacity(rows.len());
    for r in rows {
        let mut v = r.into_json();
        let Some(pid_str) = v
            .as_object_mut()
            .and_then(|o| o.remove("persistent_id"))
            .and_then(|v| v.as_str().map(str::to_string))
        else {
            continue;
        };
        let Ok(pid) = u64::from_str_radix(&pid_str, 16) else {
            continue;
        };
        let t: LocalTrackForSync =
            serde_json::from_value(v).map_err(|e| TracksError::Query(anyhow::Error::from(e)))?;
        out.insert(pid, t);
    }
    Ok(out)
}

pub async fn by_persistent_id(
    engine: &SqliteRawEngine,
    sync_source_id: i64,
    persistent_id_hex: &str,
) -> Result<Option<LocalTrackForSync>, TracksError> {
    let sql = format!(
        "SELECT {SELECT_LOCAL_TRACK_FIELDS} FROM tracks \
         WHERE sync_source_id = ? AND persistent_id = ?"
    );
    let params = vec![
        prax_query::filter::FilterValue::Int(sync_source_id),
        prax_query::filter::FilterValue::String(persistent_id_hex.to_string()),
    ];
    let json_row = match engine.raw_sql_optional(&sql, &params).await {
        Ok(Some(r)) => r,
        Ok(None) => return Ok(None),
        Err(e) => return Err(TracksError::Query(anyhow::Error::from(e))),
    };
    let t: LocalTrackForSync = serde_json::from_value(json_row.into_json())
        .map_err(|e| TracksError::Query(anyhow::Error::from(e)))?;
    Ok(Some(t))
}

/// Insert a new track from an ITL upsert record. Returns the local id.
pub async fn insert_from_itl(
    engine: &SqliteRawEngine,
    t: &ItlTrackUpsert<'_>,
) -> Result<i64, TracksError> {
    use crate::db::sync_util::{opt_int, opt_str, pid_hex};
    use prax_query::filter::FilterValue as FV;
    let sql = "INSERT INTO tracks ( \
        persistent_id, sync_source_id, title, artist, album, album_artist, \
        composer, genre, kind, duration_ms, size_bytes, bit_rate, sample_rate, \
        track_number, disc_number, year, bpm, rating, play_count, \
        date_added, file_path, original_path, playlist_ids) \
        VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, \
                datetime(?, 'unixepoch'), ?, ?, '[]') RETURNING id";
    let params = vec![
        FV::String(pid_hex(t.persistent_id)),
        FV::Int(t.sync_source_id),
        FV::String(t.title.to_string()),
        opt_str(t.artist),
        opt_str(t.album),
        opt_str(t.album_artist),
        opt_str(t.composer),
        opt_str(t.genre),
        opt_str(t.kind),
        FV::Int(t.duration_ms),
        FV::Int(t.size_bytes),
        opt_int(t.bit_rate),
        opt_int(t.sample_rate),
        opt_int(t.track_number),
        opt_int(t.disc_number),
        opt_int(t.year),
        opt_int(t.bpm),
        FV::Int(t.rating),
        FV::Int(t.play_count),
        FV::Int(t.date_added_unix),
        FV::String(t.file_path.to_string()),
        opt_str(t.original_path),
    ];
    let json_row = engine
        .raw_sql_first(sql, &params)
        .await
        .map_err(|e| TracksError::Query(anyhow::Error::from(e)))?;
    json_row
        .into_json()
        .get("id")
        .and_then(|v| v.as_i64())
        .ok_or_else(|| TracksError::Query(anyhow::anyhow!("INSERT ... RETURNING id missing")))
}

/// Update an existing track's descriptive fields plus the two
/// already-resolved user-state fields. User-state not listed here
/// (skip_count, last_played, last_skipped, loved) is preserved as-is.
pub async fn update_descriptive_fields(
    engine: &SqliteRawEngine,
    local_id: i64,
    t: &ItlTrackUpsert<'_>,
    resolved_rating: i64,
    resolved_play_count: i64,
) -> Result<(), TracksError> {
    use crate::db::sync_util::{opt_int, opt_str};
    use prax_query::filter::FilterValue as FV;
    let sql = "UPDATE tracks SET \
        title = ?, artist = ?, album = ?, album_artist = ?, composer = ?, \
        genre = ?, kind = ?, duration_ms = ?, size_bytes = ?, bit_rate = ?, \
        sample_rate = ?, track_number = ?, disc_number = ?, year = ?, bpm = ?, \
        rating = ?, play_count = ?, file_path = ? \
        WHERE id = ?";
    let params = vec![
        FV::String(t.title.to_string()),
        opt_str(t.artist),
        opt_str(t.album),
        opt_str(t.album_artist),
        opt_str(t.composer),
        opt_str(t.genre),
        opt_str(t.kind),
        FV::Int(t.duration_ms),
        FV::Int(t.size_bytes),
        opt_int(t.bit_rate),
        opt_int(t.sample_rate),
        opt_int(t.track_number),
        opt_int(t.disc_number),
        opt_int(t.year),
        opt_int(t.bpm),
        FV::Int(resolved_rating),
        FV::Int(resolved_play_count),
        FV::String(t.file_path.to_string()),
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
    keep: &[u64],
) -> Result<u64, TracksError> {
    crate::db::sync_util::delete_by_keep_set(engine, "tracks", sync_source_id, keep)
        .await
        .map_err(|e| TracksError::Query(anyhow::Error::from(e)))
}

/// Update the path-related columns after a successful ingest. Sets
/// `file_path`, `original_path`, `file_hash`, `artwork_path`; marks
/// `import_status = 'ok'` and refreshes `verified_at`.
pub async fn set_file_paths(
    engine: &SqliteRawEngine,
    local_id: i64,
    managed_file_path: &str,
    original_path: Option<&str>,
    file_hash_hex: &str,
    artwork_path: Option<&str>,
) -> Result<(), TracksError> {
    use crate::db::sync_util::opt_str;
    use prax_query::filter::FilterValue as FV;
    let sql = "UPDATE tracks SET \
        file_path = ?, original_path = ?, file_hash = ?, artwork_path = ?, \
        import_status = 'ok', verified_at = CURRENT_TIMESTAMP \
        WHERE id = ?";
    let params = vec![
        FV::String(managed_file_path.to_string()),
        opt_str(original_path),
        FV::String(file_hash_hex.to_string()),
        opt_str(artwork_path),
        FV::Int(local_id),
    ];
    engine
        .raw_sql_execute(sql, &params)
        .await
        .map(|_| ())
        .map_err(|e| TracksError::Query(anyhow::Error::from(e)))
}

/// Mark a track as `missing_source` (file not reachable). Preserves
/// `file_path` so the user can diagnose the mapping.
pub async fn mark_missing_source(
    engine: &SqliteRawEngine,
    local_id: i64,
) -> Result<(), TracksError> {
    use prax_query::filter::FilterValue as FV;
    let sql = "UPDATE tracks SET import_status = 'missing_source', \
               verified_at = CURRENT_TIMESTAMP WHERE id = ?";
    engine
        .raw_sql_execute(sql, &[FV::Int(local_id)])
        .await
        .map(|_| ())
        .map_err(|e| TracksError::Query(anyhow::Error::from(e)))
}

/// Record a freshly-computed file hash (plus bump `verified_at`) — used
/// by the "Verify Library" walk when content confirms a file is intact.
pub async fn set_file_hash(
    engine: &SqliteRawEngine,
    local_id: i64,
    file_hash_hex: &str,
) -> Result<(), TracksError> {
    use prax_query::filter::FilterValue as FV;
    let sql = "UPDATE tracks SET file_hash = ?, \
               verified_at = CURRENT_TIMESTAMP WHERE id = ?";
    engine
        .raw_sql_execute(
            sql,
            &[FV::String(file_hash_hex.to_string()), FV::Int(local_id)],
        )
        .await
        .map(|_| ())
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
        let rows = list(&db.engine, 10, 0, None).await.unwrap();
        assert_eq!(rows.len(), 2);
        // newest first — Bravo was inserted second → has the higher id
        assert_eq!(rows[0].id, b);
        assert_eq!(rows[1].id, a);
    }

    #[tokio::test]
    async fn list_filters_by_search_substring_case_insensitive() {
        let db = tmp_db().await;
        insert_fixture(&db.engine, "Alpha", "/tmp/a.flac").await;
        insert_fixture(&db.engine, "Bravo", "/tmp/b.flac").await;
        // SQLite LIKE is case-insensitive for ASCII by default.
        let rows = list(&db.engine, 10, 0, Some("brav")).await.unwrap();
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].title, "Bravo");
    }

    #[tokio::test]
    async fn list_search_escapes_like_wildcards() {
        let db = tmp_db().await;
        insert_fixture(&db.engine, "Alpha", "/tmp/a.flac").await;
        insert_fixture(&db.engine, "100% pure", "/tmp/p.flac").await;
        // A literal `%` must not act as a wildcard.
        let rows = list(&db.engine, 10, 0, Some("%")).await.unwrap();
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].title, "100% pure");
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
            file_hash: Some("deadbeefdeadbeef".into()),
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

        let hex = crate::db::sync_util::pid_hex(upsert.persistent_id);
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

        let keep = vec![1u64, 3u64];
        let deleted = delete_missing(&db.engine, 1, &keep).await.unwrap();
        assert_eq!(deleted, 1);

        let remaining: i64 = db
            .engine
            .raw_sql_scalar("SELECT COUNT(*) FROM tracks", &[])
            .await
            .unwrap();
        assert_eq!(remaining, 2);
    }

    #[tokio::test]
    async fn set_file_paths_writes_columns_and_marks_ok() {
        let db = tmp_db().await;
        let id = insert_fixture(&db.engine, "title", "/tmp/a.flac").await;
        set_file_paths(
            &db.engine,
            id,
            "/home/joe/Music/TuxTunes/a/b.flac",
            Some("D:\\a\\b.flac"),
            "deadbeefdeadbeef",
            Some("/home/joe/Music/TuxTunes/a/cover.jpg"),
        )
        .await
        .unwrap();

        let got = get(&db.engine, id).await.unwrap();
        assert_eq!(got.file_path, "/home/joe/Music/TuxTunes/a/b.flac");

        let status: String = db
            .engine
            .raw_sql_scalar(
                "SELECT import_status FROM tracks WHERE id = ?",
                &[prax_query::filter::FilterValue::Int(id)],
            )
            .await
            .unwrap();
        assert_eq!(status, "ok");
    }

    #[tokio::test]
    async fn mark_missing_source_sets_status() {
        let db = tmp_db().await;
        let id = insert_fixture(&db.engine, "missing", "/tmp/m.flac").await;
        mark_missing_source(&db.engine, id).await.unwrap();
        let status: String = db
            .engine
            .raw_sql_scalar(
                "SELECT import_status FROM tracks WHERE id = ?",
                &[prax_query::filter::FilterValue::Int(id)],
            )
            .await
            .unwrap();
        assert_eq!(status, "missing_source");
    }

    #[tokio::test]
    async fn set_file_hash_updates_hash_and_verified_at() {
        let db = tmp_db().await;
        let id = insert_fixture(&db.engine, "hashed", "/tmp/h.flac").await;
        set_file_hash(&db.engine, id, "0123456789abcdef")
            .await
            .unwrap();

        let hash: String = db
            .engine
            .raw_sql_scalar(
                "SELECT file_hash FROM tracks WHERE id = ?",
                &[prax_query::filter::FilterValue::Int(id)],
            )
            .await
            .unwrap();
        assert_eq!(hash, "0123456789abcdef");

        // verified_at should be populated (non-null).
        let verified: Option<String> = db
            .engine
            .raw_sql_optional(
                "SELECT verified_at FROM tracks WHERE id = ?",
                &[prax_query::filter::FilterValue::Int(id)],
            )
            .await
            .unwrap()
            .and_then(|r| {
                r.into_json()
                    .get("verified_at")
                    .and_then(|v| v.as_str().map(|s| s.to_string()))
            });
        assert!(verified.is_some(), "verified_at should be set");
    }
}
