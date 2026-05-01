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
