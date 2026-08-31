//! Tauri commands for outbound device sync.

use crate::db::devices::{self, DeviceRow, DeviceSettings, SelectionEntry};
use crate::device::engine;
use crate::device::transport::fs::FsTransport;
use crate::device::transport::{DeviceTransport, StorageInfo};
use crate::runtime::AppState;
use std::path::PathBuf;

#[tauri::command]
pub async fn list_devices(state: tauri::State<'_, AppState>) -> Result<Vec<DeviceRow>, String> {
    devices::list(&state.db.engine)
        .await
        .map_err(|e| e.to_string())
}

#[tauri::command]
pub async fn get_device(
    state: tauri::State<'_, AppState>,
    device_id: i64,
) -> Result<DeviceRow, String> {
    devices::get(&state.db.engine, device_id)
        .await
        .map_err(|e| e.to_string())
}

#[derive(Debug, serde::Deserialize)]
pub struct AddFilesystemDeviceArgs {
    pub name: String,
    /// Host path the device is mounted at.
    pub mount_path: String,
    /// Device-rooted directory to sync into, e.g. `/Music`.
    pub root_path: Option<String>,
}

/// Register (or re-register) a device reached through a mounted path:
/// a gvfs/mtpfs mount, an SD card, or a DAP in mass-storage mode.
#[tauri::command]
pub async fn add_filesystem_device(
    state: tauri::State<'_, AppState>,
    args: AddFilesystemDeviceArgs,
) -> Result<i64, String> {
    if !PathBuf::from(&args.mount_path).is_dir() {
        return Err(format!("{} is not a directory", args.mount_path));
    }
    let key = format!("fs:{}", args.mount_path);
    let id = devices::upsert_by_key(
        &state.db.engine,
        &key,
        &args.name,
        "filesystem",
        Some(&args.mount_path),
    )
    .await
    .map_err(|e| e.to_string())?;

    if let Some(root) = args.root_path {
        let current = devices::get(&state.db.engine, id)
            .await
            .map_err(|e| e.to_string())?;
        devices::update_settings(
            &state.db.engine,
            id,
            &DeviceSettings {
                name: current.name,
                root_path: root,
                layout_template: current.layout_template,
                auto_sync: current.auto_sync,
                mirror_deletes: current.mirror_deletes,
                write_playlist_objects: current.write_playlist_objects,
            },
        )
        .await
        .map_err(|e| e.to_string())?;
    }
    Ok(id)
}

/// Re-stat every known filesystem device, refreshing `last_seen_at`.
///
/// Phase 1 has no hotplug enumeration; that arrives with the MTP
/// backend. Returns the refreshed list so the UI has one round trip.
#[tauri::command]
pub async fn refresh_devices(
    state: tauri::State<'_, AppState>,
) -> Result<Vec<DeviceRow>, String> {
    let rows = devices::list(&state.db.engine)
        .await
        .map_err(|e| e.to_string())?;
    for row in &rows {
        let present = row
            .mount_path
            .as_deref()
            .is_some_and(|p| PathBuf::from(p).is_dir());
        if present {
            devices::touch_seen(&state.db.engine, row.id)
                .await
                .map_err(|e| e.to_string())?;
        }
    }
    devices::list(&state.db.engine)
        .await
        .map_err(|e| e.to_string())
}

#[tauri::command]
pub async fn update_device_selection(
    state: tauri::State<'_, AppState>,
    device_id: i64,
    selection: Vec<SelectionEntry>,
) -> Result<(), String> {
    devices::update_selection(&state.db.engine, device_id, &selection)
        .await
        .map_err(|e| e.to_string())
}

#[tauri::command]
pub async fn update_device_settings(
    state: tauri::State<'_, AppState>,
    device_id: i64,
    settings: DeviceSettings,
) -> Result<(), String> {
    devices::update_settings(&state.db.engine, device_id, &settings)
        .await
        .map_err(|e| e.to_string())
}

#[tauri::command]
pub async fn forget_device(
    state: tauri::State<'_, AppState>,
    device_id: i64,
) -> Result<(), String> {
    devices::remove(&state.db.engine, device_id)
        .await
        .map_err(|e| e.to_string())
}

/// What a sync would do, without touching the device.
#[derive(Debug, Clone, Copy, Default, serde::Serialize, PartialEq, Eq)]
pub struct SyncPlanSummary {
    pub adds: u64,
    pub replaces: u64,
    pub unchanged: u64,
    pub deletes: u64,
    pub skips: u64,
    pub bytes_out: u64,
    pub free_bytes: Option<u64>,
    pub total_bytes: Option<u64>,
}

