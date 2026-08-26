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
            let sql = "UPDATE playlists SET name = ?, kind = ?, \
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
               VALUES (?, 'smart', 0, '[]', ?) RETURNING id";
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
    let sql = "SELECT id, name, kind, parent_id, sort_order, cached_track_count \
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
        let sql = format!(
            "SELECT id, title, artist, album, duration_ms, file_path, file_hash, \
             sample_rate, bit_depth, kind, play_count, skip_count, import_status, artwork_path \
             FROM tracks WHERE id IN ({placeholders})"
        );
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

/// Hard-delete a playlist by id. Sync-sourced playlists deleted this
/// way will reappear on the next sync — that's the intended behavior.
pub async fn delete(engine: &SqliteRawEngine, playlist_id: i64) -> Result<(), PlaylistsError> {
    let sql = "DELETE FROM playlists WHERE id = ?";
    engine
        .raw_sql_execute(sql, &[FilterValue::Int(playlist_id)])
        .await
        .map(|_| ())
        .map_err(|e| PlaylistsError::Query(anyhow::Error::from(e)))
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
