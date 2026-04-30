//! Smoke test: load the user's real iTunes library via itl-rs and confirm
//! the basic accessors work. Skips if no library is present (keeps CI
//! stable — CI has no iTunes library).

use std::path::PathBuf;

fn fixture_path() -> Option<PathBuf> {
    let home = std::env::var_os("HOME")?;
    let path = PathBuf::from(home).join("Music/iTunes/iTunes Library.itl");
    if path.exists() {
        Some(path)
    } else {
        None
    }
}

#[test]
fn loads_real_itl_if_available() {
    let Some(path) = fixture_path() else {
        eprintln!("skipping: ~/Music/iTunes/iTunes Library.itl not present");
        return;
    };
    let lib = itl_rs::ItlFile::open(&path).expect("open .itl");
    assert!(!lib.tracks().is_empty());
    assert!(!lib.playlists().is_empty());

    // At least one track should have a persistent id.
    assert!(lib.tracks().iter().any(|t| t.persistent_id() != 0));
    // At least one playlist should have a title.
    assert!(lib.playlists().iter().any(|p| p.title().is_some()));
}

#[tokio::test]
async fn schema_has_all_sync_columns() {
    let tmp = tempfile::NamedTempFile::new().unwrap();
    let db_path = tmp.path().to_path_buf();
    tuxtunes::smoke_open_db(&db_path).await.unwrap();

    use prax_sqlite::raw::SqliteRawEngine;
    use prax_sqlite::{SqliteConfig, SqlitePool};

    let config = SqliteConfig::file(&db_path);
    let pool = SqlitePool::new(config).await.unwrap();
    let engine = SqliteRawEngine::new(pool);

    let row: String = engine
        .raw_sql_scalar(
            "SELECT sql FROM sqlite_master WHERE type = 'table' AND name = 'tracks'",
            &[],
        )
        .await
        .unwrap();
    assert!(row.contains("import_status"));
    assert!(row.contains("original_path"));

    let row: String = engine
        .raw_sql_scalar(
            "SELECT sql FROM sqlite_master WHERE type = 'table' AND name = 'sync_sources'",
            &[],
        )
        .await
        .unwrap();
    assert!(row.contains("path_mappings"));
    assert!(row.contains("conflict_rules"));
    assert!(row.contains("last_sync_at"));
    assert!(row.contains("last_sync_hash"));
}
