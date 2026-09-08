//! Handle for file-management workers. Held in AppState.

use crate::fs::convert::{ConvertCommand, ConvertFormat, ConvertPrefs, ConvertWorker};
use crate::fs::ingest::{IngestCommand, IngestWorker};
use crate::fs::organize::{OrganizeCommand, OrganizeWorker};
use prax_sqlite::raw::SqliteRawEngine;
use std::path::PathBuf;
use std::sync::Arc;
use tauri::{AppHandle, Runtime};

pub struct FsCoordinator {
    ingest: IngestWorker,
    organize: OrganizeWorker,
    convert: ConvertWorker,
}

impl FsCoordinator {
    pub fn new<R: Runtime>(engine: Arc<SqliteRawEngine>, app: AppHandle<R>) -> Self {
        let ingest = IngestWorker::spawn(Arc::clone(&engine), app.clone());
        let ingest_tx = ingest.tx.clone();
        Self {
            ingest,
            organize: OrganizeWorker::spawn(Arc::clone(&engine), app.clone()),
            convert: ConvertWorker::spawn(engine, ingest_tx, app),
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

    /// Queue the bulk consolidate pass. Returns as soon as it is
    /// queued; progress arrives on `fs:consolidate-progress` and the
    /// summary on `fs:consolidate-complete`.
    pub fn consolidate_library(&self) -> Result<(), String> {
        self.ingest
            .tx
            .send(IngestCommand::ConsolidateAll)
            .map_err(|_| "ingest worker has exited".to_string())
    }

    /// Queue the reclaim pass. Progress arrives on
    /// `fs:reclaim-progress`, the summary on `fs:reclaim-complete`.
    pub fn reclaim_originals(&self) -> Result<(), String> {
        self.ingest
            .tx
            .send(IngestCommand::ReclaimOriginals)
            .map_err(|_| "ingest worker has exited".to_string())
    }

    /// Stop the batch in flight and drop everything queued behind it.
    /// The flag stays set until the next [`Self::convert_tracks`], so a
    /// cancel cannot leak into a batch the user asks for afterwards.
    pub fn cancel_convert(&self) -> Result<(), String> {
        self.convert
            .cancel
            .send(true)
            .map_err(|_| "convert worker has exited".to_string())
    }

    /// Queue a transcode batch. Progress arrives on
    /// `fs:convert-progress`, per-file errors on `fs:convert-failed`,
    /// and the tally on `fs:convert-complete`.
    pub fn convert_tracks(
        &self,
        track_ids: Vec<i64>,
        format: ConvertFormat,
        prefs: ConvertPrefs,
    ) -> Result<(), String> {
        // Clear any cancel left over from a previous batch before this
        // one is visible to the worker.
        self.convert
            .cancel
            .send(false)
            .map_err(|_| "convert worker has exited".to_string())?;
        self.convert
            .tx
            .send(ConvertCommand::Tracks {
                track_ids,
                format,
                prefs,
            })
            .map_err(|_| "convert worker has exited".to_string())
    }

    pub fn reorganize_track(&self, track_id: i64) -> Result<(), String> {
        self.organize
            .tx
            .send(OrganizeCommand::ReorganizeTrack { track_id })
            .map_err(|_| "organize worker has exited".to_string())
    }
}
