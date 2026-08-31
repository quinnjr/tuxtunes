//! Database client wrapper over `prax-sqlite`.
//!
//! On open, applies the bundled migration SQL from
//! `src-tauri/prax/migrations/0001_initial/migration.sql` and
//! `src-tauri/prax/migrations/0002_composite_sync_indexes/migration.sql`.
//!
//! Applied migrations are tracked in a `schema_migrations` ledger table
//! (`name`, `applied_at`), created if absent on every open. Migrations are
//! applied in order, skipping any whose name is already recorded in the
//! ledger. For a migration with no ledger row, a backfill probe (marker SQL
//! that inspects `sqlite_master` for objects the migration creates) checks
//! whether it was already applied by an older version of this loader that
//! predates the ledger; if so, only the ledger row is inserted and the
//! migration SQL itself is not re-run. This backfill path matters most for
//! 0001, whose `CREATE TABLE` statements are not safe to rerun; 0002's
//! `CREATE INDEX IF NOT EXISTS` statements are idempotent regardless.

use prax_query::filter::FilterValue;
use prax_sqlite::raw::SqliteRawEngine;
use prax_sqlite::{SqliteConfig, SqlitePool};
use std::path::Path;
use std::sync::Arc;

const INITIAL_MIGRATION: &str = include_str!("../../prax/migrations/0001_initial/migration.sql");
const COMPOSITE_SYNC_INDEXES_MIGRATION: &str =
    include_str!("../../prax/migrations/0002_composite_sync_indexes/migration.sql");
const PLAYLIST_LOCAL_EDITS_MIGRATION: &str =
    include_str!("../../prax/migrations/0003_playlist_local_edits/migration.sql");
const TRACK_USER_EDITS_MIGRATION: &str =
    include_str!("../../prax/migrations/0004_track_user_edits/migration.sql");

/// A single migration entry: a stable name (the ledger key), the SQL batch
/// to run, and a backfill probe used only when the ledger has no row for
/// this migration yet.
struct Migration {
    /// Stable identifier stored in `schema_migrations.name`.
    name: &'static str,
    /// SQL batch applied via `raw_sql_batch` when the migration has not yet
    /// been applied.
    sql: &'static str,
    /// Scalar SQL counting the sqlite_master objects this migration
    /// creates. Used only as a backfill check for pre-ledger databases.
    marker_sql: &'static str,
    /// Expected `marker_sql` count when the migration has already been
    /// fully applied.
    marker_count: i64,
}

static MIGRATIONS: &[Migration] = &[
    Migration {
        name: "0001_initial",
        sql: INITIAL_MIGRATION,
        marker_sql: "SELECT COUNT(*) FROM sqlite_master WHERE type = 'table' AND name = 'tracks'",
        marker_count: 1,
    },
    Migration {
        name: "0002_composite_sync_indexes",
        sql: COMPOSITE_SYNC_INDEXES_MIGRATION,
        marker_sql: "SELECT COUNT(*) FROM sqlite_master \
             WHERE type = 'index' AND name IN (\
             'idx_tracks_sync_source_id_persistent_id', \
             'idx_playlists_sync_source_id_persistent_id')",
        marker_count: 2,
    },
    Migration {
        name: "0003_playlist_local_edits",
        sql: PLAYLIST_LOCAL_EDITS_MIGRATION,
        marker_sql: "SELECT COUNT(*) FROM sqlite_master \
             WHERE type = 'table' AND name = 'playlist_tombstones'",
        marker_count: 1,
    },
    Migration {
        name: "0004_track_user_edits",
        sql: TRACK_USER_EDITS_MIGRATION,
        marker_sql: "SELECT COUNT(*) FROM pragma_table_info('tracks') \
             WHERE name = 'user_edited'",
        marker_count: 1,
    },
];

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
    pub engine: Arc<SqliteRawEngine>,
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

        let engine = Arc::new(SqliteRawEngine::new(pool));

        apply_migrations(&engine).await?;

        Ok(Self { engine })
    }
}

