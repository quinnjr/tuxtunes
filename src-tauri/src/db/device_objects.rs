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

/// Clear the `track_id` of every manifest row pointing at a deleted
/// track, on every device.
///
/// The table's `ON DELETE SET NULL` covers this on any connection with
/// `PRAGMA foreign_keys` on, which this pool does have. Doing it
/// explicitly makes the invariant independent of that pragma, and puts
/// it beside the equivalent `playlists::prune_track` call so both are
/// obvious at the deletion site.
///
/// The invariant matters because `tracks.id` is `INTEGER PRIMARY KEY`
/// without `AUTOINCREMENT`: SQLite reuses rowids, so a dangling id
/// would later be handed to an unrelated track and the sync would treat
/// it as already present at the deleted track's path.
///
/// The row itself is kept: the file is still on the device, and the
/// manifest is the only record that it is ours to prune.
pub async fn detach_track(
    engine: &SqliteRawEngine,
    track_id: i64,
) -> Result<(), DeviceObjectsError> {
    engine
        .raw_sql_execute(
            "UPDATE device_objects SET track_id = NULL WHERE track_id = ?",
            &[FilterValue::Int(track_id)],
        )
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
        let device_id = devices::upsert_by_key(&db.engine, "k", "D", "filesystem", None, false)
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

    /// Insert a minimal track row and return its id.
    async fn add_track(db: &Db, title: &str) -> i64 {
        let sql = "INSERT INTO tracks (title, duration_ms, size_bytes, file_path, playlist_ids) \
                   VALUES (?, 1000, 10, ?, '[]') RETURNING id";
        db.engine
            .raw_sql_first(
                sql,
                &[
                    FilterValue::String(title.to_string()),
                    FilterValue::String(format!("/lib/{title}.flac")),
                ],
            )
            .await
            .unwrap()
            .into_json()
            .get("id")
            .and_then(|v| v.as_i64())
            .unwrap()
    }

    #[tokio::test]
    async fn detach_track_clears_the_id_but_keeps_the_row() {
        let (_f, db, d) = tmp().await;
        let track_id = add_track(&db, "One").await;
        let mut row = new_row(d, "/Music/a.flac", "h");
        row.track_id = Some(track_id);
        upsert(&db.engine, &row).await.unwrap();

        detach_track(&db.engine, track_id).await.unwrap();

        let rows = list_for_device(&db.engine, d).await.unwrap();
        assert_eq!(rows.len(), 1, "the file is still on the device");
        assert_eq!(
            rows[0].track_id, None,
            "a reused rowid must not rebind this row to an unrelated track"
        );
    }

    #[tokio::test]
    async fn detach_track_leaves_other_tracks_alone() {
        let (_f, db, d) = tmp().await;
        let a_id = add_track(&db, "One").await;
        let b_id = add_track(&db, "Two").await;
        let mut a = new_row(d, "/Music/a.flac", "h");
        a.track_id = Some(a_id);
        let mut b = new_row(d, "/Music/b.flac", "h");
        b.track_id = Some(b_id);
        upsert(&db.engine, &a).await.unwrap();
        upsert(&db.engine, &b).await.unwrap();

        detach_track(&db.engine, a_id).await.unwrap();

        let rows = list_for_device(&db.engine, d).await.unwrap();
        assert_eq!(rows.iter().filter(|r| r.track_id == Some(b_id)).count(), 1);
        assert_eq!(rows.iter().filter(|r| r.track_id.is_none()).count(), 1);
    }

    #[tokio::test]
    async fn a_manifest_row_cannot_name_a_track_that_does_not_exist() {
        // Foreign keys are enforced on this pool, which is what makes
        // detach_track a guarantee rather than a hope.
        let (_f, db, d) = tmp().await;
        let mut row = new_row(d, "/Music/a.flac", "h");
        row.track_id = Some(424_242);
        assert!(upsert(&db.engine, &row).await.is_err());
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
