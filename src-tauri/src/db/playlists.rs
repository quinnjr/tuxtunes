//! CRUD for the `playlists` table — supports ITL-sync upserts for
//! regular, smart, and folder playlists.

use crate::db::sync_util;
use prax_query::filter::FilterValue;
use prax_sqlite::raw::SqliteRawEngine;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum PlaylistKind {
    Regular,
    Smart,
    Folder,
}

impl PlaylistKind {
    fn as_str(self) -> &'static str {
        match self {
            Self::Regular => "regular",
            Self::Smart => "smart",
            Self::Folder => "folder",
        }
    }
}

#[derive(Debug, thiserror::Error)]
pub enum PlaylistsError {
    #[error("query failed: {0}")]
    Query(#[source] anyhow::Error),
    #[error("playlist {0} not found")]
    NotFound(i64),
}

pub async fn by_persistent_id(
    engine: &SqliteRawEngine,
    sync_source_id: i64,
    pid_hex: &str,
) -> Result<Option<i64>, PlaylistsError> {
    let sql = "SELECT id FROM playlists WHERE sync_source_id = ? AND persistent_id = ?";
    let params = vec![
        FilterValue::Int(sync_source_id),
        FilterValue::String(pid_hex.to_string()),
    ];
    match engine.raw_sql_optional(sql, &params).await {
        Ok(Some(r)) => Ok(r.into_json().get("id").and_then(|v| v.as_i64())),
        Ok(None) => Ok(None),
        Err(e) => Err(PlaylistsError::Query(anyhow::Error::from(e))),
    }
}

pub struct PlaylistUpsert<'a> {
    pub persistent_id: u64,
    pub sync_source_id: i64,
    pub name: &'a str,
    pub kind: PlaylistKind,
    pub parent_persistent_id: Option<u64>,
    pub sort_order: i64,
    /// For regular playlists: the ordered list of local track ids.
    pub track_entries: &'a [i64],
    /// For smart playlists: the JSON-encoded rule. None for non-smart.
    pub smart_rule_json: Option<String>,
}

pub async fn upsert(
    engine: &SqliteRawEngine,
    p: &PlaylistUpsert<'_>,
) -> Result<i64, PlaylistsError> {
    let pid_hex = sync_util::pid_hex(p.persistent_id);
    let existing = by_persistent_id(engine, p.sync_source_id, &pid_hex).await?;
    upsert_with_known_id(engine, p, existing).await
}

/// Same as `upsert`, but skips the `by_persistent_id` SELECT when the
/// caller has already resolved the local id (e.g. via a batch-loaded
/// pid→local-id map). Pass `None` when no matching row is known to
/// exist so the row is inserted.
pub async fn upsert_with_known_id(
    engine: &SqliteRawEngine,
    p: &PlaylistUpsert<'_>,
    existing: Option<i64>,
) -> Result<i64, PlaylistsError> {
    let pid_hex = sync_util::pid_hex(p.persistent_id);

    let entries_json = serde_json::to_string(p.track_entries)
        .map_err(|e| PlaylistsError::Query(anyhow::Error::from(e)))?;
    let smart_rule_fv = sync_util::opt_str(p.smart_rule_json.as_deref());

    match existing {
        Some(id) => {
            // A user rename outranks the source name from then on
            // (name_overridden is set by `rename`); everything else is
            // the source's to own.
            let sql = "UPDATE playlists SET \
                       name = CASE WHEN name_overridden = 1 THEN name ELSE ? END, \
                       kind = ?, \
                       sort_order = ?, track_entries = ?, smart_rule = ? \
                       WHERE id = ?";
            let params = vec![
                FilterValue::String(p.name.to_string()),
                FilterValue::String(p.kind.as_str().to_string()),
                FilterValue::Int(p.sort_order),
                FilterValue::String(entries_json),
                smart_rule_fv,
                FilterValue::Int(id),
            ];
            engine
                .raw_sql_execute(sql, &params)
                .await
                .map_err(|e| PlaylistsError::Query(anyhow::Error::from(e)))?;
            Ok(id)
        }
        None => {
            let sql = "INSERT INTO playlists (persistent_id, sync_source_id, \
                       name, kind, sort_order, track_entries, smart_rule) \
                       VALUES (?, ?, ?, ?, ?, ?, ?) RETURNING id";
            let params = vec![
                FilterValue::String(pid_hex),
                FilterValue::Int(p.sync_source_id),
                FilterValue::String(p.name.to_string()),
                FilterValue::String(p.kind.as_str().to_string()),
                FilterValue::Int(p.sort_order),
                FilterValue::String(entries_json),
                smart_rule_fv,
            ];
            let json_row = engine
                .raw_sql_first(sql, &params)
                .await
                .map_err(|e| PlaylistsError::Query(anyhow::Error::from(e)))?;
            json_row
                .into_json()
                .get("id")
                .and_then(|v| v.as_i64())
                .ok_or_else(|| {
                    PlaylistsError::Query(anyhow::anyhow!("INSERT ... RETURNING id missing"))
                })
        }
    }
}

/// Set the `parent_id` column based on a map of persistent_id → local id.
/// Called second-pass after every row is inserted, so folder references
/// resolve.
pub async fn link_parent(
    engine: &SqliteRawEngine,
    local_id: i64,
    parent_local_id: Option<i64>,
) -> Result<(), PlaylistsError> {
    let sql = "UPDATE playlists SET parent_id = ? WHERE id = ?";
    let params = vec![
        parent_local_id
            .map(FilterValue::Int)
            .unwrap_or(FilterValue::Null),
        FilterValue::Int(local_id),
    ];
    engine
        .raw_sql_execute(sql, &params)
        .await
        .map(|_| ())
        .map_err(|e| PlaylistsError::Query(anyhow::Error::from(e)))
}

/// Snapshot of every playlist's current `parent_id`, keyed by local id.
/// Used by the sync reconciler to detect a `parent_id` cycle before
/// committing a link (`link_parent`), without a round-trip per row.
pub async fn parent_id_map(
    engine: &SqliteRawEngine,
) -> Result<std::collections::HashMap<i64, Option<i64>>, PlaylistsError> {
    let sql = "SELECT id, parent_id FROM playlists";
    let rows = engine
        .raw_sql_query(sql, &[])
        .await
        .map_err(|e| PlaylistsError::Query(anyhow::Error::from(e)))?;
    let mut map = std::collections::HashMap::with_capacity(rows.len());
    for r in rows {
        let json = r.into_json();
        let id = json
            .get("id")
            .and_then(|v| v.as_i64())
            .ok_or_else(|| PlaylistsError::Query(anyhow::anyhow!("row missing id")))?;
        let parent_id = json.get("parent_id").and_then(|v| v.as_i64());
        map.insert(id, parent_id);
    }
    Ok(map)
}