/// Ensure the `schema_migrations` ledger exists, then apply each entry in
/// `MIGRATIONS` in order, skipping ones already recorded in the ledger. For
/// a migration with no ledger row, probe `marker_sql` first: if it reports
/// the migration's objects already exist (a pre-ledger database that
/// applied this migration under the old per-migration probe functions),
/// backfill the ledger row without re-running the SQL. Otherwise run the
/// migration SQL and then record it.
async fn apply_migrations(engine: &SqliteRawEngine) -> Result<(), DbError> {
    engine
        .raw_sql_batch(
            "CREATE TABLE IF NOT EXISTS schema_migrations (\
                 name TEXT PRIMARY KEY, \
                 applied_at TEXT NOT NULL DEFAULT (datetime('now')))",
        )
        .await
        .map_err(|e| DbError::Migrate(anyhow::Error::from(e)))?;

    for migration in MIGRATIONS {
        let ledger_count: i64 = engine
            .raw_sql_scalar(
                "SELECT COUNT(*) FROM schema_migrations WHERE name = ?",
                &[FilterValue::String(migration.name.to_string())],
            )
            .await
            .map_err(|e| DbError::Query(anyhow::Error::from(e)))?;

        if ledger_count > 0 {
            continue;
        }

        let marker_count: i64 = engine
            .raw_sql_scalar(migration.marker_sql, &[])
            .await
            .map_err(|e| DbError::Query(anyhow::Error::from(e)))?;

        if marker_count < migration.marker_count {
            engine
                .raw_sql_batch(migration.sql)
                .await
                .map_err(|e| DbError::Migrate(anyhow::Error::from(e)))?;
        }

        engine
            .raw_sql_execute(
                "INSERT INTO schema_migrations (name) VALUES (?)",
                &[FilterValue::String(migration.name.to_string())],
            )
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

    async fn composite_sync_index_count(engine: &SqliteRawEngine) -> i64 {
        engine
            .raw_sql_scalar(
                "SELECT COUNT(*) FROM sqlite_master \
                 WHERE type = 'index' AND name IN (\
                 'idx_tracks_sync_source_id_persistent_id', \
                 'idx_playlists_sync_source_id_persistent_id')",
                &[],
            )
            .await
            .expect("sqlite_master queryable")
    }

    #[tokio::test]
    async fn fresh_open_creates_both_composite_sync_indexes() {
        let tmp = tempfile::NamedTempFile::new().unwrap();
        let db = Db::open(tmp.path()).await.expect("open should succeed");

        assert_eq!(
            composite_sync_index_count(&db.engine).await,
            2,
            "both composite sync indexes should exist after a fresh open"
        );
    }

    #[tokio::test]
    async fn upgrade_from_pre_ledger_database_backfills_indexes_and_ledger() {
        let tmp = tempfile::NamedTempFile::new().unwrap();

        // Simulate a pre-0002, pre-ledger database: apply only the raw
        // 0001 migration directly via the engine, with no schema_migrations
        // table and no composite indexes.
        {
            let config = SqliteConfig::file(tmp.path());
            let pool = SqlitePool::new(config).await.expect("pool for setup");
            let engine = SqliteRawEngine::new(pool);
            engine
                .raw_sql_batch(INITIAL_MIGRATION)
                .await
                .expect("apply raw initial migration");

            assert_eq!(
                composite_sync_index_count(&engine).await,
                0,
                "pre-migration database should not have the composite indexes yet"
            );
        }

        // First Db::open should detect the pre-existing `tracks` table (via
        // the 0001 backfill probe), skip re-running 0001, and apply 0002
        // since its indexes are absent.
        let db = Db::open(tmp.path())
            .await
            .expect("upgrade open should succeed");

        assert_eq!(
            composite_sync_index_count(&db.engine).await,
            2,
            "both composite sync indexes should exist after upgrade"
        );

        let ledger_names: Vec<String> = db
            .engine
            .raw_sql_query("SELECT name FROM schema_migrations ORDER BY name", &[])
            .await
            .expect("schema_migrations queryable")
            .into_iter()
            .map(|row| {
                row.json()
                    .as_object()
                    .and_then(|obj| obj.get("name"))
                    .and_then(|v| v.as_str())
                    .expect("name column present")
                    .to_string()
            })
            .collect();
        assert_eq!(
            ledger_names,
            vec![
                "0001_initial".to_string(),
                "0002_composite_sync_indexes".to_string(),
                "0003_playlist_local_edits".to_string(),
                "0004_track_user_edits".to_string()
            ],
            "ledger should carry every migration after upgrade"
        );

        // Reopening again should remain a no-op / idempotent.
        drop(db);
        let db = Db::open(tmp.path()).await.expect("second upgrade open");
        assert_eq!(composite_sync_index_count(&db.engine).await, 2);
    }
}
