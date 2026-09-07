//! End-to-end ingest smoke: insert a Track pointing at a fake source
//! file; drive the FsCoordinator; confirm the DB row's file_path moves
//! under the managed library root AND the target file exists on disk.

use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;
use tauri::Listener;

#[tokio::test(flavor = "multi_thread")]
async fn ingest_copies_hashes_and_updates_row() {
    let tmp = tempfile::tempdir().unwrap();
    let db_path = tmp.path().join("tuxtunes.db");
    let lib_root = tmp.path().join("lib");
    std::fs::create_dir_all(&lib_root).unwrap();

    // 1 KB of deterministic content. NOT a real FLAC; the ingest worker
    // only needs it to exist + hash. Lofty artwork extraction gracefully
    // returns Ok(None) (or an error that the worker swallows) for
    // non-audio content.
    let src = tmp.path().join("source.flac");
    std::fs::write(&src, vec![0xABu8; 1024]).unwrap();

    let db = tuxtunes::db::Db::open(&db_path).await.unwrap();
    tuxtunes::db::preferences::set_library_root(&db.engine, &lib_root)
        .await
        .unwrap();

    // Insert a minimal row. file_path starts pointing at the source
    // path — ingest is what moves it under lib_root.
    let row_id: i64 = {
        let v = db
            .engine
            .raw_sql_first(
                "INSERT INTO tracks (title, artist, album, duration_ms, \
                 size_bytes, file_path, playlist_ids) VALUES \
                 ('Song', 'Someone', 'Album', 100, 1024, ?, '[]') RETURNING id",
                &[prax_query::filter::FilterValue::String(
                    src.display().to_string(),
                )],
            )
            .await
            .unwrap()
            .into_json();
        v.get("id").and_then(|n| n.as_i64()).unwrap()
    };

    // Build a mock Tauri AppHandle. Requires tauri's "test" feature.
    let app: tauri::App<tauri::test::MockRuntime> = tauri::test::mock_app();
    let handle = app.handle().clone();

    let fs = tuxtunes::fs::coordinator::FsCoordinator::new(Arc::clone(&db.engine), handle);
    fs.copy_for_track(row_id, src.clone()).unwrap();

    // Poll until the worker writes the managed path (timeout 10 s).
    let src_str = src.display().to_string();
    let lib_root_str = lib_root.display().to_string();
    let start = std::time::Instant::now();
    loop {
        if start.elapsed() > Duration::from_secs(10) {
            panic!("ingest did not finish within 10s");
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
        let row = tuxtunes::db::tracks::get(&db.engine, row_id).await.unwrap();
        if row.file_path != src_str {
            assert!(
                row.file_path.starts_with(&lib_root_str),
                "expected file_path to be under {lib_root_str}, got {}",
                row.file_path
            );
            assert!(
                PathBuf::from(&row.file_path).exists(),
                "expected managed file at {}",
                row.file_path
            );
            break;
        }
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn ingest_emits_failure_and_marks_missing_source_when_unreadable() {
    let tmp = tempfile::tempdir().unwrap();
    let db_path = tmp.path().join("tuxtunes.db");
    let lib_root = tmp.path().join("lib");
    std::fs::create_dir_all(&lib_root).unwrap();

    // Source path that never existed — hash::hash_file fails, which
    // ingest_one propagates. The dispatcher then emits INGEST_FAILED
    // and marks the row missing_source.
    let missing_src = tmp.path().join("does-not-exist.flac");

    let db = tuxtunes::db::Db::open(&db_path).await.unwrap();
    tuxtunes::db::preferences::set_library_root(&db.engine, &lib_root)
        .await
        .unwrap();

    let row_id: i64 = {
        let v = db
            .engine
            .raw_sql_first(
                "INSERT INTO tracks (title, artist, album, duration_ms, \
                 size_bytes, file_path, playlist_ids) VALUES \
                 ('Song', 'Someone', 'Album', 100, 1024, ?, '[]') RETURNING id",
                &[prax_query::filter::FilterValue::String(
                    missing_src.display().to_string(),
                )],
            )
            .await
            .unwrap()
            .into_json();
        v.get("id").and_then(|n| n.as_i64()).unwrap()
    };

    let app: tauri::App<tauri::test::MockRuntime> = tauri::test::mock_app();
    let handle = app.handle().clone();

    let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel::<String>();
    app.handle()
        .listen(tuxtunes::fs::events::INGEST_FAILED, move |event| {
            let _ = tx.send(event.payload().to_string());
        });

    let fs = tuxtunes::fs::coordinator::FsCoordinator::new(Arc::clone(&db.engine), handle);
    fs.copy_for_track(row_id, missing_src.clone()).unwrap();

    let payload = tokio::time::timeout(Duration::from_secs(5), rx.recv())
        .await
        .expect("fs:ingest-failed within 5s of an unreadable source")
        .expect("channel open");
    assert!(
        payload.contains(&format!("\"track_id\":{row_id}")),
        "{payload}"
    );

    let row = tuxtunes::db::tracks::get(&db.engine, row_id).await.unwrap();
    assert_eq!(row.import_status, "missing_source");
}

#[tokio::test(flavor = "multi_thread")]
async fn ingest_leaves_a_file_already_under_the_library_root_in_place() {
    let tmp = tempfile::tempdir().unwrap();
    let db_path = tmp.path().join("tuxtunes.db");
    let lib_root = tmp.path().join("lib");
    std::fs::create_dir_all(lib_root.join("Someone/Album")).unwrap();

    // The user added a file that already lives inside the managed root.
    // Copying it would leave a duplicate, so ingest must keep the path
    // and only fill in the hash.
    let src = lib_root.join("Someone/Album/01 - Song.flac");
    std::fs::write(&src, vec![0xABu8; 1024]).unwrap();

    let db = tuxtunes::db::Db::open(&db_path).await.unwrap();
    tuxtunes::db::preferences::set_library_root(&db.engine, &lib_root)
        .await
        .unwrap();

    let row_id: i64 = {
        let v = db
            .engine
            .raw_sql_first(
                "INSERT INTO tracks (title, artist, album, duration_ms, \
                 size_bytes, file_path, playlist_ids) VALUES \
                 ('Song', 'Someone', 'Album', 100, 1024, ?, '[]') RETURNING id",
                &[prax_query::filter::FilterValue::String(
                    src.display().to_string(),
                )],
            )
            .await
            .unwrap()
            .into_json();
        v.get("id").and_then(|n| n.as_i64()).unwrap()
    };

    let app: tauri::App<tauri::test::MockRuntime> = tauri::test::mock_app();
    let handle = app.handle().clone();

    let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel::<String>();
    app.handle()
        .listen(tuxtunes::fs::events::INGEST_COMPLETE, move |event| {
            let _ = tx.send(event.payload().to_string());
        });

    let fs = tuxtunes::fs::coordinator::FsCoordinator::new(Arc::clone(&db.engine), handle);
    fs.copy_for_track(row_id, src.clone()).unwrap();

    tokio::time::timeout(Duration::from_secs(10), rx.recv())
        .await
        .expect("fs:ingest-complete within 10s")
        .expect("channel open");

    let row = tuxtunes::db::tracks::get(&db.engine, row_id).await.unwrap();
    assert_eq!(row.file_path, src.display().to_string());
    assert!(row.file_hash.is_some(), "hash should still be recorded");

    let original: Option<String> = db
        .engine
        .raw_sql_first(
            "SELECT original_path FROM tracks WHERE id = ?",
            &[prax_query::filter::FilterValue::Int(row_id)],
        )
        .await
        .unwrap()
        .into_json()
        .get("original_path")
        .and_then(|v| v.as_str())
        .map(str::to_owned);
    assert_eq!(original, None, "no copy was made, so no original");

    // Exactly one file under the album directory — no duplicate copy.
    let entries: Vec<_> = std::fs::read_dir(lib_root.join("Someone/Album"))
        .unwrap()
        .filter_map(Result::ok)
        .filter(|e| e.path().extension().is_some_and(|x| x == "flac"))
        .collect();
    assert_eq!(
        entries.len(),
        1,
        "ingest duplicated an already-managed file"
    );
}
