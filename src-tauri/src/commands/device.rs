//! Tauri commands for outbound device sync.

use crate::db::devices::{self, DeviceRow, DeviceSettings, SelectionEntry};
use crate::device::engine::{self, SharedTransport};
use crate::device::transport::fs::FsTransport;
use crate::device::transport::StorageInfo;
use crate::runtime::AppState;
use std::path::PathBuf;
use std::sync::Arc;

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
        // Not weak. A mount path *is* reusable, but the user picked
        // this folder deliberately, and the manifest already provides
        // the real protection: only objects TuxTunes recorded writing
        // are ever deletion candidates, and the upload path refuses to
        // touch anything it has no row for. Marking every filesystem
        // device weak disabled pruning outright — and filesystem is the
        // only kind with a transport today, so it made `mirror_deletes`
        // a no-op and the settings checkbox a lie.
        false,
    )
    .await
    .map_err(|e| e.to_string())?;

    if let Some(root) = args.root_path {
        validate_root_path(&root)?;
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

/// Prompt for a device's mount point and register it.
///
/// The native folder picker is how a user actually reaches this
/// feature: a device shows up as a gvfs/mtpfs mount, an SD card, or a
/// DAP in mass-storage mode, and all three are just a directory.
/// Returns `None` when the dialog is dismissed.
#[tauri::command]
pub async fn pick_and_add_device(
    app: tauri::AppHandle,
    state: tauri::State<'_, AppState>,
) -> Result<Option<i64>, String> {
    use tauri_plugin_dialog::DialogExt;

    let Some(folder) = app.dialog().file().blocking_pick_folder() else {
        return Ok(None);
    };
    let mount = folder.into_path().map_err(|e| e.to_string())?;
    let mount_path = mount.to_string_lossy().to_string();

    // Name it after the mount's last component — "SANDISK", "FiiO
    // M11" — which is what the user sees in their file manager. The
    // device settings panel can rename it, and a re-detect will not
    // revert that.
    let name = mount
        .file_name()
        .map(|n| n.to_string_lossy().to_string())
        .filter(|n| !n.is_empty())
        .unwrap_or_else(|| "Device".to_string());

    let id = add_filesystem_device(
        state,
        AddFilesystemDeviceArgs {
            name,
            mount_path,
            root_path: None,
        },
    )
    .await?;
    Ok(Some(id))
}

/// Re-stat every known filesystem device and report which are present.
///
/// Sets `last_seen_at` for devices whose mount is still a directory and
/// **clears** it for those that are not, so the sidebar can dim a
/// device that has been unplugged. Without the clear, `last_seen_at`
/// would be set once at insert and every device would read as attached
/// forever.
///
/// Phase 1 has no hotplug enumeration; that arrives with the MTP
/// backend. Returns the refreshed list so the UI has one round trip.
#[tauri::command]
pub async fn refresh_devices(state: tauri::State<'_, AppState>) -> Result<Vec<DeviceRow>, String> {
    let rows = devices::list(&state.db.engine)
        .await
        .map_err(|e| e.to_string())?;
    for row in &rows {
        // Only filesystem devices can be probed this way. Leave other
        // kinds alone rather than declaring them absent.
        if row.kind != "filesystem" {
            continue;
        }
        // `is_dir` stats the mount, which blocks on a wedged one.
        let mount = row.mount_path.clone();
        let present =
            tokio::task::spawn_blocking(move || mount.is_some_and(|p| PathBuf::from(p).is_dir()))
                .await
                .map_err(|e| e.to_string())?;
        devices::set_seen(&state.db.engine, row.id, present)
            .await
            .map_err(|e| e.to_string())?;
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

/// Reject a device root that cannot address anything on the device.
///
/// `root_path` is free text from the settings panel, and every track
/// path is built under it — a `..` segment fails 100% of uploads with a
/// per-track permission error and no hint as to why.
fn validate_root_path(root: &str) -> Result<(), String> {
    let path = crate::device::transport::DevicePath::new(root);
    for segment in path.as_str().split('/').filter(|s| !s.is_empty()) {
        if segment == ".." || segment == "." || segment.contains('\\') || segment.contains(':') {
            return Err(format!(
                "'{root}' is not a valid device folder: \
                 use a plain path like /Music"
            ));
        }
    }
    Ok(())
}

#[tauri::command]
pub async fn update_device_settings(
    state: tauri::State<'_, AppState>,
    device_id: i64,
    settings: DeviceSettings,
) -> Result<(), String> {
    validate_root_path(&settings.root_path)?;
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
    // Constructing the transport canonicalises the mount, which blocks
    // on a wedged one; keep it off the runtime like every other
    // filesystem call in this file.
    let row = device.clone();
    let transport = tokio::task::spawn_blocking(move || filesystem_transport(&row))
        .await
        .map_err(|e| e.to_string())??;
    let (plan, skips, _notes) = engine::build_plan(&state.db.engine, &device, &transport)
        .await
        .map_err(|e| e.to_string())?;
    // statvfs blocks uninterruptibly on a wedged mount, so it must not
    // run inline on the async runtime: repeated Preview clicks would
    // otherwise consume a tokio worker each and starve the shared
    // runtime that also serves the DB and playback.
    let storage: Option<StorageInfo> = if transport.capabilities().free_space {
        let t = Arc::clone(&transport);
        tokio::task::spawn_blocking(move || t.free_space().ok())
            .await
            .map_err(|e| e.to_string())?
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
fn filesystem_transport(device: &DeviceRow) -> Result<SharedTransport, String> {
    if device.kind != "filesystem" {
        return Err(format!(
            "no transport for device kind '{}' yet",
            device.kind
        ));
    }
    let mount = device
        .mount_path
        .as_deref()
        .filter(|m| !m.is_empty())
        .ok_or_else(|| "filesystem device has no mount path".to_string())?;
    Ok(Arc::new(FsTransport::new(PathBuf::from(mount))))
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
            false,
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
        let (plan, skips, _) = engine::build_plan(&db.engine, &device, &t).await.unwrap();
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
        let (plan, _, _) = engine::build_plan(&db.engine, &device, &t).await.unwrap();

        assert_eq!(plan.adds.len(), 1);
        assert_eq!(plan.bytes_out, 10);
        assert_eq!(
            std::fs::read_dir(mount.path()).unwrap().count(),
            0,
            "a dry run must not touch the device"
        );
    }

    #[test]
    fn a_root_path_that_cannot_address_the_device_is_rejected() {
        for bad in ["/../Music", "../Music", "/Music/./x", r"/C:\Music", r"/a\b"] {
            assert!(
                validate_root_path(bad).is_err(),
                "{bad:?} should be refused before it fails every upload"
            );
        }
        for good in ["/Music", "Music", "/", "/Storage/Music"] {
            assert!(validate_root_path(good).is_ok(), "{good:?} should be fine");
        }
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
