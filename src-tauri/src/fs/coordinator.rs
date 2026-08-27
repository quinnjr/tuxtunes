//! Handle for file-management workers. Held in AppState.

use crate::fs::ingest::{IngestCommand, IngestWorker};
use crate::fs::organize::{OrganizeCommand, OrganizeWorker};
use prax_sqlite::raw::SqliteRawEngine;
use std::path::PathBuf;
use std::sync::Arc;
use tauri::{AppHandle, Runtime};

pub struct FsCoordinator {
    ingest: IngestWorker,
    organize: OrganizeWorker,
}

impl FsCoordinator {
    pub fn new<R: Runtime>(engine: Arc<SqliteRawEngine>, app: AppHandle<R>) -> Self {
        Self {
            ingest: IngestWorker::spawn(Arc::clone(&engine), app.clone()),
            organize: OrganizeWorker::spawn(engine, app),
        }
    }

    pub fn copy_for_track(&self, track_id: i64, source_path: PathBuf) -> Result<(), String> {
        self.ingest
            .tx
            .send(IngestCommand::CopyForTrack {
                track_id,
                source_path,
            })
            .map_err(|_| "ingest worker has exited".to_string())
    }

    pub fn reorganize_track(&self, track_id: i64) -> Result<(), String> {
        self.organize
            .tx
            .send(OrganizeCommand::ReorganizeTrack { track_id })
            .map_err(|_| "organize worker has exited".to_string())
    }
}
