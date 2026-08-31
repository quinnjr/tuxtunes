//! Thin handle around the spawned [`DeviceWorker`] — held in AppState.

use super::worker::{DeviceCommand, DeviceWorker};
use crate::db::Db;
use std::sync::Arc;
use tauri::{AppHandle, Runtime};

pub struct DeviceCoordinator {
    worker: DeviceWorker,
}

impl DeviceCoordinator {
    pub fn new<R: Runtime>(db: Arc<Db>, app: AppHandle<R>) -> Self {
        Self {
            worker: DeviceWorker::spawn(db, app),
        }
    }

    pub fn run_now(&self, device_id: i64) -> Result<(), String> {
        self.worker
            .tx
            .send(DeviceCommand::RunNow { device_id })
            .map_err(|_| "device worker has exited".to_string())
    }

    /// Request that the in-flight sync for `device_id` stop at the next
    /// object boundary. Safe to call when nothing is running.
    pub fn cancel(&self, device_id: i64) -> Result<(), String> {
        self.worker.cancel(device_id);
        Ok(())
    }
}
