//! App-wide runtime state.
//!
//! Holds shared handles to the database client (and, later, the playback
//! engine + worker channels) behind `Arc` so they can live inside
//! `tauri::State` and be cheaply cloned into command handlers.

use crate::db::{Db, DbError};
use std::path::Path;
use std::sync::Arc;

pub struct AppState {
    /// Database handle provided to Tauri commands via `tauri::State`; first used in Task 13.
    pub db: Arc<Db>,
}

impl AppState {
    pub async fn new(db_path: &Path) -> Result<Self, DbError> {
        let db = Db::open(db_path).await?;
        Ok(Self { db: Arc::new(db) })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn appstate_initializes_with_temp_db() {
        let tmp = tempfile::NamedTempFile::new().unwrap();
        let state = AppState::new(tmp.path()).await.expect("init succeeds");

        let count: i64 = state
            .db
            .engine
            .raw_sql_scalar("SELECT COUNT(*) FROM tracks", &[])
            .await
            .expect("tracks table queryable");
        assert_eq!(count, 0);
    }
}
