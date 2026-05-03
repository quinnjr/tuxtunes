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
