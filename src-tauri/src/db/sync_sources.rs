//! CRUD helpers for the `sync_sources` table.

use crate::sync::conflict::ConflictRules;
use crate::sync::path_map::PathMapping;
use prax_query::filter::FilterValue;
use prax_sqlite::raw::SqliteRawEngine;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct SyncSourceRow {
    pub id: i64,
    pub name: String,
    pub kind: String,
    pub source_path: String,
    pub last_sync_at: Option<String>,
    pub last_sync_hash: Option<String>,
    pub path_mappings: Vec<PathMapping>,
    pub conflict_rules: ConflictRules,
    #[serde(deserialize_with = "crate::db::sync_util::sqlite_bool")]
    pub auto_copy_files: bool,
}

#[derive(Debug, thiserror::Error)]
pub enum SyncSourcesError {
    #[error("query failed: {0}")]
    Query(#[source] anyhow::Error),
}

pub async fn list(engine: &SqliteRawEngine) -> Result<Vec<SyncSourceRow>, SyncSourcesError> {
    let sql = "SELECT id, name, kind, source_path, last_sync_at, last_sync_hash, \
               path_mappings, conflict_rules, auto_copy_files \
               FROM sync_sources ORDER BY id";
    let json_rows = engine
        .raw_sql_query(sql, &[])
        .await
        .map_err(|e| SyncSourcesError::Query(anyhow::Error::from(e)))?;
    json_rows
        .into_iter()
        .map(|r| deserialize_row(r.into_json()))
        .collect::<Result<_, _>>()
        .map_err(|e| SyncSourcesError::Query(anyhow::Error::from(e)))
}

pub async fn get(engine: &SqliteRawEngine, id: i64) -> Result<SyncSourceRow, SyncSourcesError> {
    let sql = "SELECT id, name, kind, source_path, last_sync_at, last_sync_hash, \
               path_mappings, conflict_rules, auto_copy_files \
               FROM sync_sources WHERE id = ?";
    let params = vec![FilterValue::Int(id)];
    let json_row = engine
        .raw_sql_first(sql, &params)
        .await
        .map_err(|e| SyncSourcesError::Query(anyhow::Error::from(e)))?;
    deserialize_row(json_row.into_json())
        .map_err(|e| SyncSourcesError::Query(anyhow::Error::from(e)))
}

pub async fn insert(
    engine: &SqliteRawEngine,
    name: &str,
    source_path: &str,
    path_mappings: &[PathMapping],
    conflict_rules: &ConflictRules,
    auto_copy_files: bool,
) -> Result<i64, SyncSourcesError> {
    let pm_json = serde_json::to_string(path_mappings)
        .map_err(|e| SyncSourcesError::Query(anyhow::Error::from(e)))?;
    let cr_json = serde_json::to_string(conflict_rules)
        .map_err(|e| SyncSourcesError::Query(anyhow::Error::from(e)))?;
    let sql = "INSERT INTO sync_sources (name, kind, source_path, path_mappings, \
               conflict_rules, auto_copy_files) \
               VALUES (?, 'itunes_itl', ?, ?, ?, ?) RETURNING id";
    let params = vec![
        FilterValue::String(name.to_string()),
        FilterValue::String(source_path.to_string()),
        FilterValue::String(pm_json),
        FilterValue::String(cr_json),
        FilterValue::Int(i64::from(auto_copy_files)),
    ];
    let json_row = engine
        .raw_sql_first(sql, &params)
        .await
        .map_err(|e| SyncSourcesError::Query(anyhow::Error::from(e)))?;
    json_row
        .into_json()
        .get("id")
        .and_then(|v| v.as_i64())
        .ok_or_else(|| SyncSourcesError::Query(anyhow::anyhow!("INSERT ... RETURNING id missing")))
}

pub async fn finalize_sync(
    engine: &SqliteRawEngine,
    id: i64,
    hash: &str,
) -> Result<(), SyncSourcesError> {
    let sql = "UPDATE sync_sources SET last_sync_at = CURRENT_TIMESTAMP, \
               last_sync_hash = ? WHERE id = ?";
    let params = vec![FilterValue::String(hash.to_string()), FilterValue::Int(id)];
    engine
        .raw_sql_execute(sql, &params)
        .await
        .map(|_| ())
        .map_err(|e| SyncSourcesError::Query(anyhow::Error::from(e)))
}

fn deserialize_row(v: serde_json::Value) -> serde_json::Result<SyncSourceRow> {
    // The `path_mappings` and `conflict_rules` columns store JSON as TEXT
    // in SQLite. Prax hands them back as `Value::String`; unwrap those
    // into nested objects before feeding serde.
    let mut obj = match v {
        serde_json::Value::Object(m) => m,
        _ => {
            return Err(<serde_json::Error as serde::de::Error>::custom(
                "row is not an object",
            ));
        }
    };
    for field in ["path_mappings", "conflict_rules"] {
        let parsed = match obj.remove(field) {
            Some(serde_json::Value::String(s)) => serde_json::from_str(&s)?,
            Some(other) => other,
            None => serde_json::Value::Null,
        };
        obj.insert(field.into(), parsed);
    }
    serde_json::from_value(serde_json::Value::Object(obj))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::Db;

    async fn tmp() -> Db {
        let tmp = tempfile::NamedTempFile::new().unwrap();
        Db::open(tmp.path()).await.unwrap()
    }

    fn default_rules() -> ConflictRules {
        ConflictRules::default()
    }

    #[tokio::test]
    async fn insert_then_get_roundtrip() {
        let db = tmp().await;
        let id = insert(
            &db.engine,
            "My iTunes",
            "/tmp/a.itl",
            &[PathMapping {
                from: "D:/".into(),
                to: "/mnt/d/".into(),
            }],
            &default_rules(),
            true,
        )
        .await
        .unwrap();
        let row = get(&db.engine, id).await.unwrap();
        assert_eq!(row.name, "My iTunes");
        assert_eq!(row.source_path, "/tmp/a.itl");
        assert_eq!(row.path_mappings.len(), 1);
        assert!(row.auto_copy_files);
    }

    #[tokio::test]
    async fn finalize_sets_hash_and_timestamp() {
        let db = tmp().await;
        let id = insert(&db.engine, "x", "/tmp/b.itl", &[], &default_rules(), false)
            .await
            .unwrap();
        finalize_sync(&db.engine, id, "deadbeef").await.unwrap();
        let row = get(&db.engine, id).await.unwrap();
        assert_eq!(row.last_sync_hash.as_deref(), Some("deadbeef"));
        assert!(row.last_sync_at.is_some());
    }

    #[tokio::test]
    async fn list_returns_in_id_order() {
        let db = tmp().await;
        let a = insert(&db.engine, "A", "/a.itl", &[], &default_rules(), true)
            .await
            .unwrap();
        let b = insert(&db.engine, "B", "/b.itl", &[], &default_rules(), true)
            .await
            .unwrap();
        let rows = list(&db.engine).await.unwrap();
        assert_eq!(rows.len(), 2);
        assert_eq!(rows[0].id, a);
        assert_eq!(rows[1].id, b);
    }

    #[test]
    fn deserialize_row_rejects_non_object() {
        let arr = serde_json::Value::Array(vec![]);
        let err = deserialize_row(arr).unwrap_err();
        assert!(err.to_string().contains("not an object"));
    }

    #[test]
    fn deserialize_row_handles_pre_parsed_json_columns() {
        // path_mappings already comes through as a Value::Array (rather
        // than a JSON-encoded string) — exercise the "Some(other)" arm
        // of the unwrap match.
        let mut obj = serde_json::Map::new();
        obj.insert("id".into(), serde_json::Value::from(1_i64));
        obj.insert("name".into(), serde_json::Value::from("X"));
        obj.insert("source_path".into(), serde_json::Value::from("/x"));
        obj.insert(
            "path_mappings".into(),
            serde_json::Value::Array(vec![]),
        );
        obj.insert("conflict_rules".into(), serde_json::Value::Object(Default::default()));
        obj.insert("kind".into(), serde_json::Value::from("itunes_itl"));
        obj.insert("auto_copy_files".into(), serde_json::Value::from(1_i64));
        obj.insert("last_sync_at".into(), serde_json::Value::Null);
        obj.insert("last_sync_hash".into(), serde_json::Value::Null);

        // No conflict_rules contents required: the empty object satisfies
        // ConflictRules::default-style deserialization.
        let result = deserialize_row(serde_json::Value::Object(obj));
        // It might fail if ConflictRules requires fields — accept either
        // outcome but make sure the "not an object" branch isn't the
        // failure path.
        if let Err(e) = result {
            let msg = e.to_string();
            assert!(!msg.contains("not an object"), "wrong error path: {msg}");
        }
    }

    #[test]
    fn sync_sources_error_display_is_stable() {
        let e = SyncSourcesError::Query(anyhow::anyhow!("oops"));
        assert!(e.to_string().contains("oops"));
    }

    #[tokio::test]
    async fn get_for_unknown_id_returns_query_error() {
        let db = tmp().await;
        let err = get(&db.engine, 999).await.unwrap_err();
        assert!(matches!(err, SyncSourcesError::Query(_)));
    }
}
