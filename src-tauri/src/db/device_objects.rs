//! CRUD helpers for the `device_objects` manifest table.
//!
//! Every row records one object TuxTunes wrote to a device. Nothing
//! outside this table is ever a deletion candidate.

use prax_query::filter::FilterValue;
use prax_sqlite::raw::SqliteRawEngine;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct DeviceObjectRow {
    pub id: i64,
    pub device_id: i64,
    /// `track`, `playlist`, or `artwork`.
    pub kind: String,
    pub track_id: Option<i64>,
    pub persistent_id: Option<String>,
    pub device_path: String,
    pub object_id: Option<String>,
    pub source_hash: Option<String>,
    pub encoded_codec: String,
    pub size_bytes: i64,
}

/// A row about to be written. Carries no `id`: the unique
/// `(device_id, device_path)` index decides insert versus update.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NewDeviceObject {
    pub device_id: i64,
    pub kind: String,
    pub track_id: Option<i64>,
    pub persistent_id: Option<String>,
    pub device_path: String,
    pub object_id: Option<String>,
    pub source_hash: Option<String>,
    pub encoded_codec: String,
    pub size_bytes: i64,
}

#[derive(Debug, thiserror::Error)]
pub enum DeviceObjectsError {
    #[error("query failed: {0}")]
    Query(#[source] anyhow::Error),
}

fn query_err(e: impl Into<anyhow::Error>) -> DeviceObjectsError {
    DeviceObjectsError::Query(e.into())
}

const COLUMNS: &str = "id, device_id, kind, track_id, persistent_id, device_path, \
                       object_id, source_hash, encoded_codec, size_bytes";

pub async fn list_for_device(
    engine: &SqliteRawEngine,
    device_id: i64,
) -> Result<Vec<DeviceObjectRow>, DeviceObjectsError> {
    let sql = format!("SELECT {COLUMNS} FROM device_objects WHERE device_id = ? ORDER BY id");
    let rows = engine
        .raw_sql_query(&sql, &[FilterValue::Int(device_id)])
        .await
        .map_err(query_err)?;
    rows.into_iter()
        .map(|r| serde_json::from_value(r.into_json()))
        .collect::<Result<_, _>>()
        .map_err(query_err)
}

/// Insert, or replace the row already at this `(device_id, device_path)`.
pub async fn upsert(
    engine: &SqliteRawEngine,
    row: &NewDeviceObject,
) -> Result<(), DeviceObjectsError> {
    let sql = "INSERT INTO device_objects \
               (device_id, kind, track_id, persistent_id, device_path, object_id, \
                source_hash, encoded_codec, size_bytes, pushed_at) \
               VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, CURRENT_TIMESTAMP) \
               ON CONFLICT(device_id, device_path) DO UPDATE SET \
                 kind = excluded.kind, \
                 track_id = excluded.track_id, \
                 persistent_id = excluded.persistent_id, \
                 object_id = excluded.object_id, \
                 source_hash = excluded.source_hash, \
                 encoded_codec = excluded.encoded_codec, \
                 size_bytes = excluded.size_bytes, \
                 pushed_at = CURRENT_TIMESTAMP";
    let params = vec![
        FilterValue::Int(row.device_id),
        FilterValue::String(row.kind.clone()),
        crate::db::sync_util::opt_int(row.track_id),
        crate::db::sync_util::opt_str(row.persistent_id.as_deref()),
        FilterValue::String(row.device_path.clone()),
        crate::db::sync_util::opt_str(row.object_id.as_deref()),
        crate::db::sync_util::opt_str(row.source_hash.as_deref()),
        FilterValue::String(row.encoded_codec.clone()),
        FilterValue::Int(row.size_bytes),
    ];
    engine
        .raw_sql_execute(sql, &params)
        .await
        .map(|_| ())
        .map_err(query_err)
}

pub async fn remove_by_id(engine: &SqliteRawEngine, id: i64) -> Result<(), DeviceObjectsError> {
    engine
        .raw_sql_execute(
            "DELETE FROM device_objects WHERE id = ?",
            &[FilterValue::Int(id)],
        )
        .await
        .map(|_| ())
        .map_err(query_err)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::{devices, Db};

    async fn tmp() -> (tempfile::NamedTempFile, Db, i64) {
        let file = tempfile::NamedTempFile::new().unwrap();
        let db = Db::open(file.path()).await.unwrap();
        let device_id = devices::upsert_by_key(&db.engine, "k", "D", "filesystem", None)
            .await
            .unwrap();
        (file, db, device_id)
    }

    fn new_row(device_id: i64, path: &str, hash: &str) -> NewDeviceObject {
        NewDeviceObject {
            device_id,
            kind: "track".into(),
            track_id: None,
            persistent_id: None,
            device_path: path.into(),
            object_id: None,
            source_hash: Some(hash.into()),
            encoded_codec: "copy:flac".into(),
            size_bytes: 100,
        }
    }

    #[tokio::test]
    async fn upsert_replaces_on_the_same_device_path() {
        let (_f, db, d) = tmp().await;
        upsert(&db.engine, &new_row(d, "/Music/a.flac", "h1"))
            .await
            .unwrap();
        upsert(&db.engine, &new_row(d, "/Music/a.flac", "h2"))
            .await
            .unwrap();
        let rows = list_for_device(&db.engine, d).await.unwrap();
        assert_eq!(rows.len(), 1, "the unique index must collapse the pair");
        assert_eq!(rows[0].source_hash.as_deref(), Some("h2"));
    }

    #[tokio::test]
    async fn distinct_paths_are_distinct_rows() {
        let (_f, db, d) = tmp().await;
        upsert(&db.engine, &new_row(d, "/Music/a.flac", "h"))
            .await
            .unwrap();
        upsert(&db.engine, &new_row(d, "/Music/b.flac", "h"))
            .await
            .unwrap();
        assert_eq!(list_for_device(&db.engine, d).await.unwrap().len(), 2);
    }

    #[tokio::test]
    async fn remove_by_id_deletes_only_that_row() {
        let (_f, db, d) = tmp().await;
        upsert(&db.engine, &new_row(d, "/Music/a.flac", "h"))
            .await
            .unwrap();
        upsert(&db.engine, &new_row(d, "/Music/b.flac", "h"))
            .await
            .unwrap();
        let rows = list_for_device(&db.engine, d).await.unwrap();
        remove_by_id(&db.engine, rows[0].id).await.unwrap();
        let left = list_for_device(&db.engine, d).await.unwrap();
        assert_eq!(left.len(), 1);
        assert_eq!(left[0].device_path, "/Music/b.flac");
    }

    #[tokio::test]
    async fn forgetting_a_device_clears_its_manifest() {
        let (_f, db, d) = tmp().await;
        upsert(&db.engine, &new_row(d, "/Music/a.flac", "h"))
            .await
            .unwrap();
        devices::remove(&db.engine, d).await.unwrap();
        assert!(list_for_device(&db.engine, d).await.unwrap().is_empty());
    }
}
