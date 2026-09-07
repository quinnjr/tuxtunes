//! End-to-end "Reorganize library" pass: a track already at its scheme
//! path is left alone, one elsewhere under the root is moved, and one
//! outside the root is copied in.

use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;
use tauri::Listener;

/// Insert a row with the given title and file path; returns its id.
async fn insert_track(db: &tuxtunes::db::Db, title: &str, src: &std::path::Path) -> i64 {
    use prax_query::filter::FilterValue as FV;
    db.engine
        .raw_sql_first(
            "INSERT INTO tracks (title, artist, album, duration_ms, \
             size_bytes, file_path, playlist_ids) VALUES \
             (?, 'Someone', 'Album', 100, 1024, ?, '[]') RETURNING id",
            &[
                FV::String(title.to_string()),
                FV::String(src.display().to_string()),
            ],
        )
        .await
        .unwrap()
        .into_json()
        .get("id")
        .and_then(|n| n.as_i64())
        .unwrap()
}

/// Where the default scheme puts a row from `insert_track`: album
/// artist falls back to the artist, a single-disc album drops the
/// `{disc:02}-` group, and an absent track number renders as 0.
fn scheme_path(root: &std::path::Path, title: &str) -> PathBuf {
    root.join(format!("Someone/Album/00 - {title}.flac"))
}

#[tokio::test(flavor = "multi_thread")]
async fn consolidate_moves_copies_and_leaves_files_as_appropriate() {
    let tmp = tempfile::tempdir().unwrap();
    let lib_root = tmp.path().join("lib");
    std::fs::create_dir_all(lib_root.join("Someone/Album")).unwrap();
    std::fs::create_dir_all(lib_root.join("_incoming")).unwrap();

    let db = tuxtunes::db::Db::open(&tmp.path().join("tuxtunes.db"))
        .await
        .unwrap();
    tuxtunes::db::preferences::set_library_root(&db.engine, &lib_root)
        .await
        .unwrap();

    // 1. Already exactly where the scheme wants it.
    let settled = scheme_path(&lib_root, "Settled");
    std::fs::write(&settled, vec![0xABu8; 1024]).unwrap();
    let settled_id = insert_track(&db, "Settled", &settled).await;

    // 2. Inside the root but in a drop folder — should be moved.
    let dropped = lib_root.join("_incoming/whatever.flac");
    std::fs::write(&dropped, vec![0xCDu8; 1024]).unwrap();
    let dropped_id = insert_track(&db, "Dropped", &dropped).await;

    // 3. Outside the root — should be copied in, original untouched.
    let outside = tmp.path().join("elsewhere/outside.flac");
    std::fs::create_dir_all(outside.parent().unwrap()).unwrap();
    std::fs::write(&outside, vec![0xEFu8; 1024]).unwrap();
    let outside_id = insert_track(&db, "Outside", &outside).await;

    let app: tauri::App<tauri::test::MockRuntime> = tauri::test::mock_app();
    let handle = app.handle().clone();
    let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel::<String>();
    app.handle()
        .listen(tuxtunes::fs::events::CONSOLIDATE_COMPLETE, move |event| {
            let _ = tx.send(event.payload().to_string());
        });

    let fs = tuxtunes::fs::coordinator::FsCoordinator::new(Arc::clone(&db.engine), handle);
    fs.consolidate_library().unwrap();

    let payload = tokio::time::timeout(Duration::from_secs(15), rx.recv())
        .await
        .expect("fs:consolidate-complete within 15s")
        .expect("channel open");
    let summary: serde_json::Value = serde_json::from_str(&payload).unwrap();
    assert_eq!(summary["total"], 3, "{payload}");
    assert_eq!(summary["in_place"], 1, "{payload}");
    assert_eq!(summary["moved"], 1, "{payload}");
    assert_eq!(summary["copied"], 1, "{payload}");
    assert_eq!(summary["failed"], 0, "{payload}");

    let row = tuxtunes::db::tracks::get(&db.engine, settled_id)
        .await
        .unwrap();
    assert_eq!(row.file_path, settled.display().to_string());

    let row = tuxtunes::db::tracks::get(&db.engine, dropped_id)
        .await
        .unwrap();
    assert_eq!(
        row.file_path,
        scheme_path(&lib_root, "Dropped").display().to_string()
    );
    assert!(
        !dropped.exists(),
        "a file inside the root is moved, not copied"
    );

    let row = tuxtunes::db::tracks::get(&db.engine, outside_id)
        .await
        .unwrap();
    assert_eq!(
        row.file_path,
        scheme_path(&lib_root, "Outside").display().to_string()
    );
    assert!(PathBuf::from(&row.file_path).exists());
    assert!(
        outside.exists(),
        "a file outside the root keeps its original"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn consolidate_counts_a_missing_file_as_failed_and_carries_on() {
    let tmp = tempfile::tempdir().unwrap();
    let lib_root = tmp.path().join("lib");
    std::fs::create_dir_all(&lib_root).unwrap();

    let db = tuxtunes::db::Db::open(&tmp.path().join("tuxtunes.db"))
        .await
        .unwrap();
    tuxtunes::db::preferences::set_library_root(&db.engine, &lib_root)
        .await
        .unwrap();

    insert_track(&db, "Gone", &tmp.path().join("gone.flac")).await;
    let real = tmp.path().join("real.flac");
    std::fs::write(&real, vec![0xABu8; 1024]).unwrap();
    let real_id = insert_track(&db, "Real", &real).await;

    let app: tauri::App<tauri::test::MockRuntime> = tauri::test::mock_app();
    let handle = app.handle().clone();
    let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel::<String>();
    app.handle()
        .listen(tuxtunes::fs::events::CONSOLIDATE_COMPLETE, move |event| {
            let _ = tx.send(event.payload().to_string());
        });

    let fs = tuxtunes::fs::coordinator::FsCoordinator::new(Arc::clone(&db.engine), handle);
    fs.consolidate_library().unwrap();

    let payload = tokio::time::timeout(Duration::from_secs(15), rx.recv())
        .await
        .expect("fs:consolidate-complete within 15s")
        .expect("channel open");
    let summary: serde_json::Value = serde_json::from_str(&payload).unwrap();
    assert_eq!(summary["failed"], 1, "{payload}");
    assert_eq!(summary["copied"], 1, "{payload}");

    let row = tuxtunes::db::tracks::get(&db.engine, real_id)
        .await
        .unwrap();
    assert_eq!(
        row.file_path,
        lib_root
            .join("Someone/Album/00 - Real.flac")
            .display()
            .to_string(),
        "one missing file must not abandon the rest of the walk"
    );
}