/// User-side playlist row used by the sidebar. Excludes the rule JSON
/// from the projection so the sidebar query stays cheap; the editor
/// fetches the full rule via `get_smart_rule` when opening one.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct PlaylistRow {
    pub id: i64,
    pub name: String,
    pub kind: String,
    pub parent_id: Option<i64>,
    pub sort_order: i64,
    /// Track count cached on the row. For smart playlists this is the
    /// last-evaluated count (NULL until first evaluation).
    pub cached_track_count: Option<i64>,
    /// Some for playlists that came from a sync source (they reappear
    /// on the next sync when deleted); None for user-created ones.
    pub sync_source_id: Option<i64>,
}

/// Create a user-owned smart playlist (no sync source). Returns the
/// new row id. The rule is JSON-encoded by the caller so the DB layer
/// stays type-agnostic about the rule shape.
pub async fn create_smart(
    engine: &SqliteRawEngine,
    name: &str,
    rule_json: &str,
) -> Result<i64, PlaylistsError> {
    let sql = "INSERT INTO playlists (name, kind, sort_order, track_entries, smart_rule) \
               VALUES (?, 'smart', \
               (SELECT COALESCE(MAX(sort_order), -1) + 1 FROM playlists), '[]', ?) \
               RETURNING id";
    let params = vec![
        FilterValue::String(name.to_string()),
        FilterValue::String(rule_json.to_string()),
    ];
    let row = engine
        .raw_sql_first(sql, &params)
        .await
        .map_err(|e| PlaylistsError::Query(anyhow::Error::from(e)))?;
    row.into_json()
        .get("id")
        .and_then(|v| v.as_i64())
        .ok_or_else(|| PlaylistsError::Query(anyhow::anyhow!("INSERT ... RETURNING id missing")))
}

/// Create a user-owned regular (manual) playlist with no tracks yet,
/// optionally inside a folder. No sync source, so a re-sync never
/// touches or deletes it. New playlists append after every existing
/// sort_order so they land at the bottom of the sidebar.
pub async fn create_regular(
    engine: &SqliteRawEngine,
    name: &str,
    parent_id: Option<i64>,
) -> Result<i64, PlaylistsError> {
    let sql = "INSERT INTO playlists (name, kind, sort_order, track_entries, parent_id) \
               VALUES (?, 'regular', \
               (SELECT COALESCE(MAX(sort_order), -1) + 1 FROM playlists), '[]', ?) \
               RETURNING id";
    let params = vec![
        FilterValue::String(name.to_string()),
        parent_id.map(FilterValue::Int).unwrap_or(FilterValue::Null),
    ];
    let row = engine
        .raw_sql_first(sql, &params)
        .await
        .map_err(|e| PlaylistsError::Query(anyhow::Error::from(e)))?;
    row.into_json()
        .get("id")
        .and_then(|v| v.as_i64())
        .ok_or_else(|| PlaylistsError::Query(anyhow::anyhow!("INSERT ... RETURNING id missing")))
}

/// The user-mutable playlist predicate shared by add/remove: only
/// regular playlists the user owns. Synced ones are rewritten wholesale
/// by the next sync, so an edit there would silently vanish — refuse it
/// at the source of truth, not just in the UI.
const USER_REGULAR: &str = "kind = 'regular' AND sync_source_id IS NULL";

/// Append `track_ids` (in the given order) to a user-owned regular
/// playlist, skipping ids already present and ids that do not exist in
/// `tracks`. One atomic UPDATE — no read-modify-write window against
/// the sync worker or a concurrent user action — that also keeps
/// `cached_track_count` in step (counting only real entries).
pub async fn add_tracks(
    engine: &SqliteRawEngine,
    playlist_id: i64,
    track_ids: &[i64],
) -> Result<(), PlaylistsError> {
    // Order-preserving dedupe of the request itself; the SQL below
    // handles ids already stored on the row.
    let mut seen = std::collections::HashSet::new();
    let ids: Vec<i64> = track_ids
        .iter()
        .copied()
        .filter(|id| seen.insert(*id))
        .collect();
    let req = serde_json::to_string(&ids)
        .map_err(|e| PlaylistsError::Query(anyhow::Error::from(e)))?;
    let sql = format!(
        "UPDATE playlists SET \
         track_entries = (SELECT json_group_array(v ORDER BY ord) FROM ( \
             SELECT value AS v, key AS ord FROM json_each(playlists.track_entries) \
             UNION ALL \
             SELECT t.id, 1000000000 + r.key FROM json_each(?1) AS r \
             JOIN tracks t ON t.id = r.value \
             WHERE t.id NOT IN (SELECT value FROM json_each(playlists.track_entries)))), \
         cached_track_count = \
             (SELECT count(*) FROM json_each(playlists.track_entries)) + \
             (SELECT count(*) FROM json_each(?1) AS r \
              JOIN tracks t ON t.id = r.value \
              WHERE t.id NOT IN (SELECT value FROM json_each(playlists.track_entries))) \
         WHERE id = ?2 AND {USER_REGULAR}"
    );
    let params = vec![FilterValue::String(req), FilterValue::Int(playlist_id)];
    let n = engine
        .raw_sql_execute(&sql, &params)
        .await
        .map_err(|e| PlaylistsError::Query(anyhow::Error::from(e)))?;
    if n == 0 {
        return Err(PlaylistsError::NotFound(playlist_id));
    }
    Ok(())
}

