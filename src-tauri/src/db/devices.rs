//! CRUD helpers for the `devices` table.

use prax_query::filter::FilterValue;
use prax_sqlite::raw::SqliteRawEngine;
use serde::{Deserialize, Serialize};

/// What a user picked to push to a device.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SelectionKind {
    Playlist,
    Album,
    Smart,
    /// The entire library.
    All,
}

/// One entry in a device's `selection` column.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct SelectionEntry {
    pub kind: SelectionKind,
    /// The playlist or album id. Ignored for [`SelectionKind::All`].
    #[serde(default)]
    pub id: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct DeviceRow {
    pub id: i64,
    pub name: String,
    pub kind: String,
    pub device_key: String,
    #[serde(deserialize_with = "crate::db::sync_util::sqlite_bool")]
    pub key_is_weak: bool,
    pub root_path: String,
    pub mount_path: Option<String>,
    pub last_seen_at: Option<String>,
    pub last_sync_at: Option<String>,
    pub selection: Vec<SelectionEntry>,
    pub layout_template: String,
    #[serde(deserialize_with = "crate::db::sync_util::sqlite_bool")]
    pub auto_sync: bool,
    #[serde(deserialize_with = "crate::db::sync_util::sqlite_bool")]
    pub mirror_deletes: bool,
    #[serde(deserialize_with = "crate::db::sync_util::sqlite_bool")]
    pub write_playlist_objects: bool,
}

