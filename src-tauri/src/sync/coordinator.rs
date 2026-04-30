//! Thin handle around the spawned SyncWorker — held in AppState.

use crate::db::Db;
use crate::sync::worker::{SyncCommand, SyncWorker};
use std::sync::Arc;
use tauri::AppHandle;

pub struct SyncCoordinator {
    worker: SyncWorker,
}

impl SyncCoordinator {
    pub fn new(db: Arc<Db>, app: AppHandle) -> Self {
        Self {
            worker: SyncWorker::spawn(db, app),
        }
    }

    pub fn run_now(&self, source_id: i64) -> Result<(), String> {
        self.worker
            .tx
            .send(SyncCommand::RunNow { source_id })
            .map_err(|_| "sync worker has exited".to_string())
    }
}