/// Remove every occurrence of each of `track_ids` from a user-owned
/// regular playlist. Same atomicity story as `add_tracks`.
pub async fn remove_tracks(
    engine: &SqliteRawEngine,
    playlist_id: i64,
    track_ids: &[i64],
) -> Result<(), PlaylistsError> {
    let req = serde_json::to_string(track_ids)
        .map_err(|e| PlaylistsError::Query(anyhow::Error::from(e)))?;
    let sql = format!(
        "UPDATE playlists SET \
         track_entries = (SELECT json_group_array(value ORDER BY key) \
             FROM json_each(playlists.track_entries) \
             WHERE value NOT IN (SELECT value FROM json_each(?1))), \
         cached_track_count = (SELECT count(*) \
             FROM json_each(playlists.track_entries) \
             WHERE value NOT IN (SELECT value FROM json_each(?1))) \
         WHERE id = ?2 AND {USER_REGULAR}"
    );
    let params = vec![FilterValue::String(req), FilterValue::Int(playlist_id)];
    let n = engine
        .raw_sql_execute(&sql, &params)
        .await
        .map_err(|e| PlaylistsError::Query(anyhow::Error::from(e)))?;
    if n == 0 {
        return Err(PlaylistsError::NotFound(playlist_id));
    }
    Ok(())
}

/// Drop `track_id` from every regular playlist that references it.
/// Called when a track leaves the library so no playlist keeps a
/// dangling id — which SQLite could later hand to a brand-new track via
/// rowid reuse. Synced playlists are pruned too: the file is gone
/// locally either way, and the next sync rebuilds their entries.
pub async fn prune_track(engine: &SqliteRawEngine, track_id: i64) -> Result<(), PlaylistsError> {
    let sql = "UPDATE playlists SET \
               track_entries = (SELECT json_group_array(value ORDER BY key) \
                   FROM json_each(playlists.track_entries) WHERE value <> ?1), \
               cached_track_count = (SELECT count(*) \
                   FROM json_each(playlists.track_entries) WHERE value <> ?1) \
               WHERE kind = 'regular' AND EXISTS \
                   (SELECT 1 FROM json_each(playlists.track_entries) WHERE value = ?1)";
    engine
        .raw_sql_execute(sql, &[FilterValue::Int(track_id)])
        .await
        .map(|_| ())
        .map_err(|e| PlaylistsError::Query(anyhow::Error::from(e)))
}

/// Update an existing smart playlist's rule. The DB doesn't validate
/// the JSON — the caller (commands::smart) round-trips it through
/// SmartRule serde first.
pub async fn update_smart_rule(
    engine: &SqliteRawEngine,
    playlist_id: i64,
    rule_json: &str,
) -> Result<(), PlaylistsError> {
    let sql = "UPDATE playlists SET smart_rule = ? WHERE id = ? AND kind = 'smart'";
    let params = vec![
        FilterValue::String(rule_json.to_string()),
        FilterValue::Int(playlist_id),
    ];
    engine
        .raw_sql_execute(sql, &params)
        .await
        .map(|_| ())
        .map_err(|e| PlaylistsError::Query(anyhow::Error::from(e)))
}

/// Rename any playlist. Marks the name as user-overridden so a synced
/// playlist keeps the local name across future syncs (`upsert` skips
/// the name column for overridden rows).
pub async fn rename(
    engine: &SqliteRawEngine,
    playlist_id: i64,
    name: &str,
) -> Result<(), PlaylistsError> {
    let sql = "UPDATE playlists SET name = ?, name_overridden = 1 WHERE id = ?";
    let params = vec![
        FilterValue::String(name.to_string()),
        FilterValue::Int(playlist_id),
    ];
    let n = engine
        .raw_sql_execute(sql, &params)
        .await
        .map_err(|e| PlaylistsError::Query(anyhow::Error::from(e)))?;
    if n == 0 {
        return Err(PlaylistsError::NotFound(playlist_id));
    }
    Ok(())
}

/// Read back the rule JSON for a smart playlist. Returns Ok(None) for
/// non-smart playlists or unknown ids.
///
/// `smart_rule` is a JSON column. prax-sqlite may surface it either as
/// a raw JSON string (when SQLite stored it as TEXT) or as an already-
/// parsed Value (when SQLite recognized JSON1). Re-serialize the
/// non-string case so callers always get the canonical JSON text.
pub async fn get_smart_rule(
    engine: &SqliteRawEngine,
    playlist_id: i64,
) -> Result<Option<String>, PlaylistsError> {
    let sql = "SELECT smart_rule FROM playlists WHERE id = ? AND kind = 'smart'";
    let params = vec![FilterValue::Int(playlist_id)];
    let row = match engine.raw_sql_optional(sql, &params).await {
        Ok(Some(r)) => r,
        Ok(None) => return Ok(None),
        Err(e) => return Err(PlaylistsError::Query(anyhow::Error::from(e))),
    };
    let cell = row.into_json().get("smart_rule").cloned();
    let parsed = match cell {
        None | Some(serde_json::Value::Null) => None,
        Some(serde_json::Value::String(s)) => Some(s),
        Some(other) => Some(
            serde_json::to_string(&other)
                .map_err(|e| PlaylistsError::Query(anyhow::Error::from(e)))?,
        ),
    };
    Ok(parsed)
}

/// List every playlist for the sidebar. Ordered by sort_order then
/// name so the user sees a stable presentation.
pub async fn list_all(engine: &SqliteRawEngine) -> Result<Vec<PlaylistRow>, PlaylistsError> {
    let sql = "SELECT id, name, kind, parent_id, sort_order, cached_track_count, \
               sync_source_id \
               FROM playlists \
               ORDER BY sort_order ASC, name COLLATE NOCASE ASC";
    let rows = engine
        .raw_sql_query(sql, &[])
        .await
        .map_err(|e| PlaylistsError::Query(anyhow::Error::from(e)))?;
    rows.into_iter()
        .map(|r| serde_json::from_value(r.into_json()))
        .collect::<Result<Vec<_>, _>>()
        .map_err(|e| PlaylistsError::Query(anyhow::Error::from(e)))
}

