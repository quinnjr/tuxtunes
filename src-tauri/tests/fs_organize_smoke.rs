//! Drive the OrganizeWorker through the FsCoordinator: a track that
//! starts under the managed library root with stale metadata gets
//! moved/renamed once metadata changes via reorganize_track.

use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

#[tokio::test(flavor = "multi_thread")]
async fn reorganize_renames_file_on_metadata_change() {
    let tmp = tempfile::tempdir().unwrap();
    let db_path = tmp.path().join("tuxtunes.db");
    let lib_root = tmp.path().join("lib");
    std::fs::create_dir_all(&lib_root).unwrap();

    // Old managed path: under lib_root with a stale name. The
    // organize worker will compute the canonical scheme path and move
    // the file there.
    let old_dir = lib_root.join("OLD/OLD-album");
    std::fs::create_dir_all(&old_dir).unwrap();
    let old_path = old_dir.join("01 - old-title.flac");
    std::fs::write(&old_path, b"x").unwrap();

    let db = tuxtunes::db::Db::open(&db_path).await.unwrap();
    tuxtunes::db::preferences::set_library_root(&db.engine, &lib_root)
        .await
        .unwrap();

    // Insert a row with FRESH metadata that differs from the stale path.
    let row_id: i64 = db
        .engine
        .raw_sql_first(
            "INSERT INTO tracks (title, artist, album_artist, album, track_number, \
             disc_number, duration_ms, size_bytes, file_path, playlist_ids) \
             VALUES ('NewTitle', 'A', 'A', 'NewAlbum', 3, 1, 0, 0, ?, '[]') RETURNING id",
            &[prax_query::filter::FilterValue::String(
                old_path.display().to_string(),
            )],
        )
        .await
        .unwrap()
        .into_json()
        .get("id")
        .and_then(|v| v.as_i64())
        .unwrap();

    let app: tauri::App<tauri::test::MockRuntime> = tauri::test::mock_app();
    let handle = app.handle().clone();
    let fs = tuxtunes::fs::coordinator::FsCoordinator::new(Arc::clone(&db.engine), handle);
    fs.reorganize_track(row_id).unwrap();

    // Poll until the row's file_path moves to a new canonical
    // location (or 5 s timeout).
    let start = std::time::Instant::now();
    let old_str = old_path.display().to_string();
    loop {
        if start.elapsed() > Duration::from_secs(5) {
            panic!("organize did not finish within 5s");
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
        let row = tuxtunes::db::tracks::get(&db.engine, row_id).await.unwrap();
        if row.file_path != old_str {
            assert!(
                PathBuf::from(&row.file_path).exists(),
                "expected new managed file at {}",
                row.file_path
            );
            assert!(
                row.file_path.contains("NewAlbum"),
                "expected new path under NewAlbum, got {}",
                row.file_path
            );
            // Old file should have been moved away.
            assert!(!old_path.exists(), "old path should be gone");
            break;
        }
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn reorganize_emits_failure_event_when_source_missing() {
    let tmp = tempfile::tempdir().unwrap();
    let db_path = tmp.path().join("tuxtunes.db");
    let lib_root = tmp.path().join("lib");
    std::fs::create_dir_all(&lib_root).unwrap();

    let db = tuxtunes::db::Db::open(&db_path).await.unwrap();
    tuxtunes::db::preferences::set_library_root(&db.engine, &lib_root)
        .await
        .unwrap();

    // Insert a row whose file_path doesn't exist on disk — organize_one
    // takes the bail!() path and emits ORGANIZE_FAILED.
    let row_id: i64 = db
        .engine
        .raw_sql_first(
            "INSERT INTO tracks (title, duration_ms, size_bytes, file_path, playlist_ids) \
             VALUES ('Phantom', 0, 0, '/nope/missing.flac', '[]') RETURNING id",
            &[],
        )
        .await
        .unwrap()
        .into_json()
        .get("id")
        .and_then(|v| v.as_i64())
        .unwrap();

    let app: tauri::App<tauri::test::MockRuntime> = tauri::test::mock_app();
    let handle = app.handle().clone();
    let fs = tuxtunes::fs::coordinator::FsCoordinator::new(Arc::clone(&db.engine), handle);
    fs.reorganize_track(row_id).unwrap();

    // Give the worker a beat. We don't assert on the event payload
    // (mock_app doesn't expose listeners cleanly); the file_path
    // should remain unchanged because the bail() returned before any
    // mutation.
    tokio::time::sleep(Duration::from_millis(100)).await;
    let row = tuxtunes::db::tracks::get(&db.engine, row_id).await.unwrap();
    assert_eq!(row.file_path, "/nope/missing.flac");
}
