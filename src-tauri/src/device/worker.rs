//! Single-task device sync worker.
//!
//! Reads [`DeviceCommand`]s from an unbounded channel and runs one sync
//! at a time, mirroring [`crate::sync::worker`]. Serialising runs is
//! deliberate: two syncs sharing a device would race on the manifest,
//! and MTP transports tolerate exactly one active session per device.

use super::engine::{self, EngineError, SharedTransport};
use super::events::{DeviceComplete, DeviceFailed, DeviceLogLine, DevicePhase, DeviceProgress};
use super::observer::{DeviceObserver, TauriObserver};
use super::transport::fs::FsTransport;
use crate::db::devices::{self, DeviceRow};
use crate::db::Db;
use crate::sync::import_log::{ImportLog, LogLevel, LogSink};
use crate::sync::log_tailer::LogTailer;
use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use tauri::{AppHandle, Emitter, Runtime};
use tokio::sync::mpsc;

#[derive(Debug)]
pub enum DeviceCommand {
    RunNow { device_id: i64 },
}

/// Cancellation flags, keyed by device id, shared with the UI thread.
type CancelFlags = Arc<Mutex<HashMap<i64, Arc<AtomicBool>>>>;

/// Devices with a run queued or in flight.
type QueuedSet = Arc<Mutex<HashSet<i64>>>;

pub struct DeviceWorker {
    tx: mpsc::UnboundedSender<DeviceCommand>,
    cancels: CancelFlags,
    queued: QueuedSet,
    _task: tokio::task::JoinHandle<()>,
}

impl DeviceWorker {
    pub fn spawn<R: Runtime>(db: Arc<Db>, app: AppHandle<R>) -> Self {
        let (tx, mut rx) = mpsc::unbounded_channel::<DeviceCommand>();
        let cancels: CancelFlags = Arc::new(Mutex::new(HashMap::new()));
        let queued: QueuedSet = Arc::new(Mutex::new(HashSet::new()));
        let db_clone = Arc::clone(&db);
        let cancels_clone = Arc::clone(&cancels);
        let queued_clone = Arc::clone(&queued);
        let task = tokio::spawn(async move {
            while let Some(cmd) = rx.recv().await {
                match cmd {
                    DeviceCommand::RunNow { device_id } => {
                        let flag = flag_for(&cancels_clone, device_id);
                        // Deliberately NOT cleared here. A cancel
                        // issued between enqueue and dequeue is the
                        // user asking for *this* run to stop; clearing
                        // it would silently resume writing to the
                        // device after they pressed Stop. `run_now`
                        // clears it at enqueue time instead.
                        run_one(&db_clone, &app, device_id, &flag).await;
                        queued_clone
                            .lock()
                            .expect("queued set mutex poisoned")
                            .remove(&device_id);
                    }
                }
            }
        });
        Self {
            tx,
            cancels,
            queued,
            _task: task,
        }
    }

    /// Queue a run for `device_id`.
    ///
    /// A device already queued or in flight is ignored: the Sync button
    /// only disables once a `device:progress` event has round-tripped,
    /// so a double-click would otherwise enqueue a second run that
    /// resumes writing right after the first one was cancelled.
    pub fn run_now(&self, device_id: i64) -> Result<(), String> {
        {
            let mut queued = self.queued.lock().expect("queued set mutex poisoned");
            if !queued.insert(device_id) {
                return Ok(());
            }
        }
        // Clear the cancel flag at enqueue time, so a cancel arriving
        // after this point always belongs to the run being queued.
        flag_for(&self.cancels, device_id).store(false, Ordering::Relaxed);

        if let Err(e) = self.tx.send(DeviceCommand::RunNow { device_id }) {
            self.queued
                .lock()
                .expect("queued set mutex poisoned")
                .remove(&device_id);
            return Err(format!("device worker has exited: {e}"));
        }
        Ok(())
    }

    /// Ask the in-flight sync for `device_id` to stop at the next
    /// object boundary. A no-op if nothing is running for it.
    pub fn cancel(&self, device_id: i64) {
        flag_for(&self.cancels, device_id).store(true, Ordering::Relaxed);
    }
}

fn flag_for(cancels: &CancelFlags, device_id: i64) -> Arc<AtomicBool> {
    Arc::clone(
        cancels
            .lock()
            .expect("cancel flag mutex poisoned")
            .entry(device_id)
            .or_insert_with(|| Arc::new(AtomicBool::new(false))),
    )
}

/// Build the transport for a device row.
///
/// The MTP and WPD backends land in later phases; until then, asking
/// for one is a clean, explicit failure rather than a silent no-op.
fn transport_for(device: &DeviceRow) -> Result<SharedTransport, EngineError> {
    match device.kind.as_str() {
        "filesystem" => {
            let mount = device.mount_path.as_deref().unwrap_or_default();
            if mount.is_empty() {
                return Err(EngineError::TransportUnavailable(
                    "filesystem device has no mount path".into(),
                ));
            }
            Ok(Arc::new(FsTransport::new(PathBuf::from(mount))))
        }
        other => Err(EngineError::TransportUnavailable(other.to_string())),
    }
}

