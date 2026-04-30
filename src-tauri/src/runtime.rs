//! App-wide runtime state.

use crate::db::{Db, DbError};
use crate::playback::{EngineError, PlaybackEngine};
use std::path::Path;
use std::sync::Arc;
use tauri::AppHandle;

pub struct AppState {
    pub db: Arc<Db>,
    pub engine: Arc<PlaybackEngine>,
}

#[derive(Debug, thiserror::Error)]
pub enum AppStateError {
    #[error(transparent)]
    Db(#[from] DbError),

    #[error(transparent)]
    Engine(#[from] EngineError),
}

impl AppState {
    pub async fn new(db_path: &Path, app: AppHandle) -> Result<Self, AppStateError> {
        let db = Arc::new(Db::open(db_path).await?);
        let engine = Arc::new(PlaybackEngine::spawn(app)?);
        Ok(Self { db, engine })
    }
}
