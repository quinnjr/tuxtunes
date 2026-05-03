//! App-wide runtime state.

use crate::db::{Db, DbError};
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
        let engine = Arc::new(PlaybackEngine::spawn(app.clone())?);
        let fs = Arc::new(FsCoordinator::new(Arc::clone(&db.engine), app.clone()));
        let sync = Arc::new(SyncCoordinator::new(Arc::clone(&db), Arc::clone(&fs), app));
        Ok(Self {
            db,
            engine,
            sync,
            fs,
        })
    }
}
