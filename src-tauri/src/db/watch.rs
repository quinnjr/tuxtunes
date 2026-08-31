//! External-change detection for the library database.
//!
//! `PRAGMA data_version` increments (as seen by a given connection)
//! whenever a commit lands from any *other* connection — another
//! process (`tuxtunes-cli`, direct sqlite edits) or another pooled
//! connection in this process. The app holds one dedicated connection
//! and polls it; when the version moves, the UI is told to refresh.
//! Refreshes triggered by the app's own writes are redundant but
//! harmless — the queries are idempotent.

use prax_sqlite::raw::SqliteRawEngine;
use prax_sqlite::SqliteConnection;

/// How often the app polls for external changes.
pub const POLL_INTERVAL: std::time::Duration = std::time::Duration::from_secs(2);

#[derive(Debug, thiserror::Error)]
pub enum WatchError {
    #[error("data_version query failed: {0}")]
    Query(#[source] anyhow::Error),
}

/// Read `PRAGMA data_version` on the watcher's dedicated connection.
pub async fn data_version(conn: &SqliteConnection) -> Result<i64, WatchError> {
    let rows = conn
        .query("PRAGMA data_version")
        .await
        .map_err(|e| WatchError::Query(anyhow::Error::from(e)))?;
    rows.first()
        .and_then(|r| r.as_object())
        .and_then(|o| o.values().next())
        .and_then(|v| v.as_i64())
        .ok_or_else(|| WatchError::Query(anyhow::anyhow!("PRAGMA data_version returned no row")))
}

/// Check out (and keep) the watcher's connection. Holding it is the
/// point: `data_version` is per-connection, so the baseline must never
/// move between polls.
pub async fn checkout(engine: &SqliteRawEngine) -> Result<SqliteConnection, WatchError> {
    engine
        .pool()
        .get()
        .await
        .map_err(|e| WatchError::Query(anyhow::Error::from(e)))
}

/// One poll step: true (and a new baseline) when another connection has
/// committed since `last`.
pub async fn changed_since(
    conn: &SqliteConnection,
    last: i64,
) -> Result<(i64, bool), WatchError> {
    let now = data_version(conn).await?;
    Ok((now, now != last))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::Db;

    #[tokio::test]
    async fn data_version_moves_when_another_connection_writes() {
        let tmp = tempfile::NamedTempFile::new().unwrap();
        let db = Db::open(tmp.path()).await.unwrap();
        let watcher = checkout(&db.engine).await.unwrap();
        let baseline = data_version(&watcher).await.unwrap();

        // No writes → no change.
        let (v, changed) = changed_since(&watcher, baseline).await.unwrap();
        assert!(!changed, "no commit happened, version must hold");
        assert_eq!(v, baseline);

        // A write through the engine lands on a *different* pooled
        // connection (the watcher's is checked out), i.e. it looks
        // exactly like an external writer.
        db.engine
            .raw_sql_execute(
                "INSERT INTO preferences (key, value) VALUES ('watch_test', '1')",
                &[],
            )
            .await
            .unwrap();

        let (v2, changed) = changed_since(&watcher, baseline).await.unwrap();
        assert!(changed, "a foreign commit must move data_version");
        assert_ne!(v2, baseline);

        // The new baseline holds until the next foreign commit.
        let (_, changed) = changed_since(&watcher, v2).await.unwrap();
        assert!(!changed);
    }
}
