//! Minimal query helpers for the `tracks` table.

use prax_sqlite::raw::SqliteRawEngine;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct TrackRow {
    pub id: i64,
    pub title: String,
    pub artist: Option<String>,
    pub album: Option<String>,
    /// Album-level artist, distinct from the per-track `artist` (e.g.
    /// "Various Artists" compilations). Falls back to `artist` when
    /// absent — see `fs::path::TrackFields::from_track_row`.
    #[serde(default)]
    pub album_artist: Option<String>,
    #[serde(default)]
    pub genre: Option<String>,
    #[serde(default)]
    pub year: Option<i64>,
    #[serde(default)]
    pub track_number: Option<i64>,
    #[serde(default)]
    pub disc_number: Option<i64>,
    pub duration_ms: i64,
    pub file_path: String,
    pub file_hash: Option<String>,
    pub sample_rate: Option<i64>,
    pub bit_depth: Option<i64>,
    pub kind: Option<String>,
    pub play_count: i64,
    pub skip_count: i64,
    /// `'ok'` normally; `'missing_source'` once verify or a failed play
    /// found the file unreachable. Defaults to `'ok'` when absent so
    /// callers constructing rows by hand (tests) don't need to care.
    #[serde(default = "default_import_status")]
    pub import_status: String,
    /// Cached cover image, if one has been resolved for the album.
    #[serde(default)]
    pub artwork_path: Option<String>,
}

fn default_import_status() -> String {
    "ok".to_string()
}

/// Every column backing `TrackRow`, in declaration order. Shared by
/// every SELECT that hydrates `TrackRow`s (see `db::albums`,
/// `db::playlists`, `db::smart`) so the column list lives in one place.
pub const TRACK_ROW_COLUMNS: &str = "id, title, artist, album, album_artist, genre, year, \
     track_number, disc_number, duration_ms, file_path, file_hash, sample_rate, bit_depth, \
     kind, play_count, skip_count, import_status, artwork_path";