#[tauri::command]
pub async fn preview_device_sync(
    state: tauri::State<'_, AppState>,
    device_id: i64,
) -> Result<SyncPlanSummary, String> {
    let device = devices::get(&state.db.engine, device_id)
        .await
        .map_err(|e| e.to_string())?;
    let transport = filesystem_transport(&device)?;
    let (plan, skips) = engine::build_plan(&state.db.engine, &device, transport.as_ref())
        .await
        .map_err(|e| e.to_string())?;
    let storage: Option<StorageInfo> = if transport.capabilities().free_space {
        transport.free_space().ok()
    } else {
        None
    };

    // Pruning is what `mirror_deletes` and `key_is_weak` gate, so the
    // preview must apply the same gates or it would promise deletions
    // the run will not perform.
    let will_delete = device.mirror_deletes && !device.key_is_weak;

    Ok(SyncPlanSummary {
        adds: plan.adds.len() as u64,
        replaces: plan.replaces.len() as u64,
        unchanged: plan.unchanged as u64,
        deletes: if will_delete {
            plan.orphans.len() as u64
        } else {
            0
        },
        skips: skips.len() as u64,
        bytes_out: plan.bytes_out,
        free_bytes: storage.map(|s| s.free_bytes),
        total_bytes: storage.map(|s| s.total_bytes),
    })
}

#[tauri::command]
pub async fn run_device_sync(
    state: tauri::State<'_, AppState>,
    device_id: i64,
) -> Result<(), String> {
    state.devices.run_now(device_id)
}

#[tauri::command]
pub async fn cancel_device_sync(
    state: tauri::State<'_, AppState>,
    device_id: i64,
) -> Result<(), String> {
    state.devices.cancel(device_id)
}

/// Build a read-only transport for the preview path.
///
/// Only filesystem devices are reachable in Phase 1; the MTP and WPD
/// backends replace this with the worker's shared resolver.
fn filesystem_transport(device: &DeviceRow) -> Result<Box<dyn DeviceTransport>, String> {
    if device.kind != "filesystem" {
        return Err(format!("no transport for device kind '{}' yet", device.kind));
    }
    let mount = device
        .mount_path
        .as_deref()
        .filter(|m| !m.is_empty())
        .ok_or_else(|| "filesystem device has no mount path".to_string())?;
    Ok(Box::new(FsTransport::new(PathBuf::from(mount))))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::Db;
    use prax_query::filter::FilterValue;

    async fn setup() -> (tempfile::NamedTempFile, tempfile::TempDir, Db, i64) {
        let dbf = tempfile::NamedTempFile::new().unwrap();
        let db = Db::open(dbf.path()).await.unwrap();
        let mount = tempfile::tempdir().unwrap();
        let id = devices::upsert_by_key(
            &db.engine,
            "fs:test",
            "DAP",
            "filesystem",
            mount.path().to_str(),
        )
        .await
        .unwrap();
        (dbf, mount, db, id)
    }

    async fn add_track(db: &Db, title: &str, path: &std::path::Path) -> i64 {
        std::fs::write(path, b"1234567890").unwrap();
        let sql = "INSERT INTO tracks (title, artist, album_artist, album, track_number, \
                   duration_ms, size_bytes, file_path, file_hash, kind, playlist_ids) \
                   VALUES (?, 'A', 'A', 'Alb', 1, 1000, 10, ?, 'h', 'flac', '[]') RETURNING id";
        db.engine
            .raw_sql_first(
                sql,
                &[
                    FilterValue::String(title.to_string()),
                    FilterValue::String(path.to_str().unwrap().to_string()),
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
    async fn a_device_with_no_selection_previews_as_a_no_op() {
        let (_f, _m, db, id) = setup().await;
        let device = devices::get(&db.engine, id).await.unwrap();
        let t = filesystem_transport(&device).unwrap();
        let (plan, skips) = engine::build_plan(&db.engine, &device, t.as_ref())
            .await
            .unwrap();
        assert!(plan.adds.is_empty());
        assert!(skips.is_empty());
    }

    #[tokio::test]
    async fn preview_reports_counts_without_writing_to_the_device() {
        let (_f, mount, db, id) = setup().await;
        let lib = tempfile::tempdir().unwrap();
        let track_id = add_track(&db, "One", &lib.path().join("a.flac")).await;
        let pl = crate::db::playlists::create_regular(&db.engine, "P", None)
            .await
            .unwrap();
        crate::db::playlists::add_tracks(&db.engine, pl, &[track_id])
            .await
            .unwrap();
        devices::update_selection(&db.engine, id, &[SelectionEntry::Playlist { id: pl }])
            .await
            .unwrap();

        let device = devices::get(&db.engine, id).await.unwrap();
        let t = filesystem_transport(&device).unwrap();
        let (plan, _) = engine::build_plan(&db.engine, &device, t.as_ref())
            .await
            .unwrap();

        assert_eq!(plan.adds.len(), 1);
        assert_eq!(plan.bytes_out, 10);
        assert_eq!(
            std::fs::read_dir(mount.path()).unwrap().count(),
            0,
            "a dry run must not touch the device"
        );
    }

    #[test]
    fn a_device_kind_without_a_backend_is_rejected() {
        let device = DeviceRow {
            id: 1,
            name: "Phone".into(),
            kind: "mtp".into(),
            device_key: "k".into(),
            key_is_weak: false,
            root_path: "/Music".into(),
            mount_path: None,
            last_seen_at: None,
            last_sync_at: None,
            selection: Vec::new(),
            layout_template: "{title}.{ext}".into(),
            auto_sync: false,
            mirror_deletes: true,
            write_playlist_objects: true,
        };
        let err = match filesystem_transport(&device) {
            Ok(_) => panic!("mtp should have no transport in phase 1"),
            Err(e) => e,
        };
        assert!(err.contains("mtp"), "{err}");
    }
}
