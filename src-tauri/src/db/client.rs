//! Database client wrapper over `prax-sqlite`.
//!
//! On open, applies the bundled migration SQL from
//! `src-tauri/prax/migrations/0001_initial/migration.sql`. The migration is
//! idempotent for fresh databases and a no-op for already-migrated ones
//! (the loader inspects sqlite_master before applying).

use prax_sqlite::{SqliteConfig, SqliteEngine, SqlitePool};
use std::path::Path;

const INITIAL_MIGRATION: &str = include_str!("../../prax/migrations/0001_initial/migration.sql");

#[derive(Debug, thiserror::Error)]
pub enum DbError {
    #[error("failed to open database at {path}: {source}")]
    Open {
        path: String,
        #[source]
        source: anyhow::Error,
    },

    #[error("failed to apply migration: {0}")]
    Migrate(#[source] anyhow::Error),

    #[error("query failed: {0}")]
    Query(#[source] anyhow::Error),
}

pub struct Db {
    /// Exposed for query execution by Tauri commands; first used in Task 13.
    #[allow(dead_code)]
    pub engine: SqliteEngine,
}

impl Db {
    /// Open the database at `db_path`, creating the file if necessary, and
    /// apply the initial migration if the core tables are not yet present.
    pub async fn open(db_path: &Path) -> Result<Self, DbError> {
        let config = SqliteConfig::file(db_path);

        let pool = SqlitePool::new(config).await.map_err(|e| DbError::Open {
            path: db_path.display().to_string(),
            source: anyhow::Error::from(e),
        })?;

        let engine = SqliteEngine::new(pool);

        apply_initial_migration(&engine).await?;

        Ok(Self { engine })
    }
}

/// Check whether the core schema has been applied by looking for the `tracks`
/// table in sqlite_master. If absent, run the migration.
async fn apply_initial_migration(engine: &SqliteEngine) -> Result<(), DbError> {
    let count: i64 = engine
        .raw_sql_scalar(
            "SELECT COUNT(*) FROM sqlite_master \
             WHERE type = 'table' AND name = 'tracks'",
            &[],
        )
        .await
        .map_err(|e| DbError::Query(anyhow::Error::from(e)))?;

    if count == 0 {
        engine
            .raw_sql_batch(INITIAL_MIGRATION)
            .await
            .map_err(|e| DbError::Migrate(anyhow::Error::from(e)))?;
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn open_creates_schema_in_temp_db() {
        let tmp = tempfile::NamedTempFile::new().unwrap();
        let db = Db::open(tmp.path()).await.expect("open should succeed");

        let count: i64 = db
            .engine
            .raw_sql_scalar("SELECT COUNT(*) FROM tracks", &[])
            .await
            .expect("tracks table queryable");
        assert_eq!(count, 0, "freshly migrated DB has no rows");
    }

    #[tokio::test]
    async fn open_is_idempotent_on_reopen() {
        let tmp = tempfile::NamedTempFile::new().unwrap();
        // First open creates schema
        {
            let _db = Db::open(tmp.path()).await.expect("first open");
        }
        // Second open should succeed without re-applying the migration
        let db = Db::open(tmp.path()).await.expect("second open");
        let count: i64 = db
            .engine
            .raw_sql_scalar("SELECT COUNT(*) FROM playlists", &[])
            .await
            .expect("playlists table queryable");
        assert_eq!(count, 0);
    }
}