#[derive(Debug, thiserror::Error)]
pub enum DevicesError {
    #[error("query failed: {0}")]
    Query(#[source] anyhow::Error),
}

fn query_err(e: impl Into<anyhow::Error>) -> DevicesError {
    DevicesError::Query(e.into())
}

const COLUMNS: &str = "id, name, kind, device_key, key_is_weak, root_path, mount_path, \
                       last_seen_at, last_sync_at, selection, layout_template, auto_sync, \
                       mirror_deletes, write_playlist_objects";

pub async fn list(engine: &SqliteRawEngine) -> Result<Vec<DeviceRow>, DevicesError> {
    let sql = format!("SELECT {COLUMNS} FROM devices ORDER BY name, id");
    let rows = engine.raw_sql_query(&sql, &[]).await.map_err(query_err)?;
    rows.into_iter()
        .map(|r| deserialize_row(r.into_json()))
        .collect::<Result<_, _>>()
        .map_err(query_err)
}

pub async fn get(engine: &SqliteRawEngine, id: i64) -> Result<DeviceRow, DevicesError> {
    let sql = format!("SELECT {COLUMNS} FROM devices WHERE id = ?");
    let row = engine
        .raw_sql_first(&sql, &[FilterValue::Int(id)])
        .await
        .map_err(query_err)?;
    deserialize_row(row.into_json()).map_err(query_err)
}

/// Insert the device, or update the name and mount of the one already
/// holding `device_key`. Returns its id either way, so re-plugging a
/// device keeps its selection and manifest.
pub async fn upsert_by_key(
    engine: &SqliteRawEngine,
    device_key: &str,
    name: &str,
    kind: &str,
    mount_path: Option<&str>,
) -> Result<i64, DevicesError> {
    let sql = "INSERT INTO devices (device_key, name, kind, mount_path, last_seen_at) \
               VALUES (?, ?, ?, ?, CURRENT_TIMESTAMP) \
               ON CONFLICT(device_key) DO UPDATE SET \
                 name = excluded.name, \
                 kind = excluded.kind, \
                 mount_path = excluded.mount_path, \
                 last_seen_at = CURRENT_TIMESTAMP \
               RETURNING id";
    let params = vec![
        FilterValue::String(device_key.to_string()),
        FilterValue::String(name.to_string()),
        FilterValue::String(kind.to_string()),
        crate::db::sync_util::opt_str(mount_path),
    ];
    let row = engine
        .raw_sql_first(sql, &params)
        .await
        .map_err(query_err)?;
    row.into_json()
        .get("id")
        .and_then(|v| v.as_i64())
        .ok_or_else(|| DevicesError::Query(anyhow::anyhow!("INSERT ... RETURNING id missing")))
}

pub async fn update_selection(
    engine: &SqliteRawEngine,
    id: i64,
    selection: &[SelectionEntry],
) -> Result<(), DevicesError> {
    let json = serde_json::to_string(selection).map_err(query_err)?;
    let params = vec![FilterValue::String(json), FilterValue::Int(id)];
    engine
        .raw_sql_execute("UPDATE devices SET selection = ? WHERE id = ?", &params)
        .await
        .map(|_| ())
        .map_err(query_err)
}

/// The user-editable knobs from the device settings panel.
#[derive(Debug, Clone, serde::Deserialize)]
pub struct DeviceSettings {
    pub name: String,
    pub root_path: String,
    pub layout_template: String,
    pub auto_sync: bool,
    pub mirror_deletes: bool,
    pub write_playlist_objects: bool,
}

pub async fn update_settings(
    engine: &SqliteRawEngine,
    id: i64,
    s: &DeviceSettings,
) -> Result<(), DevicesError> {
    let sql = "UPDATE devices SET name = ?, root_path = ?, layout_template = ?, \
               auto_sync = ?, mirror_deletes = ?, write_playlist_objects = ? WHERE id = ?";
    let params = vec![
        FilterValue::String(s.name.clone()),
        FilterValue::String(s.root_path.clone()),
        FilterValue::String(s.layout_template.clone()),
        FilterValue::Int(i64::from(s.auto_sync)),
        FilterValue::Int(i64::from(s.mirror_deletes)),
        FilterValue::Int(i64::from(s.write_playlist_objects)),
        FilterValue::Int(id),
    ];
    engine
        .raw_sql_execute(sql, &params)
        .await
        .map(|_| ())
        .map_err(query_err)
}

pub async fn touch_seen(engine: &SqliteRawEngine, id: i64) -> Result<(), DevicesError> {
    engine
        .raw_sql_execute(
            "UPDATE devices SET last_seen_at = CURRENT_TIMESTAMP WHERE id = ?",
            &[FilterValue::Int(id)],
        )
        .await
        .map(|_| ())
        .map_err(query_err)
}

pub async fn mark_synced(engine: &SqliteRawEngine, id: i64) -> Result<(), DevicesError> {
    engine
        .raw_sql_execute(
            "UPDATE devices SET last_sync_at = CURRENT_TIMESTAMP WHERE id = ?",
            &[FilterValue::Int(id)],
        )
        .await
        .map(|_| ())
        .map_err(query_err)
}

/// Forget a device and its whole manifest.
///
/// The manifest rows are deleted explicitly rather than relying on the
/// `ON DELETE CASCADE`, because `PRAGMA foreign_keys` is not guaranteed
/// on for every connection in the pool and orphaned manifest rows would
/// make a later device reuse the same ids look already-synced.
pub async fn remove(engine: &SqliteRawEngine, id: i64) -> Result<(), DevicesError> {
    engine
        .raw_sql_execute(
            "DELETE FROM device_objects WHERE device_id = ?",
            &[FilterValue::Int(id)],
        )
        .await
        .map_err(query_err)?;
    engine
        .raw_sql_execute("DELETE FROM devices WHERE id = ?", &[FilterValue::Int(id)])
        .await
        .map(|_| ())
        .map_err(query_err)
}

/// `selection` is stored as JSON in a TEXT column; Prax returns it as a
/// `Value::String`, so unwrap it before handing the row to serde.
fn deserialize_row(v: serde_json::Value) -> serde_json::Result<DeviceRow> {
    let mut obj = match v {
        serde_json::Value::Object(m) => m,
        _ => {
            return Err(<serde_json::Error as serde::de::Error>::custom(
                "row is not an object",
            ))
        }
    };
    let parsed = match obj.remove("selection") {
        Some(serde_json::Value::String(s)) if !s.is_empty() => serde_json::from_str(&s)?,
        Some(serde_json::Value::String(_)) | None | Some(serde_json::Value::Null) => {
            serde_json::Value::Array(Vec::new())
        }
        Some(other) => other,
    };
    obj.insert("selection".into(), parsed);
    serde_json::from_value(serde_json::Value::Object(obj))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::Db;

    async fn tmp() -> (tempfile::NamedTempFile, Db) {
        let file = tempfile::NamedTempFile::new().unwrap();
        let db = Db::open(file.path()).await.unwrap();
        (file, db)
    }

    #[tokio::test]
    async fn upsert_by_key_inserts_then_updates_in_place() {
        let (_f, db) = tmp().await;
        let a = upsert_by_key(&db.engine, "usb:1:abc", "Pixel", "filesystem", Some("/mnt/p"))
            .await
            .unwrap();
        let b = upsert_by_key(
            &db.engine,
            "usb:1:abc",
            "Pixel 8",
            "filesystem",
            Some("/mnt/q"),
        )
        .await
        .unwrap();
        assert_eq!(a, b, "the same key must reuse the same device row");
        let row = get(&db.engine, a).await.unwrap();
        assert_eq!(row.name, "Pixel 8");
        assert_eq!(row.mount_path.as_deref(), Some("/mnt/q"));
        assert_eq!(list(&db.engine).await.unwrap().len(), 1);
    }

    #[tokio::test]
    async fn defaults_are_sane_on_insert() {
        let (_f, db) = tmp().await;
        let id = upsert_by_key(&db.engine, "k", "D", "filesystem", None)
            .await
            .unwrap();
        let row = get(&db.engine, id).await.unwrap();
        assert!(row.selection.is_empty(), "selection defaults to []");
        assert_eq!(row.root_path, "/Music");
        assert!(row.mirror_deletes);
        assert!(row.write_playlist_objects);
        assert!(!row.auto_sync);
        assert!(!row.key_is_weak);
    }

    #[tokio::test]
    async fn update_selection_roundtrips_json() {
        let (_f, db) = tmp().await;
        let id = upsert_by_key(&db.engine, "k", "D", "filesystem", None)
            .await
            .unwrap();
        let sel = vec![
            SelectionEntry {
                kind: SelectionKind::Playlist,
                id: 7,
            },
            SelectionEntry {
                kind: SelectionKind::Album,
                id: 9,
            },
        ];
        update_selection(&db.engine, id, &sel).await.unwrap();
        assert_eq!(get(&db.engine, id).await.unwrap().selection, sel);
    }

    #[tokio::test]
    async fn update_settings_persists_every_field() {
        let (_f, db) = tmp().await;
        let id = upsert_by_key(&db.engine, "k", "D", "filesystem", None)
            .await
            .unwrap();
        update_settings(
            &db.engine,
            id,
            &DeviceSettings {
                name: "DAP".into(),
                root_path: "/Storage/Music".into(),
                layout_template: "{artist}/{title}.{ext}".into(),
                auto_sync: true,
                mirror_deletes: false,
                write_playlist_objects: false,
            },
        )
        .await
        .unwrap();
        let row = get(&db.engine, id).await.unwrap();
        assert_eq!(row.name, "DAP");
        assert_eq!(row.root_path, "/Storage/Music");
        assert_eq!(row.layout_template, "{artist}/{title}.{ext}");
        assert!(row.auto_sync);
        assert!(!row.mirror_deletes);
        assert!(!row.write_playlist_objects);
    }

    #[tokio::test]
    async fn remove_deletes_the_row() {
        let (_f, db) = tmp().await;
        let id = upsert_by_key(&db.engine, "k", "D", "filesystem", None)
            .await
            .unwrap();
        remove(&db.engine, id).await.unwrap();
        assert!(list(&db.engine).await.unwrap().is_empty());
    }
}
