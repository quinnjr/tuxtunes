//! App-wide runtime state.

use crate::db::{Db, DbError};
use crate::device::coordinator::DeviceCoordinator;
use crate::fs::coordinator::FsCoordinator;
use crate::playback::{EngineError, PlaybackEngine};
use crate::sync::coordinator::SyncCoordinator;
use std::path::Path;
use std::sync::Arc;
use tauri::{AppHandle, Runtime};

pub struct AppState {
    pub db: Arc<Db>,
    pub engine: Arc<PlaybackEngine>,
    pub sync: Arc<SyncCoordinator>,
    pub fs: Arc<FsCoordinator>,
    /// Outbound sync to MTP devices and mounted storage.
    pub devices: Arc<DeviceCoordinator>,
}

#[derive(Debug, thiserror::Error)]
pub enum AppStateError {
    #[error(transparent)]
    Db(#[from] DbError),

    #[error(transparent)]
    Engine(#[from] EngineError),
}

impl AppState {
    /// Construct AppState for any Tauri runtime — Wry in production,
    /// MockRuntime in tests. The components that hold the AppHandle
    /// (PlaybackEngine, FsCoordinator, SyncCoordinator) are each
    /// generic over `R: Runtime` and erase the runtime as soon as
    /// they capture the handle into their worker threads.
    pub async fn new<R: Runtime>(db_path: &Path, app: AppHandle<R>) -> Result<Self, AppStateError> {
        let db = Arc::new(Db::open(db_path).await?);
        // Seed mpv with the saved volume so its boot value (which the
        // tracking consumer persists on first observation) is the
        // user's level, not 100 — restoring it afterwards raced that
        // first write and could overwrite the saved value.
        let initial_volume =
            crate::db::preferences::get::<i64>(&db.engine, crate::db::preferences::KEY_VOLUME)
                .await
                .ok()
                .flatten()
                .map(|v| v.clamp(0, 100) as u8);
        let engine = Arc::new(PlaybackEngine::spawn(app.clone(), initial_volume)?);
        let fs = Arc::new(FsCoordinator::new(Arc::clone(&db.engine), app.clone()));
        let sync = Arc::new(SyncCoordinator::new(
            Arc::clone(&db),
            Arc::clone(&fs),
            app.clone(),
        ));
        let devices = Arc::new(DeviceCoordinator::new(Arc::clone(&db), app));
        Ok(Self {
            db,
            engine,
            sync,
            fs,
            devices,
        })
    }
}