/// Resolve a regular playlist's ordered `track_entries` into full track
/// rows. Order (and duplicates) follow the playlist, not the DB; ids
/// that no longer exist in `tracks` are silently dropped. Returns an
/// empty Vec for folders/smart playlists (their `track_entries` is
/// `[]`). Fetches in chunks to stay well under SQLite's bind limit on
/// very long playlists.
pub async fn tracks_for_regular(
    engine: &SqliteRawEngine,
    playlist_id: i64,
) -> Result<Vec<crate::db::tracks::TrackRow>, PlaylistsError> {
    use crate::db::tracks::TrackRow;
    use std::collections::HashMap;

    let sql = "SELECT track_entries FROM playlists WHERE id = ?";
    let row = engine
        .raw_sql_optional(sql, &[FilterValue::Int(playlist_id)])
        .await
        .map_err(|e| PlaylistsError::Query(anyhow::Error::from(e)))?
        .ok_or(PlaylistsError::NotFound(playlist_id))?;
    let entries_val = row
        .into_json()
        .get("track_entries")
        .cloned()
        .unwrap_or(serde_json::Value::Null);
    let ids: Vec<i64> = match entries_val {
        serde_json::Value::String(s) => {
            serde_json::from_str(&s).map_err(|e| PlaylistsError::Query(anyhow::Error::from(e)))?
        }
        serde_json::Value::Array(_) => serde_json::from_value(entries_val)
            .map_err(|e| PlaylistsError::Query(anyhow::Error::from(e)))?,
        _ => Vec::new(),
    };
    if ids.is_empty() {
        return Ok(Vec::new());
    }

    const CHUNK: usize = 500;
    let mut by_id: HashMap<i64, TrackRow> = HashMap::with_capacity(ids.len());
    let mut unique: Vec<i64> = ids.clone();
    unique.sort_unstable();
    unique.dedup();
    for chunk in unique.chunks(CHUNK) {
        let placeholders = vec!["?"; chunk.len()].join(", ");
        let columns = crate::db::tracks::TRACK_ROW_COLUMNS;
        let sql = format!("SELECT {columns} FROM tracks WHERE id IN ({placeholders})");
        let params: Vec<FilterValue> = chunk.iter().map(|id| FilterValue::Int(*id)).collect();
        let rows = engine
            .raw_sql_query(&sql, &params)
            .await
            .map_err(|e| PlaylistsError::Query(anyhow::Error::from(e)))?;
        for r in rows {
            let t: TrackRow = serde_json::from_value(r.into_json())
                .map_err(|e| PlaylistsError::Query(anyhow::Error::from(e)))?;
            by_id.insert(t.id, t);
        }
    }
    Ok(ids.iter().filter_map(|id| by_id.get(id).cloned()).collect())
}

/// Hard-delete a playlist by id. A sync-sourced playlist leaves a
/// tombstone behind so the reconciler never re-imports it — the delete
/// is permanent, same as for user-created playlists.
pub async fn delete(engine: &SqliteRawEngine, playlist_id: i64) -> Result<(), PlaylistsError> {
    let tombstone_sql = "INSERT OR IGNORE INTO playlist_tombstones \
                         (sync_source_id, persistent_id) \
                         SELECT sync_source_id, persistent_id FROM playlists \
                         WHERE id = ? AND sync_source_id IS NOT NULL \
                         AND persistent_id IS NOT NULL";
    engine
        .raw_sql_execute(tombstone_sql, &[FilterValue::Int(playlist_id)])
        .await
        .map_err(|e| PlaylistsError::Query(anyhow::Error::from(e)))?;
    let sql = "DELETE FROM playlists WHERE id = ?";
    engine
        .raw_sql_execute(sql, &[FilterValue::Int(playlist_id)])
        .await
        .map(|_| ())
        .map_err(|e| PlaylistsError::Query(anyhow::Error::from(e)))
}

/// Persistent ids (parsed from their hex form) of playlists the user
/// deleted from `source_id`. The reconciler skips these entirely.
pub async fn tombstoned_pids(
    engine: &SqliteRawEngine,
    source_id: i64,
) -> Result<std::collections::HashSet<u64>, PlaylistsError> {
    let sql = "SELECT persistent_id FROM playlist_tombstones WHERE sync_source_id = ?";
    let rows = engine
        .raw_sql_query(sql, &[FilterValue::Int(source_id)])
        .await
        .map_err(|e| PlaylistsError::Query(anyhow::Error::from(e)))?;
    let mut pids = std::collections::HashSet::with_capacity(rows.len());
    for r in rows {
        let json = r.into_json();
        let hex = json
            .get("persistent_id")
            .and_then(|v| v.as_str())
            .ok_or_else(|| PlaylistsError::Query(anyhow::anyhow!("tombstone row missing pid")))?
            .to_string();
        let pid = u64::from_str_radix(&hex, 16)
            .map_err(|e| PlaylistsError::Query(anyhow::Error::from(e)))?;
        pids.insert(pid);
    }
    Ok(pids)
}

/// Update the cached track-count for a smart playlist after a fresh
/// evaluation. Skipped silently for non-smart rows so the caller can
/// always invoke this in the evaluator.
pub async fn set_cached_count(
    engine: &SqliteRawEngine,
    playlist_id: i64,
    count: i64,
) -> Result<(), PlaylistsError> {
    let sql = "UPDATE playlists SET cached_track_count = ?, cached_at = CURRENT_TIMESTAMP \
               WHERE id = ?";
    let params = vec![FilterValue::Int(count), FilterValue::Int(playlist_id)];
    engine
        .raw_sql_execute(sql, &params)
        .await
        .map(|_| ())
        .map_err(|e| PlaylistsError::Query(anyhow::Error::from(e)))
}

