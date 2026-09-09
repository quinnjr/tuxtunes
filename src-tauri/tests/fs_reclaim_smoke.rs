//! Reclaiming the originals that copy-on-add left behind.

use prax_query::filter::FilterValue as FV;
use std::sync::Arc;
use std::time::Duration;
use tauri::Listener;

/// Insert a row that records having been copied from `original`.
async fn insert_copied(
    db: &tuxtunes::db::Db,
    title: &str,
    managed: &std::path::Path,
    original: &std::path::Path,
) -> i64 {
    db.engine
        .raw_sql_first(
            "INSERT INTO tracks (title, duration_ms, size_bytes, file_path, \
             original_path, playlist_ids) VALUES (?, 100, 1024, ?, ?, '[]') RETURNING id",
            &[
                FV::String(title.to_string()),
                FV::String(managed.display().to_string()),
                FV::String(original.display().to_string()),
            ],
        )
        .await
        .unwrap()
        .into_json()
        .get("id")
        .and_then(|v| v.as_i64())
        .unwrap()
}

async fn original_path(db: &tuxtunes::db::Db, id: i64) -> Option<String> {
    db.engine
        .raw_sql_first(
            "SELECT original_path FROM tracks WHERE id = ?",
            &[FV::Int(id)],
        )
        .await
        .unwrap()
        .into_json()
        .get("original_path")
        .and_then(|v| v.as_str())
        .map(str::to_owned)
}

/// Drive the reclaim pass and return the completion payload.
async fn run_reclaim(db: &tuxtunes::db::Db) -> String {
    let app: tauri::App<tauri::test::MockRuntime> = tauri::test::mock_app();
    let handle = app.handle().clone();
    let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel::<String>();
    app.handle()
        .listen(tuxtunes::fs::events::RECLAIM_COMPLETE, move |event| {
            let _ = tx.send(event.payload().to_string());
        });

    let fs = tuxtunes::fs::coordinator::FsCoordinator::new(Arc::clone(&db.engine), handle);
    fs.reclaim_originals().unwrap();

    tokio::time::timeout(Duration::from_secs(15), rx.recv())
        .await
        .expect("fs:reclaim-complete within 15s")
        .expect("channel open")
}

#[tokio::test(flavor = "multi_thread")]
async fn reclaim_trashes_only_originals_whose_copy_matches() {
    let tmp = tempfile::tempdir().unwrap();
    let lib_root = tmp.path().join("lib");
    std::fs::create_dir_all(&lib_root).unwrap();
    let src_dir = tmp.path().join("source");
    std::fs::create_dir_all(&src_dir).unwrap();

    let db = tuxtunes::db::Db::open(&tmp.path().join("t.db"))
        .await
        .unwrap();
    tuxtunes::db::preferences::set_library_root(&db.engine, &lib_root)
        .await
        .unwrap();

    // 1. A faithful copy: the original is redundant.
    let good_src = src_dir.join("good.flac");
    let good_copy = lib_root.join("good.flac");
    std::fs::write(&good_src, vec![0xABu8; 4096]).unwrap();
    std::fs::copy(&good_src, &good_copy).unwrap();
    let good_id = insert_copied(&db, "Good", &good_copy, &good_src).await;

    // 2. Same size, different bytes — the copy has diverged (tags
    //    written back to it, say). Not ours to choose between them.
    let diverged_src = src_dir.join("diverged.flac");
    let diverged_copy = lib_root.join("diverged.flac");
    std::fs::write(&diverged_src, vec![0x11u8; 4096]).unwrap();
    std::fs::write(&diverged_copy, vec![0x22u8; 4096]).unwrap();
    let diverged_id = insert_copied(&db, "Diverged", &diverged_copy, &diverged_src).await;

    // 3. The copy never landed: the original is all there is.
    let nocopy_src = src_dir.join("nocopy.flac");
    std::fs::write(&nocopy_src, vec![0x33u8; 4096]).unwrap();
    let nocopy_id = insert_copied(&db, "No Copy", &lib_root.join("nocopy.flac"), &nocopy_src).await;

    let payload = run_reclaim(&db).await;
    let summary: serde_json::Value = serde_json::from_str(&payload).unwrap();
    assert_eq!(summary["reclaimed"], 1, "{payload}");
    assert_eq!(summary["bytes_freed"], 4096, "{payload}");
    assert_eq!(summary["skipped"], 2, "{payload}");
    assert_eq!(summary["failed"], 0, "{payload}");

    assert!(!good_src.exists(), "the redundant original was kept");
    assert!(good_copy.is_file(), "the managed copy must survive");
    assert!(diverged_src.is_file(), "a diverged original was trashed");
    assert!(nocopy_src.is_file(), "the only copy of a file was trashed");

    // The reclaimed row stops offering itself; the others still can.
    assert_eq!(original_path(&db, good_id).await, None);
    assert!(original_path(&db, diverged_id).await.is_some());
    assert!(original_path(&db, nocopy_id).await.is_some());
}

#[tokio::test(flavor = "multi_thread")]
async fn reclaim_forgets_an_original_that_is_already_gone() {
    let tmp = tempfile::tempdir().unwrap();
    let lib_root = tmp.path().join("lib");
    std::fs::create_dir_all(&lib_root).unwrap();

    let db = tuxtunes::db::Db::open(&tmp.path().join("t.db"))
        .await
        .unwrap();
    tuxtunes::db::preferences::set_library_root(&db.engine, &lib_root)
        .await
        .unwrap();

    let copy = lib_root.join("song.flac");
    std::fs::write(&copy, vec![0xABu8; 1024]).unwrap();
    let id = insert_copied(&db, "Song", &copy, &tmp.path().join("gone.flac")).await;

    let payload = run_reclaim(&db).await;
    let summary: serde_json::Value = serde_json::from_str(&payload).unwrap();
    assert_eq!(summary["reclaimed"], 0, "{payload}");
    // Not a skip and not a failure: there is simply nothing there.
    assert_eq!(summary["skipped"], 0, "{payload}");
    assert_eq!(summary["failed"], 0, "{payload}");
    assert_eq!(
        original_path(&db, id).await,
        None,
        "a vanished original should stop being offered"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn pending_counts_what_reclaim_would_free() {
    let tmp = tempfile::tempdir().unwrap();
    let lib_root = tmp.path().join("lib");
    std::fs::create_dir_all(&lib_root).unwrap();
    let db = tuxtunes::db::Db::open(&tmp.path().join("t.db"))
        .await
        .unwrap();

    let src = tmp.path().join("a.flac");
    std::fs::write(&src, vec![0xABu8; 2048]).unwrap();
    insert_copied(&db, "A", &lib_root.join("a.flac"), &src).await;
    // An original that is already gone contributes nothing.
    insert_copied(
        &db,
        "B",
        &lib_root.join("b.flac"),
        &tmp.path().join("gone.flac"),
    )
    .await;

    let (files, bytes) = tuxtunes::fs::reclaim::pending(&db.engine).await.unwrap();
    assert_eq!(files, 1);
    assert_eq!(bytes, 2048);
}