#[derive(Debug, thiserror::Error)]
pub enum TracksError {
    #[error("query failed: {0}")]
    Query(#[source] anyhow::Error),
}

/// Sort spec for the track-list view. The `column` field is validated
/// against an allowlist below so user-supplied input never reaches SQL.
#[derive(Debug, Clone, Deserialize)]
pub struct TrackSort {
    pub column: String,
    #[serde(default)]
    pub descending: bool,
}

impl Default for TrackSort {
    fn default() -> Self {
        Self {
            column: "date_added".into(),
            descending: true,
        }
    }
}

/// Map a logical sort column to its SQL ORDER BY expression. Unknown
/// columns return None so the caller can fall back to the default.
/// User input is validated against this allowlist — no string-built SQL.
fn sort_expr_for(column: &str, descending: bool) -> Option<String> {
    let (expr, nocase): (&str, bool) = match column {
        "title" => ("title", true),
        "artist" => (
            "COALESCE(NULLIF(album_artist, ''), NULLIF(artist, ''))",
            true,
        ),
        "album" => ("album", true),
        "genre" => ("genre", true),
        "year" => ("year", false),
        "duration_ms" => ("duration_ms", false),
        "rating" => ("rating", false),
        "play_count" => ("play_count", false),
        "last_played" => ("last_played", false),
        "date_added" => ("date_added", false),
        "bit_rate" => ("bit_rate", false),
        "sample_rate" => ("sample_rate", false),
        "kind" => ("kind", true),
        "size_bytes" => ("size_bytes", false),
        _ => return None,
    };
    let dir = if descending { "DESC" } else { "ASC" };
    let nulls = if descending { "FIRST" } else { "LAST" };
    Some(if nocase {
        format!("{expr} COLLATE NOCASE {dir} NULLS {nulls}")
    } else {
        format!("{expr} {dir} NULLS {nulls}")
    })
}

pub async fn list(
    engine: &SqliteRawEngine,
    limit: i64,
    offset: i64,
    filters: &crate::db::distinct::TrackFilters,
    sort: Option<&TrackSort>,
) -> Result<Vec<TrackRow>, TracksError> {
    use prax_query::filter::FilterValue as FV;

    let (where_clause, mut params) = crate::db::distinct::build_where(filters);

    // Resolve the requested sort; unknown columns fall back to the
    // default rather than erroring — the UI's column picker is the
    // primary entry point and a typo there shouldn't break the view.
    let order_expr = sort
        .and_then(|s| sort_expr_for(&s.column, s.descending))
        .unwrap_or_else(|| "date_added DESC, id DESC".to_string());

    let sql = format!(
        "SELECT {TRACK_ROW_COLUMNS} \
         FROM tracks {where_clause} \
         ORDER BY {order_expr} \
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

pub async fn get(engine: &SqliteRawEngine, id: i64) -> Result<TrackRow, TracksError> {
    let sql = format!("SELECT {TRACK_ROW_COLUMNS} FROM tracks WHERE id = ?");
    let params = vec![prax_query::filter::FilterValue::Int(id)];
    let json_row = engine
        .raw_sql_first(&sql, &params)
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

/// The descriptive fields a user can edit from the track-info dialog.
/// Everything else on the row (technical facts, user state, sync
/// bookkeeping) is owned elsewhere.
#[derive(Debug, Clone, serde::Deserialize)]
pub struct MetadataEdit<'a> {
    pub title: &'a str,
    pub artist: Option<&'a str>,
    pub album: Option<&'a str>,
    pub album_artist: Option<&'a str>,
    pub genre: Option<&'a str>,
    pub year: Option<i64>,
    pub track_number: Option<i64>,
    pub disc_number: Option<i64>,
}

/// Apply a user's metadata edit and mark the row `user_edited` so the
/// sync reconciler stops overwriting these fields from the source.
pub async fn update_metadata(
    engine: &SqliteRawEngine,
    local_id: i64,
    e: &MetadataEdit<'_>,
) -> Result<(), TracksError> {
    use crate::db::sync_util::{opt_int, opt_str};
    use prax_query::filter::FilterValue as FV;
    let title = e.title.trim();
    if title.is_empty() {
        return Err(TracksError::Query(anyhow::anyhow!(
            "track title cannot be empty"
        )));
    }
    let sql = "UPDATE tracks SET \
        title = ?, artist = ?, album = ?, album_artist = ?, genre = ?, \
        year = ?, track_number = ?, disc_number = ?, \
        user_edited = 1, date_modified = CURRENT_TIMESTAMP \
        WHERE id = ?";
    let params = vec![
        FV::String(title.to_string()),
        opt_str(e.artist),
        opt_str(e.album),
        opt_str(e.album_artist),
        opt_str(e.genre),
        opt_int(e.year),
        opt_int(e.track_number),
        opt_int(e.disc_number),
        FV::Int(local_id),
    ];
    let n = engine
        .raw_sql_execute(sql, &params)
        .await
        .map_err(|e| TracksError::Query(anyhow::Error::from(e)))?;
    if n == 0 {
        return Err(TracksError::Query(anyhow::anyhow!(
            "track {local_id} not found"
        )));
    }
    Ok(())
}

/// Local-side view of a track used for conflict resolution: row id +
/// every user-state field needed by `sync::conflict::resolve_*`.
#[derive(Debug, Clone, Deserialize)]
pub struct LocalTrackForSync {
    pub id: i64,
    pub rating: i64,
    pub play_count: i64,
    pub skip_count: i64,
    /// SQLite `CURRENT_TIMESTAMP` text (`YYYY-MM-DD HH:MM:SS`), set by
    /// `bump_play_count` / `bump_skip_count`. Carried for the conflict
    /// resolver's benefit; never parsed as a number.
    pub last_played: Option<String>,
    pub last_skipped: Option<String>,
    #[serde(deserialize_with = "crate::db::sync_util::sqlite_bool")]
    pub loved: bool,
    pub original_path: Option<String>,
    /// Owning sync source, or `None` for rows added by "Add Folder".
    /// `load_local_state_by_path` scans every row regardless of source
    /// (`file_path` is globally UNIQUE), so the reconciler needs this to
    /// know when adopting a row also means claiming it for its source.
    #[serde(default)]
    pub sync_source_id: Option<i64>,
    /// Present only in `load_local_state_by_path` results.
    #[serde(default)]
    pub persistent_id: Option<String>,
}

const SELECT_LOCAL_TRACK_FIELDS: &str = "id, rating, play_count, skip_count, last_played, \
     last_skipped, loved, original_path, sync_source_id";

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
///
/// When `t.file_path` differs from the row's current path the row has
/// been relinked (collision-suffix recovery, a changed path mapping), so
/// the stale verification state is reset too: `import_status` back to
/// `'ok'`, `file_hash` cleared so the next Verify re-canonicalises the
/// new file, and `verified_at` refreshed. SQLite evaluates every SET
/// expression against the pre-UPDATE row, so the `file_path = ?`
/// comparisons below see the old value.
pub async fn update_descriptive_fields(
    engine: &SqliteRawEngine,
    local_id: i64,
    t: &ItlTrackUpsert<'_>,
    resolved_rating: i64,
    resolved_play_count: i64,
) -> Result<(), TracksError> {
    use crate::db::sync_util::{opt_int, opt_str};
    use prax_query::filter::FilterValue as FV;
    // Fields the user may have edited locally (see `update_metadata`)
    // are preserved for `user_edited` rows; the source only ever writes
    // them for rows the user never touched. Technical facts and
    // sync-owned state update either way.
    let sql = "UPDATE tracks SET \
        title = CASE WHEN user_edited = 1 THEN title ELSE ? END, \
        artist = CASE WHEN user_edited = 1 THEN artist ELSE ? END, \
        album = CASE WHEN user_edited = 1 THEN album ELSE ? END, \
        album_artist = CASE WHEN user_edited = 1 THEN album_artist ELSE ? END, \
        composer = ?, \
        genre = CASE WHEN user_edited = 1 THEN genre ELSE ? END, \
        kind = ?, duration_ms = ?, size_bytes = ?, bit_rate = ?, \
        sample_rate = ?, \
        track_number = CASE WHEN user_edited = 1 THEN track_number ELSE ? END, \
        disc_number = CASE WHEN user_edited = 1 THEN disc_number ELSE ? END, \
        year = CASE WHEN user_edited = 1 THEN year ELSE ? END, \
        bpm = ?, \
        rating = ?, play_count = ?, file_path = ?, \
        import_status = CASE WHEN file_path = ? THEN import_status ELSE 'ok' END, \
        file_hash = CASE WHEN file_path = ? THEN file_hash ELSE NULL END, \
        verified_at = CASE WHEN file_path = ? THEN verified_at ELSE CURRENT_TIMESTAMP END \
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
        FV::String(t.file_path.to_string()),
        FV::String(t.file_path.to_string()),
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

/// `file_path → local state` for **every** track row, whatever sync
/// source owns it (including "Add Folder" rows with a NULL
/// `sync_source_id`). Lets the reconciler re-adopt rows whose
/// persistent id changed (a parser fix upstream, or an iTunes rebuild)
/// instead of tripping the UNIQUE constraint on `file_path` — which is
/// global, so scoping this to one source made foreign rows invisible
/// and turned every such collision into an aborted sync.
///
/// The value carries the row's user state so the reconciler can run the
/// same conflict rules it uses on a pid match, rather than blindly
/// overwriting local ratings and play counts.
pub async fn load_local_state_by_path(
    engine: &SqliteRawEngine,
) -> Result<std::collections::HashMap<String, LocalTrackForSync>, TracksError> {
    let sql = format!("SELECT {SELECT_LOCAL_TRACK_FIELDS}, persistent_id, file_path FROM tracks");
    let rows = engine
        .raw_sql_query(&sql, &[])
        .await
        .map_err(|e| TracksError::Query(anyhow::Error::from(e)))?;
    let mut out = std::collections::HashMap::with_capacity(rows.len());
    for r in rows {
        let mut v = r.into_json();
        let Some(path) = v
            .as_object_mut()
            .and_then(|o| o.remove("file_path"))
            .and_then(|v| v.as_str().map(str::to_string))
        else {
            continue;
        };
        let t: LocalTrackForSync =
            serde_json::from_value(v).map_err(|e| TracksError::Query(anyhow::Error::from(e)))?;
        out.insert(path, t);
    }
    Ok(out)
}

/// Claim a row for `sync_source_id` while re-keying it to `pid_hex`.
/// Used when the reconciler adopts a row that "Add Folder" (NULL
/// source) or another sync source created at the same path.
pub async fn adopt_into_source(
    engine: &SqliteRawEngine,
    local_id: i64,
    sync_source_id: i64,
    pid_hex: &str,
) -> Result<(), TracksError> {
    use prax_query::filter::FilterValue as FV;
    engine
        .raw_sql_execute(
            "UPDATE tracks SET persistent_id = ?, sync_source_id = ? WHERE id = ?",
            &[
                FV::String(pid_hex.to_string()),
                FV::Int(sync_source_id),
                FV::Int(local_id),
            ],
        )
        .await
        .map(|_| ())
        .map_err(|e| TracksError::Query(anyhow::Error::from(e)))
}

/// Re-key a row to a new persistent id (see `load_local_state_by_path`).
pub async fn set_persistent_id(
    engine: &SqliteRawEngine,
    local_id: i64,
    pid_hex: &str,
) -> Result<(), TracksError> {
    use prax_query::filter::FilterValue as FV;
    engine
        .raw_sql_execute(
            "UPDATE tracks SET persistent_id = ? WHERE id = ?",
            &[FV::String(pid_hex.to_string()), FV::Int(local_id)],
        )
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

/// Point a track at a different file (the recovered base file for an
/// iTunes ` N` collision-suffix entry) and mark it healthy again.
/// Fails on the `file_path` UNIQUE constraint if another row already
/// owns `new_path` — callers treat that as "merge instead".
pub async fn relink_file_path(
    engine: &SqliteRawEngine,
    local_id: i64,
    new_path: &str,
) -> Result<(), TracksError> {
    use prax_query::filter::FilterValue as FV;
    let sql = "UPDATE tracks SET file_path = ?, import_status = 'ok', file_hash = NULL, \
               verified_at = CURRENT_TIMESTAMP WHERE id = ?";
    engine
        .raw_sql_execute(sql, &[FV::String(new_path.to_string()), FV::Int(local_id)])
        .await
        .map(|_| ())
        .map_err(|e| TracksError::Query(anyhow::Error::from(e)))
}

/// Does any track already reference `path`? Used before relinking so
/// a UNIQUE violation is a decision, not a surprise.
pub async fn path_in_use(engine: &SqliteRawEngine, path: &str) -> Result<bool, TracksError> {
    use prax_query::filter::FilterValue as FV;
    let n: i64 = engine
        .raw_sql_scalar(
            "SELECT COUNT(*) FROM tracks WHERE file_path = ?",
            &[FV::String(path.to_string())],
        )
        .await
        .map_err(|e| TracksError::Query(anyhow::Error::from(e)))?;
    Ok(n > 0)
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

    fn itl_fixture(pid: u64, title: &'static str, path: &'static str) -> ItlTrackUpsert<'static> {
        ItlTrackUpsert {
            persistent_id: pid,
            sync_source_id: 1,
            title,
            artist: Some("Source Artist"),
            album: Some("Source Album"),
            album_artist: None,
            composer: None,
            genre: Some("Rock"),
            kind: None,
            duration_ms: 1000,
            size_bytes: 100,
            bit_rate: None,
            sample_rate: None,
            track_number: Some(1),
            disc_number: None,
            year: Some(2001),
            bpm: None,
            rating: 0,
            play_count: 0,
            date_added_unix: 0,
            file_path: path,
            original_path: None,
        }
    }

    async fn seed_source(db: &Db) {
        db.engine
            .raw_sql_execute(
                "INSERT INTO sync_sources (id, name, source_path, path_mappings, \
                 conflict_rules, kind) VALUES (1, 'x', '/x', '[]', '{}', 'itunes_itl')",
                &[],
            )
            .await
            .unwrap();
    }

    #[tokio::test]
    async fn update_metadata_writes_fields_and_marks_user_edited() {
        let db = tmp_db().await;
        let id = insert_fixture(&db.engine, "Old Title", "/tmp/m.flac").await;
        update_metadata(
            &db.engine,
            id,
            &MetadataEdit {
                title: "New Title",
                artist: Some("blink-182"),
                album: Some("TOYPAJ"),
                album_artist: Some("blink-182"),
                genre: Some("Punk"),
                year: Some(2001),
                track_number: Some(1),
                disc_number: None,
            },
        )
        .await
        .unwrap();
        let row = get(&db.engine, id).await.unwrap();
        assert_eq!(row.title, "New Title");
        assert_eq!(row.artist.as_deref(), Some("blink-182"));
        assert_eq!(row.genre.as_deref(), Some("Punk"));
        assert_eq!(row.year, Some(2001));
        assert_eq!(row.disc_number, None);
        let flagged: i64 = db
            .engine
            .raw_sql_scalar(
                "SELECT user_edited FROM tracks WHERE id = ?",
                &[prax_query::filter::FilterValue::Int(id)],
            )
            .await
            .unwrap();
        assert_eq!(flagged, 1);
    }

    #[tokio::test]
    async fn update_metadata_rejects_empty_title_and_unknown_id() {
        let db = tmp_db().await;
        let id = insert_fixture(&db.engine, "T", "/tmp/t.flac").await;
        let edit = MetadataEdit {
            title: "  ",
            artist: None,
            album: None,
            album_artist: None,
            genre: None,
            year: None,
            track_number: None,
            disc_number: None,
        };
        assert!(update_metadata(&db.engine, id, &edit).await.is_err());
        let ok = MetadataEdit { title: "x", ..edit };
        assert!(update_metadata(&db.engine, 424_242, &ok).await.is_err());
    }

    #[tokio::test]
    async fn user_edited_metadata_survives_a_sync_update() {
        let db = tmp_db().await;
        seed_source(&db).await;
        let u = itl_fixture(7, "Source Title", "/tmp/s.flac");
        let id = insert_from_itl(&db.engine, &u).await.unwrap();
        update_metadata(
            &db.engine,
            id,
            &MetadataEdit {
                title: "My Title",
                artist: Some("My Artist"),
                album: Some("My Album"),
                album_artist: None,
                genre: Some("My Genre"),
                year: Some(1999),
                track_number: Some(9),
                disc_number: Some(2),
            },
        )
        .await
        .unwrap();
        // The next sync re-applies the source's descriptive fields…
        update_descriptive_fields(&db.engine, id, &u, 0, 0)
            .await
            .unwrap();
        let row = get(&db.engine, id).await.unwrap();
        // …but the user's edits win for the editable set.
        assert_eq!(row.title, "My Title");
        assert_eq!(row.artist.as_deref(), Some("My Artist"));
        assert_eq!(row.album.as_deref(), Some("My Album"));
        assert_eq!(row.genre.as_deref(), Some("My Genre"));
        assert_eq!(row.year, Some(1999));
        assert_eq!(row.track_number, Some(9));
        assert_eq!(row.disc_number, Some(2));
    }

    #[tokio::test]
    async fn sync_still_updates_metadata_for_untouched_rows() {
        let db = tmp_db().await;
        seed_source(&db).await;
        let u = itl_fixture(8, "Source Title", "/tmp/u.flac");
        let id = insert_from_itl(&db.engine, &u).await.unwrap();
        let renamed = ItlTrackUpsert {
            title: "Retagged",
            ..u
        };
        update_descriptive_fields(&db.engine, id, &renamed, 0, 0)
            .await
            .unwrap();
        let row = get(&db.engine, id).await.unwrap();
        assert_eq!(row.title, "Retagged");
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
        let rows = list(&db.engine, 10, 0, &Default::default(), None)
            .await
            .unwrap();
        assert_eq!(rows.len(), 2);
        // newest first — Bravo was inserted second → has the higher id
        assert_eq!(rows[0].id, b);
        assert_eq!(rows[1].id, a);
    }

    #[tokio::test]
    async fn list_filters_by_search_substring_case_insensitive() {
        use crate::db::distinct::TrackFilters;
        let db = tmp_db().await;
        insert_fixture(&db.engine, "Alpha", "/tmp/a.flac").await;
        insert_fixture(&db.engine, "Bravo", "/tmp/b.flac").await;
        let f = TrackFilters {
            search: Some("brav".into()),
            ..Default::default()
        };
        let rows = list(&db.engine, 10, 0, &f, None).await.unwrap();
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].title, "Bravo");
    }

    #[tokio::test]
    async fn list_search_escapes_like_wildcards() {
        use crate::db::distinct::TrackFilters;
        let db = tmp_db().await;
        insert_fixture(&db.engine, "Alpha", "/tmp/a.flac").await;
        insert_fixture(&db.engine, "100% pure", "/tmp/p.flac").await;
        let f = TrackFilters {
            search: Some("%".into()),
            ..Default::default()
        };
        let rows = list(&db.engine, 10, 0, &f, None).await.unwrap();
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].title, "100% pure");
    }