/// Delete playlists in `sync_source_id` whose `persistent_id` is not in
/// `keep`.
pub async fn delete_missing(
    engine: &SqliteRawEngine,
    sync_source_id: i64,
    keep: &[u64],
) -> Result<u64, PlaylistsError> {
    sync_util::delete_by_keep_set(engine, "playlists", sync_source_id, keep)
        .await
        .map_err(|e| PlaylistsError::Query(anyhow::Error::from(e)))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::Db;

    async fn tmp() -> Db {
        let tmp = tempfile::NamedTempFile::new().unwrap();
        let db = Db::open(tmp.path()).await.unwrap();
        db.engine
            .raw_sql_execute(
                "INSERT INTO sync_sources (id, name, source_path, path_mappings, \
                 conflict_rules, kind) VALUES (1, 'x', '/x', '[]', '{}', 'itunes_itl')",
                &[],
            )
            .await
            .unwrap();
        db
    }

    async fn insert_track(db: &Db, title: &str) -> i64 {
        db.engine
            .raw_sql_scalar(
                "INSERT INTO tracks (title, duration_ms, size_bytes, file_path, playlist_ids) \
                 VALUES (?, 1000, 0, ?, '[]') RETURNING id",
                &[
                    FilterValue::String(title.to_string()),
                    FilterValue::String(format!("/tmp/{title}.flac")),
                ],
            )
            .await
            .unwrap()
    }

    #[tokio::test]
    async fn tracks_for_regular_preserves_playlist_order_and_duplicates() {
        let db = tmp().await;
        let a = insert_track(&db, "a").await;
        let b = insert_track(&db, "b").await;
        let c = insert_track(&db, "c").await;
        let entries = [c, a, c, 9_999_999, b];
        let u = PlaylistUpsert {
            persistent_id: 0x1111_2222_3333_4444,
            sync_source_id: 1,
            name: "Ordered",
            kind: PlaylistKind::Regular,
            parent_persistent_id: None,
            sort_order: 0,
            track_entries: &entries,
            smart_rule_json: None,
        };
        let id = upsert(&db.engine, &u).await.unwrap();
        let rows = tracks_for_regular(&db.engine, id).await.unwrap();
        let titles: Vec<&str> = rows.iter().map(|r| r.title.as_str()).collect();
        // Playlist order kept, duplicate kept, dangling id dropped.
        assert_eq!(titles, ["c", "a", "c", "b"]);
    }

    #[tokio::test]
    async fn tracks_for_regular_errors_on_unknown_playlist() {
        let db = tmp().await;
        let err = tracks_for_regular(&db.engine, 424_242).await.unwrap_err();
        assert!(matches!(err, PlaylistsError::NotFound(424_242)), "{err}");
    }

    #[tokio::test]
    async fn tracks_for_regular_handles_long_playlists_across_chunks() {
        let db = tmp().await;
        let mut ids = Vec::new();
        for i in 0..1_203 {
            ids.push(insert_track(&db, &format!("t{i}")).await);
        }
        ids.reverse();
        let u = PlaylistUpsert {
            persistent_id: 0x5555_6666_7777_8888,
            sync_source_id: 1,
            name: "Long",
            kind: PlaylistKind::Regular,
            parent_persistent_id: None,
            sort_order: 0,
            track_entries: &ids,
            smart_rule_json: None,
        };
        let id = upsert(&db.engine, &u).await.unwrap();
        let rows = tracks_for_regular(&db.engine, id).await.unwrap();
        let got: Vec<i64> = rows.iter().map(|r| r.id).collect();
        assert_eq!(got, ids);
    }

    #[tokio::test]
    async fn rename_updates_name_and_errors_for_unknown_id() {
        let db = tmp().await;
        let id = create_smart(&db.engine, "Old", "{}").await.unwrap();
        rename(&db.engine, id, "New").await.unwrap();
        let rows = list_all(&db.engine).await.unwrap();
        assert_eq!(rows.iter().find(|r| r.id == id).unwrap().name, "New");
        let err = rename(&db.engine, 424_242, "x").await.unwrap_err();
        assert!(matches!(err, PlaylistsError::NotFound(424_242)), "{err}");
    }

    #[tokio::test]
    async fn insert_then_update_via_upsert() {
        let db = tmp().await;
        let u = PlaylistUpsert {
            persistent_id: 0xBEEF_BEEF_BEEF_BEEF,
            sync_source_id: 1,
            name: "Heavy",
            kind: PlaylistKind::Regular,
            parent_persistent_id: None,
            sort_order: 0,
            track_entries: &[10, 11, 12],
            smart_rule_json: None,
        };
        let id1 = upsert(&db.engine, &u).await.unwrap();
        // Re-upsert with a new name.
        let u2 = PlaylistUpsert {
            name: "Heavier",
            track_entries: &[10, 11, 12, 13],
            ..u
        };
        let id2 = upsert(&db.engine, &u2).await.unwrap();
        assert_eq!(id1, id2, "upsert should reuse the row");
    }

    #[tokio::test]
    async fn upsert_with_known_id_inserts_when_none_and_updates_when_some() {
        let db = tmp().await;
        let u = PlaylistUpsert {
            persistent_id: 0xABCD_ABCD_ABCD_ABCD,
            sync_source_id: 1,
            name: "Known",
            kind: PlaylistKind::Regular,
            parent_persistent_id: None,
            sort_order: 0,
            track_entries: &[1, 2],
            smart_rule_json: None,
        };
        // Pre-resolved id of None → behaves like an insert.
        let id1 = upsert_with_known_id(&db.engine, &u, None).await.unwrap();

        // Pre-resolved id of Some(id1) → behaves like an update, and
        // must not touch the row's id.
        let u2 = PlaylistUpsert {
            name: "Known Updated",
            track_entries: &[1, 2, 3],
            ..u
        };
        let id2 = upsert_with_known_id(&db.engine, &u2, Some(id1))
            .await
            .unwrap();
        assert_eq!(id1, id2, "known-id path should update the same row");

        // Confirm the update actually landed and no duplicate row was
        // created via the pre-resolved-id path.
        let rows = list_all(&db.engine).await.unwrap();
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].name, "Known Updated");
    }

    #[tokio::test]
    async fn link_parent_sets_the_column() {
        let db = tmp().await;
        let child = upsert(
            &db.engine,
            &PlaylistUpsert {
                persistent_id: 1,
                sync_source_id: 1,
                name: "Child",
                kind: PlaylistKind::Smart,
                parent_persistent_id: Some(2),
                sort_order: 0,
                track_entries: &[],
                smart_rule_json: Some(r#"{"x":1}"#.into()),
            },
        )
        .await
        .unwrap();
        let parent = upsert(
            &db.engine,
            &PlaylistUpsert {
                persistent_id: 2,
                sync_source_id: 1,
                name: "Folder",
                kind: PlaylistKind::Folder,
                parent_persistent_id: None,
                sort_order: 0,
                track_entries: &[],
                smart_rule_json: None,
            },
        )
        .await
        .unwrap();
        link_parent(&db.engine, child, Some(parent)).await.unwrap();

        let check: i64 = db
            .engine
            .raw_sql_scalar(
                "SELECT parent_id FROM playlists WHERE id = ?",
                &[FilterValue::Int(child)],
            )
            .await
            .unwrap();
        assert_eq!(check, parent);
    }

    #[tokio::test]
    async fn link_parent_none_clears_the_column() {
        let db = tmp().await;
        let child = upsert(
            &db.engine,
            &PlaylistUpsert {
                persistent_id: 1,
                sync_source_id: 1,
                name: "Child",
                kind: PlaylistKind::Smart,
                parent_persistent_id: Some(2),
                sort_order: 0,
                track_entries: &[],
                smart_rule_json: Some(r#"{"x":1}"#.into()),
            },
        )
        .await
        .unwrap();
        let parent = upsert(
            &db.engine,
            &PlaylistUpsert {
                persistent_id: 2,
                sync_source_id: 1,
                name: "Folder",
                kind: PlaylistKind::Folder,
                parent_persistent_id: None,
                sort_order: 0,
                track_entries: &[],
                smart_rule_json: None,
            },
        )
        .await
        .unwrap();
        link_parent(&db.engine, child, Some(parent)).await.unwrap();
        let before: Option<i64> = db
            .engine
            .raw_sql_scalar(
                "SELECT parent_id FROM playlists WHERE id = ?",
                &[FilterValue::Int(child)],
            )
            .await
            .unwrap();
        assert_eq!(before, Some(parent));

        // Clearing a stale parent link (e.g. the source no longer
        // reports a parent for this playlist).
        link_parent(&db.engine, child, None).await.unwrap();
        let after: Option<i64> = db
            .engine
            .raw_sql_scalar(
                "SELECT parent_id FROM playlists WHERE id = ?",
                &[FilterValue::Int(child)],
            )
            .await
            .unwrap();
        assert_eq!(after, None);
    }

    #[tokio::test]
    async fn parent_id_map_reflects_current_links() {
        let db = tmp().await;
        let parent = upsert(
            &db.engine,
            &PlaylistUpsert {
                persistent_id: 1,
                sync_source_id: 1,
                name: "Folder",
                kind: PlaylistKind::Folder,
                parent_persistent_id: None,
                sort_order: 0,
                track_entries: &[],
                smart_rule_json: None,
            },
        )
        .await
        .unwrap();
        let child = upsert(
            &db.engine,
            &PlaylistUpsert {
                persistent_id: 2,
                sync_source_id: 1,
                name: "Child",
                kind: PlaylistKind::Regular,
                parent_persistent_id: None,
                sort_order: 0,
                track_entries: &[],
                smart_rule_json: None,
            },
        )
        .await
        .unwrap();
        link_parent(&db.engine, child, Some(parent)).await.unwrap();

        let map = parent_id_map(&db.engine).await.unwrap();
        assert_eq!(map.get(&parent).copied(), Some(None));
        assert_eq!(map.get(&child).copied(), Some(Some(parent)));
    }

    #[tokio::test]
    async fn create_smart_then_get_rule_roundtrips() {
        let db = tmp().await;
        let id = create_smart(&db.engine, "Top Plays", r#"{"any":true}"#)
            .await
            .unwrap();
        assert!(id > 0);
        let stored = get_smart_rule(&db.engine, id).await.unwrap();
        assert_eq!(stored.as_deref(), Some(r#"{"any":true}"#));
    }

    #[tokio::test]
    async fn update_smart_rule_replaces_value() {
        let db = tmp().await;
        let id = create_smart(&db.engine, "x", r#"{"a":1}"#).await.unwrap();
        update_smart_rule(&db.engine, id, r#"{"a":2}"#)
            .await
            .unwrap();
        assert_eq!(
            get_smart_rule(&db.engine, id).await.unwrap().as_deref(),
            Some(r#"{"a":2}"#)
        );
    }

    #[tokio::test]
    async fn list_all_returns_user_and_synced_playlists() {
        let db = tmp().await;
        // Synced (kind=regular).
        upsert(
            &db.engine,
            &PlaylistUpsert {
                persistent_id: 1,
                sync_source_id: 1,
                name: "Synced",
                kind: PlaylistKind::Regular,
                parent_persistent_id: None,
                sort_order: 0,
                track_entries: &[],
                smart_rule_json: None,
            },
        )
        .await
        .unwrap();
        // User-created (kind=smart).
        create_smart(&db.engine, "Mine", r#"{}"#).await.unwrap();
        let rows = list_all(&db.engine).await.unwrap();
        assert_eq!(rows.len(), 2);
        // Synced rows carry their source id; user-created rows don't.
        let synced = rows.iter().find(|r| r.name == "Synced").unwrap();
        assert_eq!(synced.sync_source_id, Some(1));
        let mine = rows.iter().find(|r| r.name == "Mine").unwrap();
        assert_eq!(mine.sync_source_id, None);
    }

    #[tokio::test]
    async fn delete_removes_the_row() {
        let db = tmp().await;
        let id = create_smart(&db.engine, "to_delete", r#"{}"#)
            .await
            .unwrap();
        delete(&db.engine, id).await.unwrap();
        assert!(get_smart_rule(&db.engine, id).await.unwrap().is_none());
    }

    #[tokio::test]
    async fn set_cached_count_writes_columns() {
        let db = tmp().await;
        let id = create_smart(&db.engine, "x", r#"{}"#).await.unwrap();
        set_cached_count(&db.engine, id, 42).await.unwrap();
        let count: i64 = db
            .engine
            .raw_sql_scalar(
                "SELECT cached_track_count FROM playlists WHERE id = ?",
                &[FilterValue::Int(id)],
            )
            .await
            .unwrap();
        assert_eq!(count, 42);
    }

    #[tokio::test]
    async fn get_smart_rule_for_non_smart_row_returns_none() {
        let db = tmp().await;
        // Regular playlist, not smart — get_smart_rule must return None
        // because the WHERE clause filters on kind='smart'.
        let id = upsert(
            &db.engine,
            &PlaylistUpsert {
                persistent_id: 1,
                sync_source_id: 1,
                name: "regular",
                kind: PlaylistKind::Regular,
                parent_persistent_id: None,
                sort_order: 0,
                track_entries: &[],
                smart_rule_json: None,
            },
        )
        .await
        .unwrap();
        assert!(get_smart_rule(&db.engine, id).await.unwrap().is_none());
    }

    #[tokio::test]
    async fn get_smart_rule_for_unknown_id_returns_none() {
        let db = tmp().await;
        assert!(get_smart_rule(&db.engine, 9999).await.unwrap().is_none());
    }

    #[test]
    fn playlists_error_display_works() {
        let e = PlaylistsError::Query(anyhow::anyhow!("kaput"));
        assert!(e.to_string().contains("kaput"));
    }

    #[tokio::test]
    async fn create_regular_inserts_empty_user_playlist() {
        let db = tmp().await;
        let id = create_regular(&db.engine, "Mixtape", None).await.unwrap();
        assert!(id > 0);
        let rows = list_all(&db.engine).await.unwrap();
        let row = rows.iter().find(|r| r.id == id).unwrap();
        assert_eq!(row.name, "Mixtape");
        assert_eq!(row.kind, "regular");
        assert!(tracks_for_regular(&db.engine, id).await.unwrap().is_empty());
    }

    #[tokio::test]
    async fn create_regular_appends_after_existing_sort_orders() {
        let db = tmp().await;
        upsert(
            &db.engine,
            &PlaylistUpsert {
                persistent_id: 1,
                sync_source_id: 1,
                name: "Synced",
                kind: PlaylistKind::Regular,
                parent_persistent_id: None,
                sort_order: 7,
                track_entries: &[],
                smart_rule_json: None,
            },
        )
        .await
        .unwrap();
        let a = create_regular(&db.engine, "A", None).await.unwrap();
        let b = create_regular(&db.engine, "B", None).await.unwrap();
        let rows = list_all(&db.engine).await.unwrap();
        let so = |id: i64| rows.iter().find(|r| r.id == id).unwrap().sort_order;
        assert_eq!(so(a), 8, "first user playlist goes after the synced max");
        assert_eq!(so(b), 9, "second keeps creation order");
    }

    #[tokio::test]
    async fn create_regular_sets_parent_when_given() {
        let db = tmp().await;
        let folder = create_regular(&db.engine, "Folder-ish", None).await.unwrap();
        let child = create_regular(&db.engine, "Child", Some(folder))
            .await
            .unwrap();
        let rows = list_all(&db.engine).await.unwrap();
        assert_eq!(
            rows.iter().find(|r| r.id == child).unwrap().parent_id,
            Some(folder)
        );
    }

    #[tokio::test]
    async fn add_tracks_appends_in_order_and_updates_cached_count() {
        let db = tmp().await;
        let a = insert_track(&db, "a").await;
        let b = insert_track(&db, "b").await;
        let c = insert_track(&db, "c").await;
        let id = create_regular(&db.engine, "Mix", None).await.unwrap();
        add_tracks(&db.engine, id, &[b, a]).await.unwrap();
        add_tracks(&db.engine, id, &[c]).await.unwrap();
        let rows = tracks_for_regular(&db.engine, id).await.unwrap();
        let got: Vec<i64> = rows.iter().map(|r| r.id).collect();
        assert_eq!(got, [b, a, c]);
        let listed = list_all(&db.engine).await.unwrap();
        assert_eq!(
            listed.iter().find(|r| r.id == id).unwrap().cached_track_count,
            Some(3)
        );
    }

    #[tokio::test]
    async fn add_tracks_skips_ids_already_present_and_within_the_batch() {
        let db = tmp().await;
        let a = insert_track(&db, "a").await;
        let b = insert_track(&db, "b").await;
        let id = create_regular(&db.engine, "Mix", None).await.unwrap();
        add_tracks(&db.engine, id, &[a, a, b]).await.unwrap();
        add_tracks(&db.engine, id, &[b, a]).await.unwrap();
        let rows = tracks_for_regular(&db.engine, id).await.unwrap();
        let got: Vec<i64> = rows.iter().map(|r| r.id).collect();
        assert_eq!(got, [a, b], "no duplicates from repeated adds");
    }

    #[tokio::test]
    async fn add_tracks_ignores_track_ids_that_do_not_exist() {
        let db = tmp().await;
        let a = insert_track(&db, "a").await;
        let id = create_regular(&db.engine, "Mix", None).await.unwrap();
        add_tracks(&db.engine, id, &[9_999_999, a]).await.unwrap();
        let rows = tracks_for_regular(&db.engine, id).await.unwrap();
        assert_eq!(rows.iter().map(|r| r.id).collect::<Vec<_>>(), [a]);
        let listed = list_all(&db.engine).await.unwrap();
        assert_eq!(
            listed.iter().find(|r| r.id == id).unwrap().cached_track_count,
            Some(1),
            "cached count must not include dangling ids"
        );
    }

    #[tokio::test]
    async fn add_tracks_errors_for_unknown_or_non_regular_playlist() {
        let db = tmp().await;
        let err = add_tracks(&db.engine, 424_242, &[1]).await.unwrap_err();
        assert!(matches!(err, PlaylistsError::NotFound(424_242)), "{err}");
        let smart = create_smart(&db.engine, "s", "{}").await.unwrap();
        let err = add_tracks(&db.engine, smart, &[1]).await.unwrap_err();
        assert!(matches!(err, PlaylistsError::NotFound(_)), "{err}");
    }

    #[tokio::test]
    async fn add_and_remove_reject_synced_playlists() {
        let db = tmp().await;
        let synced = upsert(
            &db.engine,
            &PlaylistUpsert {
                persistent_id: 1,
                sync_source_id: 1,
                name: "Synced",
                kind: PlaylistKind::Regular,
                parent_persistent_id: None,
                sort_order: 0,
                track_entries: &[1, 2],
                smart_rule_json: None,
            },
        )
        .await
        .unwrap();
        let err = add_tracks(&db.engine, synced, &[3]).await.unwrap_err();
        assert!(matches!(err, PlaylistsError::NotFound(_)), "{err}");
        let err = remove_tracks(&db.engine, synced, &[1]).await.unwrap_err();
        assert!(matches!(err, PlaylistsError::NotFound(_)), "{err}");
    }

    #[tokio::test]
    async fn remove_tracks_drops_every_occurrence() {
        let db = tmp().await;
        let a = insert_track(&db, "a").await;
        let b = insert_track(&db, "b").await;
        let id = create_regular(&db.engine, "Mix", None).await.unwrap();
        // Duplicates can only come from legacy data now that add_tracks
        // dedupes; seed them directly.
        db.engine
            .raw_sql_execute(
                "UPDATE playlists SET track_entries = ? WHERE id = ?",
                &[
                    FilterValue::String(format!("[{a},{b},{a}]")),
                    FilterValue::Int(id),
                ],
            )
            .await
            .unwrap();
        remove_tracks(&db.engine, id, &[a]).await.unwrap();
        let rows = tracks_for_regular(&db.engine, id).await.unwrap();
        let got: Vec<i64> = rows.iter().map(|r| r.id).collect();
        assert_eq!(got, [b]);
        let listed = list_all(&db.engine).await.unwrap();
        assert_eq!(
            listed.iter().find(|r| r.id == id).unwrap().cached_track_count,
            Some(1)
        );
    }

    #[tokio::test]
    async fn remove_tracks_can_empty_a_playlist() {
        let db = tmp().await;
        let a = insert_track(&db, "a").await;
        let id = create_regular(&db.engine, "Mix", None).await.unwrap();
        add_tracks(&db.engine, id, &[a]).await.unwrap();
        remove_tracks(&db.engine, id, &[a]).await.unwrap();
        assert!(tracks_for_regular(&db.engine, id).await.unwrap().is_empty());
        // A later add must still work against the emptied entries.
        add_tracks(&db.engine, id, &[a]).await.unwrap();
        assert_eq!(tracks_for_regular(&db.engine, id).await.unwrap().len(), 1);
    }

    #[tokio::test]
    async fn remove_tracks_errors_for_unknown_playlist() {
        let db = tmp().await;
        let err = remove_tracks(&db.engine, 424_242, &[1]).await.unwrap_err();
        assert!(matches!(err, PlaylistsError::NotFound(424_242)), "{err}");
    }

    #[tokio::test]
    async fn prune_track_removes_it_from_every_regular_playlist() {
        let db = tmp().await;
        let a = insert_track(&db, "a").await;
        let b = insert_track(&db, "b").await;
        let p1 = create_regular(&db.engine, "One", None).await.unwrap();
        let p2 = create_regular(&db.engine, "Two", None).await.unwrap();
        add_tracks(&db.engine, p1, &[a, b]).await.unwrap();
        add_tracks(&db.engine, p2, &[a]).await.unwrap();
        prune_track(&db.engine, a).await.unwrap();
        let one: Vec<i64> = tracks_for_regular(&db.engine, p1)
            .await
            .unwrap()
            .iter()
            .map(|r| r.id)
            .collect();
        assert_eq!(one, [b]);
        assert!(tracks_for_regular(&db.engine, p2).await.unwrap().is_empty());
        let listed = list_all(&db.engine).await.unwrap();
        assert_eq!(
            listed.iter().find(|r| r.id == p1).unwrap().cached_track_count,
            Some(1)
        );
        assert_eq!(
            listed.iter().find(|r| r.id == p2).unwrap().cached_track_count,
            Some(0)
        );
    }

    #[tokio::test]
    async fn user_rename_survives_a_sync_upsert() {
        let db = tmp().await;
        let u = PlaylistUpsert {
            persistent_id: 0xAAAA_BBBB_CCCC_DDDD,
            sync_source_id: 1,
            name: "Gym",
            kind: PlaylistKind::Regular,
            parent_persistent_id: None,
            sort_order: 0,
            track_entries: &[],
            smart_rule_json: None,
        };
        let id = upsert(&db.engine, &u).await.unwrap();
        rename(&db.engine, id, "Workout").await.unwrap();
        // Next sync re-upserts the source row with its original name…
        upsert(&db.engine, &u).await.unwrap();
        let rows = list_all(&db.engine).await.unwrap();
        // …but the user's rename wins.
        assert_eq!(rows.iter().find(|r| r.id == id).unwrap().name, "Workout");
    }

    #[tokio::test]
    async fn sync_still_renames_rows_the_user_never_touched() {
        let db = tmp().await;
        let u = PlaylistUpsert {
            persistent_id: 0xAAAA_BBBB_CCCC_DDDD,
            sync_source_id: 1,
            name: "Gym",
            kind: PlaylistKind::Regular,
            parent_persistent_id: None,
            sort_order: 0,
            track_entries: &[],
            smart_rule_json: None,
        };
        let id = upsert(&db.engine, &u).await.unwrap();
        let renamed = PlaylistUpsert { name: "Gym 2024", ..u };
        upsert(&db.engine, &renamed).await.unwrap();
        let rows = list_all(&db.engine).await.unwrap();
        assert_eq!(rows.iter().find(|r| r.id == id).unwrap().name, "Gym 2024");
    }

    #[tokio::test]
    async fn deleting_a_synced_playlist_records_a_tombstone() {
        let db = tmp().await;
        let pid = 0x1234_5678_9ABC_DEF0u64;
        let u = PlaylistUpsert {
            persistent_id: pid,
            sync_source_id: 1,
            name: "Synced",
            kind: PlaylistKind::Regular,
            parent_persistent_id: None,
            sort_order: 0,
            track_entries: &[],
            smart_rule_json: None,
        };
        let id = upsert(&db.engine, &u).await.unwrap();
        delete(&db.engine, id).await.unwrap();
        assert!(list_all(&db.engine).await.unwrap().is_empty());
        let dead = tombstoned_pids(&db.engine, 1).await.unwrap();
        assert!(dead.contains(&pid));
    }

    #[tokio::test]
    async fn deleting_a_user_playlist_records_no_tombstone() {
        let db = tmp().await;
        let id = create_regular(&db.engine, "Mine", None).await.unwrap();
        delete(&db.engine, id).await.unwrap();
        assert!(tombstoned_pids(&db.engine, 1).await.unwrap().is_empty());
    }

    #[tokio::test]
    async fn tombstoned_pids_scopes_by_source() {
        let db = tmp().await;
        db.engine
            .raw_sql_execute(
                "INSERT INTO sync_sources (id, name, source_path, path_mappings, \
                 conflict_rules, kind) VALUES (2, 'y', '/y', '[]', '{}', 'itunes_itl')",
                &[],
            )
            .await
            .unwrap();
        for (source, pid) in [(1i64, 0x1u64), (2i64, 0x2u64)] {
            let u = PlaylistUpsert {
                persistent_id: pid,
                sync_source_id: source,
                name: "p",
                kind: PlaylistKind::Regular,
                parent_persistent_id: None,
                sort_order: 0,
                track_entries: &[],
                smart_rule_json: None,
            };
            let id = upsert(&db.engine, &u).await.unwrap();
            delete(&db.engine, id).await.unwrap();
        }
        assert_eq!(
            tombstoned_pids(&db.engine, 1).await.unwrap(),
            std::collections::HashSet::from([0x1u64])
        );
    }

    #[tokio::test]
    async fn delete_missing_drops_unlisted() {
        let db = tmp().await;
        for i in 1u64..=3 {
            upsert(
                &db.engine,
                &PlaylistUpsert {
                    persistent_id: i,
                    sync_source_id: 1,
                    name: "p",
                    kind: PlaylistKind::Regular,
                    parent_persistent_id: None,
                    sort_order: 0,
                    track_entries: &[],
                    smart_rule_json: None,
                },
            )
            .await
            .unwrap();
        }
        let keep = vec![2u64];
        let d = delete_missing(&db.engine, 1, &keep).await.unwrap();
        assert_eq!(d, 2);
    }
}
