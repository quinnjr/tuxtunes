//! Drive the verify_all walk over a small library: one healthy track
//! whose hash matches, one missing-source track whose path is gone,
//! one mismatch whose hash drifts.

use std::sync::Arc;

#[tokio::test(flavor = "multi_thread")]
async fn verify_walks_library_and_classifies_each_row() {
    let tmp = tempfile::tempdir().unwrap();
    let db_path = tmp.path().join("tuxtunes.db");
    let lib_root = tmp.path().join("lib");
    std::fs::create_dir_all(&lib_root).unwrap();

    let db = Arc::new(tuxtunes::db::Db::open(&db_path).await.unwrap());
    tuxtunes::db::preferences::set_library_root(&db.engine, &lib_root)
        .await
        .unwrap();

    // Healthy: file exists, hash matches the stored value.
    let healthy = lib_root.join("healthy.flac");
    std::fs::write(&healthy, b"healthy bytes").unwrap();
    let healthy_hash =
        tuxtunes::fs::hash::hash_hex(tuxtunes::fs::hash::hash_file(&healthy).unwrap());

    // Missing: file_path points to a non-existent path.
    // Mismatch: file exists but stored hash doesn't match.
    let mismatch = lib_root.join("mismatch.flac");
    std::fs::write(&mismatch, b"current content").unwrap();

    let _healthy_id: i64 = db
        .engine
        .raw_sql_first(
            "INSERT INTO tracks (title, duration_ms, size_bytes, file_path, file_hash, \
             playlist_ids) VALUES ('h', 0, 0, ?, ?, '[]') RETURNING id",
            &[
                prax_query::filter::FilterValue::String(healthy.display().to_string()),
                prax_query::filter::FilterValue::String(healthy_hash.clone()),
            ],
        )
        .await
        .unwrap()
        .into_json()
        .get("id")
        .and_then(|v| v.as_i64())
        .unwrap();

    db.engine
        .raw_sql_execute(
            "INSERT INTO tracks (title, duration_ms, size_bytes, file_path, file_hash, \
             playlist_ids) VALUES \
             ('missing', 0, 0, '/no/such/path.flac', 'cafe', '[]'), \
             ('mismatch', 0, 0, ?, 'beef', '[]')",
            &[prax_query::filter::FilterValue::String(
                mismatch.display().to_string(),
            )],
        )
        .await
        .unwrap();

    let app: tauri::App<tauri::test::MockRuntime> = tauri::test::mock_app();
    let handle = app.handle().clone();
    let stats = tuxtunes::fs::verify::verify_all(&db.engine, &handle).await.unwrap();

    assert_eq!(stats.total, 3);
    assert_eq!(stats.verified, 1);
    assert_eq!(stats.missing, 1);
    assert_eq!(stats.mismatched, 1);
}

#[tokio::test(flavor = "multi_thread")]
async fn verify_on_empty_library_completes() {
    let tmp = tempfile::tempdir().unwrap();
    let db_path = tmp.path().join("tuxtunes.db");
    let db = tuxtunes::db::Db::open(&db_path).await.unwrap();

    let app: tauri::App<tauri::test::MockRuntime> = tauri::test::mock_app();
    let handle = app.handle().clone();
    let stats = tuxtunes::fs::verify::verify_all(&db.engine, &handle).await.unwrap();
    assert_eq!(stats.total, 0);
}