    #[tokio::test]
    async fn list_sort_by_title_ascending() {
        let db = tmp_db().await;
        insert_fixture(&db.engine, "Charlie", "/tmp/c.flac").await;
        insert_fixture(&db.engine, "Alpha", "/tmp/a.flac").await;
        insert_fixture(&db.engine, "Bravo", "/tmp/b.flac").await;
        let sort = TrackSort {
            column: "title".into(),
            descending: false,
        };
        let rows = list(&db.engine, 10, 0, &Default::default(), Some(&sort))
            .await
            .unwrap();
        let titles: Vec<&str> = rows.iter().map(|r| r.title.as_str()).collect();
        assert_eq!(titles, vec!["Alpha", "Bravo", "Charlie"]);
    }

    #[tokio::test]
    async fn list_sort_unknown_column_falls_back_to_default() {
        let db = tmp_db().await;
        // Default order is date_added DESC, id DESC — last inserted first.
        let _a = insert_fixture(&db.engine, "Alpha", "/tmp/a.flac").await;
        let b = insert_fixture(&db.engine, "Bravo", "/tmp/b.flac").await;
        let sort = TrackSort {
            column: "bogus".into(),
            descending: false,
        };
        let rows = list(&db.engine, 10, 0, &Default::default(), Some(&sort))
            .await
            .unwrap();
        assert_eq!(rows[0].id, b);
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
            album_artist: Some("Test Album Artist".into()),
            genre: Some("Rock".into()),
            year: Some(1999),
            track_number: Some(3),
            disc_number: Some(1),
            duration_ms: 180_000,
            file_path: "/test/path.flac".into(),
            file_hash: Some("deadbeefdeadbeef".into()),
            sample_rate: Some(44_100),
            bit_depth: Some(16),
            kind: Some("flac".into()),
            play_count: 5,
            skip_count: 2,
            import_status: "ok".to_string(),
            artwork_path: None,
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

    #[test]
    fn track_sort_default_is_date_added_descending() {
        let s = TrackSort::default();
        assert_eq!(s.column, "date_added");
        assert!(s.descending);
    }

    #[tokio::test]
    async fn list_sort_ascending_uses_nulls_last() {
        // Mix of named and NULL years: ascending should put NULL last,
        // exercising the "ASC NULLS LAST" branch of sort_expr_for.
        let db = tmp_db().await;
        db.engine
            .raw_sql_execute(
                "INSERT INTO tracks (title, year, duration_ms, size_bytes, file_path, \
                 playlist_ids) VALUES \
                 ('a', 2020, 0, 0, '/tmp/a', '[]'), \
                 ('b', 2010, 0, 0, '/tmp/b', '[]'), \
                 ('c', NULL, 0, 0, '/tmp/c', '[]')",
                &[],
            )
            .await
            .unwrap();
        let sort = TrackSort {
            column: "year".into(),
            descending: false,
        };
        let rows = list(&db.engine, 10, 0, &Default::default(), Some(&sort))
            .await
            .unwrap();
        // NULLS LAST = NULL year shows up at position 2 (after 2010, 2020).
        assert_eq!(rows[0].title, "b");
        assert_eq!(rows[1].title, "a");
        assert_eq!(rows[2].title, "c");
    }

    #[tokio::test]
    async fn list_sort_by_artist_uses_album_artist_preference() {
        let db = tmp_db().await;
        db.engine
            .raw_sql_execute(
                "INSERT INTO tracks (title, artist, album_artist, duration_ms, size_bytes, \
                 file_path, playlist_ids) VALUES \
                 ('a', 'Z artist', 'A album_artist', 0, 0, '/tmp/a', '[]'), \
                 ('b', 'B artist', NULL, 0, 0, '/tmp/b', '[]')",
                &[],
            )
            .await
            .unwrap();
        let sort = TrackSort {
            column: "artist".into(),
            descending: false,
        };
        let rows = list(&db.engine, 10, 0, &Default::default(), Some(&sort))
            .await
            .unwrap();
        // Effective artist: 'A album_artist' < 'B artist', so a comes first.
        assert_eq!(rows[0].title, "a");
    }

    #[tokio::test]
    async fn load_local_state_map_skips_invalid_pid() {
        let db = tmp_db().await;
        db.engine
            .raw_sql_execute(
                "INSERT INTO sync_sources (id, name, source_path, path_mappings, \
                 conflict_rules, kind) VALUES (1, 's', '/s', '[]', '{}', 'itunes_itl')",
                &[],
            )
            .await
            .unwrap();
        // Two valid + one with un-parseable persistent_id.
        db.engine
            .raw_sql_execute(
                "INSERT INTO tracks (sync_source_id, persistent_id, title, duration_ms, \
                 size_bytes, file_path, playlist_ids, rating, play_count, loved) VALUES \
                 (1, '00000000deadbeef', 'a', 0, 0, '/tmp/a', '[]', 80, 5, 1), \
                 (1, '00000000feedface', 'b', 0, 0, '/tmp/b', '[]', 0, 0, 0), \
                 (1, 'NOT_HEX',         'c', 0, 0, '/tmp/c', '[]', 0, 0, 0)",
                &[],
            )
            .await
            .unwrap();
        let map = load_local_state_map(&db.engine, 1).await.unwrap();
        assert_eq!(map.len(), 2);
        let row = map.get(&0xDEAD_BEEF).unwrap();
        assert_eq!(row.rating, 80);
        assert_eq!(row.play_count, 5);
        assert!(row.loved);
    }

    /// Regression: `last_played` / `last_skipped` are TEXT timestamps
    /// written by the bump helpers. A played track used to make every
    /// later sync fail with "expected i64".
    #[tokio::test]
    async fn load_local_state_map_survives_played_tracks() {
        let db = tmp_db().await;
        db.engine
            .raw_sql_execute(
                "INSERT INTO sync_sources (id, name, source_path, path_mappings, \
                 conflict_rules, kind) VALUES (1, 's', '/s', '[]', '{}', 'itunes_itl')",
                &[],
            )
            .await
            .unwrap();
        let id: i64 = db
            .engine
            .raw_sql_scalar(
                "INSERT INTO tracks (sync_source_id, persistent_id, title, duration_ms, \
                 size_bytes, file_path, playlist_ids) VALUES \
                 (1, '00000000deadbeef', 'a', 0, 0, '/tmp/a', '[]') RETURNING id",
                &[],
            )
            .await
            .unwrap();
        bump_play_count(&db.engine, id).await.unwrap();
        bump_skip_count(&db.engine, id).await.unwrap();
        let map = load_local_state_map(&db.engine, 1).await.unwrap();
        let row = map.get(&0xDEAD_BEEF).unwrap();
        assert!(row.last_played.as_deref().is_some_and(|t| t.contains(':')));
        assert!(row.last_skipped.is_some());
        assert_eq!(row.play_count, 1);
    }

    #[tokio::test]
    async fn by_persistent_id_returns_none_when_absent() {
        let db = tmp_db().await;
        db.engine
            .raw_sql_execute(
                "INSERT INTO sync_sources (id, name, source_path, path_mappings, \
                 conflict_rules, kind) VALUES (1, 's', '/s', '[]', '{}', 'itunes_itl')",
                &[],
            )
            .await
            .unwrap();
        let res = by_persistent_id(&db.engine, 1, "ffffffffffffffff")
            .await
            .unwrap();
        assert!(res.is_none());
    }

    #[tokio::test]
    async fn update_descriptive_fields_writes_every_column() {
        let db = tmp_db().await;
        db.engine
            .raw_sql_execute(
                "INSERT INTO sync_sources (id, name, source_path, path_mappings, \
                 conflict_rules, kind) VALUES (1, 's', '/s', '[]', '{}', 'itunes_itl')",
                &[],
            )
            .await
            .unwrap();
        let upsert = ItlTrackUpsert {
            persistent_id: 0xAAAA_BBBB_CCCC_DDDD,
            sync_source_id: 1,
            title: "old",
            artist: Some("Old Artist"),
            album: Some("Old Album"),
            album_artist: Some("Old AA"),
            composer: None,
            genre: Some("Rock"),
            kind: Some("flac"),
            duration_ms: 1000,
            size_bytes: 100,
            bit_rate: Some(128),
            sample_rate: Some(44100),
            track_number: Some(1),
            disc_number: Some(1),
            year: Some(2000),
            bpm: None,
            rating: 0,
            play_count: 0,
            date_added_unix: 0,
            file_path: "/tmp/u.flac",
            original_path: None,
        };
        let id = insert_from_itl(&db.engine, &upsert).await.unwrap();

        let updated = ItlTrackUpsert {
            title: "new",
            artist: Some("New Artist"),
            album: Some("New Album"),
            year: Some(2025),
            ..upsert
        };
        update_descriptive_fields(&db.engine, id, &updated, 100, 42)
            .await
            .unwrap();

        let row = get(&db.engine, id).await.unwrap();
        assert_eq!(row.title, "new");
        assert_eq!(row.artist.as_deref(), Some("New Artist"));
        assert_eq!(row.album.as_deref(), Some("New Album"));
        assert_eq!(row.play_count, 42);
    }

    #[tokio::test]
    async fn get_errors_for_unknown_id() {
        let db = tmp_db().await;
        let err = get(&db.engine, 999_999).await.unwrap_err();
        assert!(matches!(err, TracksError::Query(_)));
    }

    #[tokio::test]
    async fn relink_file_path_fails_when_target_path_already_taken() {
        let db = tmp_db().await;
        let _a = insert_fixture(&db.engine, "Alpha", "/music/base.flac").await;
        let b = insert_fixture(&db.engine, "Bravo", "/music/base 2.flac").await;

        let err = relink_file_path(&db.engine, b, "/music/base.flac")
            .await
            .unwrap_err();
        assert!(matches!(err, TracksError::Query(_)));

        let row = get(&db.engine, b).await.unwrap();
        assert_eq!(row.file_path, "/music/base 2.flac");
    }

    /// `file_path` is globally UNIQUE, so the by-path map must see rows
    /// that belong to no sync source (Add Folder) or to another one —
    /// otherwise the reconciler INSERTs on top of them and the UNIQUE
    /// violation aborts the whole sync.
    #[tokio::test]
    async fn load_local_state_by_path_covers_rows_of_every_source() {
        let db = tmp_db().await;
        db.engine
            .raw_sql_execute(
                "INSERT INTO sync_sources (id, name, source_path, path_mappings, \
                 conflict_rules, kind) VALUES (1, 's', '/s', '[]', '{}', 'itunes_itl')",
                &[],
            )
            .await
            .unwrap();
        // One synced row, one Add Folder row (NULL sync_source_id).
        db.engine
            .raw_sql_execute(
                "INSERT INTO tracks (sync_source_id, persistent_id, title, duration_ms, \
                 size_bytes, file_path, playlist_ids, rating, play_count, loved) VALUES \
                 (1, '00000000deadbeef', 'synced', 0, 0, '/music/a.mp3', '[]', 80, 5, 0)",
                &[],
            )
            .await
            .unwrap();
        let free_id = insert_fixture(&db.engine, "added", "/music/b.mp3").await;

        let map = load_local_state_by_path(&db.engine).await.unwrap();
        assert_eq!(map.len(), 2);

        let synced = map.get("/music/a.mp3").expect("synced row present");
        assert_eq!(synced.sync_source_id, Some(1));
        assert_eq!(synced.persistent_id.as_deref(), Some("00000000deadbeef"));
        assert_eq!(synced.rating, 80);
        assert_eq!(synced.play_count, 5);

        let free = map.get("/music/b.mp3").expect("NULL-source row present");
        assert_eq!(free.id, free_id);
        assert_eq!(free.sync_source_id, None);
        assert_eq!(free.persistent_id, None);
    }

    #[tokio::test]
    async fn adopt_into_source_claims_a_null_source_row() {
        let db = tmp_db().await;
        db.engine
            .raw_sql_execute(
                "INSERT INTO sync_sources (id, name, source_path, path_mappings, \
                 conflict_rules, kind) VALUES (1, 's', '/s', '[]', '{}', 'itunes_itl')",
                &[],
            )
            .await
            .unwrap();
        let id = insert_fixture(&db.engine, "added", "/music/b.mp3").await;
        adopt_into_source(&db.engine, id, 1, "00000000feedface")
            .await
            .unwrap();
        let map = load_local_state_by_path(&db.engine).await.unwrap();
        let row = map.get("/music/b.mp3").unwrap();
        assert_eq!(row.sync_source_id, Some(1));
        assert_eq!(row.persistent_id.as_deref(), Some("00000000feedface"));
    }

    /// A relink through `update_descriptive_fields` must clear the stale
    /// verification state, or the next Verify canonicalises the old
    /// file's hash against the new file and flags it as modified.
    #[tokio::test]
    async fn update_descriptive_fields_resets_verify_state_when_the_path_moves() {
        let db = tmp_db().await;
        let id = insert_fixture(&db.engine, "t", "/music/old 2.mp3").await;
        db.engine
            .raw_sql_execute(
                "UPDATE tracks SET import_status = 'missing_source', file_hash = 'abc123' \
                 WHERE id = ?",
                &[prax_query::filter::FilterValue::Int(id)],
            )
            .await
            .unwrap();

        let mut upsert = ItlTrackUpsert {
            persistent_id: 0xDEAD_BEEF,
            sync_source_id: 1,
            title: "t",
            artist: None,
            album: None,
            album_artist: None,
            composer: None,
            genre: None,
            kind: None,
            duration_ms: 0,
            size_bytes: 0,
            bit_rate: None,
            sample_rate: None,
            track_number: None,
            disc_number: None,
            year: None,
            bpm: None,
            rating: 0,
            play_count: 0,
            date_added_unix: 0,
            file_path: "/music/old.mp3",
            original_path: None,
        };
        update_descriptive_fields(&db.engine, id, &upsert, 0, 0)
            .await
            .unwrap();
        let row = get(&db.engine, id).await.unwrap();
        assert_eq!(row.file_path, "/music/old.mp3");
        assert_eq!(row.import_status, "ok");
        assert_eq!(row.file_hash, None);

        // An update that leaves the path alone must not resurrect a row
        // Verify legitimately flagged.
        db.engine
            .raw_sql_execute(
                "UPDATE tracks SET import_status = 'missing_source', file_hash = 'abc123' \
                 WHERE id = ?",
                &[prax_query::filter::FilterValue::Int(id)],
            )
            .await
            .unwrap();
        upsert.title = "t2";
        update_descriptive_fields(&db.engine, id, &upsert, 0, 0)
            .await
            .unwrap();
        let row = get(&db.engine, id).await.unwrap();
        assert_eq!(row.title, "t2");
        assert_eq!(row.import_status, "missing_source");
        assert_eq!(row.file_hash.as_deref(), Some("abc123"));
    }
}