/// One GUI run: open the per-run log and its live tailer, build the
/// observer, drive the engine, and emit the terminal event.
async fn run_one<R: Runtime>(
    db: &Arc<Db>,
    app: &AppHandle<R>,
    device_id: i64,
    cancel: &Arc<AtomicBool>,
) {
    let obs = TauriObserver::new(app.clone());

    // A missing log must never fail a sync.
    let log = match ImportLog::create_named(app, "device", device_id) {
        Ok(l) => Some(l),
        Err(e) => {
            log::warn!("device log unavailable: {e}");
            None
        }
    };

    let tailer = log.as_ref().map(|l| {
        let app = app.clone();
        LogTailer::spawn(l.path().to_path_buf(), move |seq, line| {
            let _ = app.emit(
                super::events::LOG,
                DeviceLogLine {
                    device_id,
                    seq,
                    line,
                },
            );
        })
    });

    let write = |level: LogLevel, msg: &str| {
        if let Some(l) = &log {
            l.write(level, msg);
        }
    };

    match sync_device(db, &obs, device_id, cancel, &write).await {
        Ok(done) => {
            write(LogLevel::Info, &format!("complete: {done:?}"));
            obs.complete(&done);
        }
        Err(e) => {
            write(LogLevel::Warn, &format!("failed: {e}"));
            obs.failed(&DeviceFailed {
                device_id,
                error: e.to_string(),
            });
        }
    }

    // Drain before stopping so the closing lines reach the UI.
    if let Some(t) = tailer {
        t.stop().await;
    }
}

/// Load the device, build its transport, and run the engine.
async fn sync_device(
    db: &Arc<Db>,
    obs: &dyn DeviceObserver,
    device_id: i64,
    cancel: &Arc<AtomicBool>,
    write: LogSink<'_>,
) -> Result<DeviceComplete, EngineError> {
    let device = devices::get(&db.engine, device_id)
        .await
        .map_err(|e| EngineError::Db(anyhow::Error::from(e)))?;
    write(LogLevel::Info, &format!("syncing '{}'", device.name));

    // `FsTransport::new` canonicalises the root, which blocks on a
    // wedged mount — and does so before the first `op()` deadline could
    // apply, so it goes to the blocking pool too.
    let row = device.clone();
    let transport = tokio::task::spawn_blocking(move || transport_for(&row))
        .await
        .map_err(|e| EngineError::Db(anyhow::Error::from(e)))??;
    obs.progress(&DeviceProgress {
        device_id,
        phase: DevicePhase::Enumerating,
        current: 0,
        total: 1,
        message: format!("opening {}", device.name),
    });

    engine::run(&db.engine, &transport, obs, &device, cancel).await
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::devices;

    async fn tmp_db() -> (tempfile::NamedTempFile, Arc<Db>) {
        let f = tempfile::NamedTempFile::new().unwrap();
        let db = Arc::new(Db::open(f.path()).await.unwrap());
        (f, db)
    }

    fn row(kind: &str, mount: Option<&str>) -> DeviceRow {
        DeviceRow {
            id: 1,
            name: "D".into(),
            kind: kind.into(),
            device_key: "k".into(),
            key_is_weak: false,
            root_path: "/Music".into(),
            mount_path: mount.map(str::to_string),
            last_seen_at: None,
            last_sync_at: None,
            selection: Vec::new(),
            layout_template: "{title}.{ext}".into(),
            auto_sync: false,
            mirror_deletes: true,
            write_playlist_objects: true,
        }
    }

    #[test]
    fn a_filesystem_device_gets_an_fs_transport() {
        assert!(transport_for(&row("filesystem", Some("/mnt/dap"))).is_ok());
    }

    /// `Box<dyn DeviceTransport>` is not `Debug`, so unwrap the result
    /// by hand rather than through `unwrap_err`.
    fn transport_error(device: &DeviceRow) -> EngineError {
        match transport_for(device) {
            Ok(_) => panic!("expected no transport for kind '{}'", device.kind),
            Err(e) => e,
        }
    }

    #[test]
    fn a_filesystem_device_without_a_mount_is_rejected() {
        let err = transport_error(&row("filesystem", None));
        assert!(
            matches!(err, EngineError::TransportUnavailable(_)),
            "{err:?}"
        );
    }

    #[test]
    fn mtp_and_wpd_are_not_available_yet() {
        for kind in ["mtp", "wpd"] {
            let err = transport_error(&row(kind, None));
            assert!(
                matches!(err, EngineError::TransportUnavailable(ref k) if k == kind),
                "{err:?}"
            );
        }
    }

    #[tokio::test]
    async fn syncing_an_unknown_device_fails_cleanly() {
        let (_f, db) = tmp_db().await;
        let obs = crate::device::observer::NoopObserver;
        let err = sync_device(
            &db,
            &obs,
            4242,
            &Arc::new(AtomicBool::new(false)),
            &|_, _| {},
        )
        .await
        .unwrap_err();
        assert!(matches!(err, EngineError::Db(_)), "{err:?}");
    }

    #[tokio::test]
    async fn a_filesystem_device_syncs_end_to_end_through_the_worker_path() {
        let (_f, db) = tmp_db().await;
        let mount = tempfile::tempdir().unwrap();
        devices::upsert_by_key(
            &db.engine,
            "fs:test",
            "DAP",
            "filesystem",
            mount.path().to_str(),
            false,
        )
        .await
        .unwrap();
        let obs = crate::device::observer::NoopObserver;
        let done = sync_device(&db, &obs, 1, &Arc::new(AtomicBool::new(false)), &|_, _| {})
            .await
            .expect("an empty selection is a valid, no-op sync");
        assert_eq!(done.added, 0);
    }

    #[tokio::test]
    async fn cancel_sets_the_flag_for_that_device_only() {
        let (_f, db) = tmp_db().await;
        let app: tauri::App<tauri::test::MockRuntime> = tauri::test::mock_app();
        let worker = DeviceWorker::spawn(db, app.handle().clone());
        worker.cancel(7);
        assert!(flag_for(&worker.cancels, 7).load(Ordering::Relaxed));
        assert!(!flag_for(&worker.cancels, 8).load(Ordering::Relaxed));
    }
}
