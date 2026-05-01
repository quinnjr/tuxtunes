//! Handle for file-management workers. Held in AppState.

use crate::fs::ingest::{IngestCommand, IngestWorker};
use prax_sqlite::raw::SqliteRawEngine;
use std::path::PathBuf;
use std::sync::Arc;
use tauri::{AppHandle, Runtime};

pub struct FsCoordinator {
    ingest: IngestWorker,
}

impl FsCoordinator {
    pub fn new<R: Runtime>(engine: Arc<SqliteRawEngine>, app: AppHandle<R>) -> Self {
        Self {
            ingest: IngestWorker::spawn(engine, app),
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
}
