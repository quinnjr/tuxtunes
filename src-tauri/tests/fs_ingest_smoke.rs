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

    // The event is emitted before the row is flagged, so poll rather
    // than reading once.
    let start = std::time::Instant::now();
    loop {
        let row = tuxtunes::db::tracks::get(&db.engine, row_id).await.unwrap();
        if row.import_status == "missing_source" {
            break;
        }
        assert!(
            start.elapsed() < Duration::from_secs(5),
            "still {:?} 5s after fs:ingest-failed",
            row.import_status
        );
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
}

/// Insert a minimal row whose `file_path` is `src`, returning its id.
async fn insert_track(db: &tuxtunes::db::Db, src: &std::path::Path) -> i64 {
    db.engine
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
        .into_json()
        .get("id")
        .and_then(|n| n.as_i64())
        .unwrap()
}

/// Where the default organize scheme puts the row `insert_track`
/// writes: no album_artist falls back to the artist, a single-disc
/// album drops the `{disc:02}-` group, and an absent track number
/// renders as 0.
const SCHEME_REL: &str = "Someone/Album/00 - Song.flac";

/// Run the ingest worker for `track_id` and wait for it to settle,
/// returning the `fs:ingest-complete` payload if one was emitted.
async fn run_ingest(db: &tuxtunes::db::Db, track_id: i64, src: &std::path::Path) -> Option<String> {
    let app: tauri::App<tauri::test::MockRuntime> = tauri::test::mock_app();
    let handle = app.handle().clone();

    let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel::<String>();
    for channel in [
        tuxtunes::fs::events::INGEST_COMPLETE,
        tuxtunes::fs::events::INGEST_FAILED,
    ] {
        let tx = tx.clone();
        let done = channel == tuxtunes::fs::events::INGEST_COMPLETE;
        app.handle().listen(channel, move |event| {
            let _ = tx.send(if done {
                event.payload().to_string()
            } else {
                String::new()
            });
        });
    }

    let fs = tuxtunes::fs::coordinator::FsCoordinator::new(Arc::clone(&db.engine), handle);
    fs.copy_for_track(track_id, src.to_path_buf()).unwrap();

    let payload = tokio::time::timeout(Duration::from_secs(10), rx.recv())
        .await
        .expect("ingest to settle within 10s")
        .expect("channel open");
    (!payload.is_empty()).then_some(payload)
}

#[tokio::test(flavor = "multi_thread")]
async fn ingest_leaves_a_file_already_at_its_scheme_path_in_place() {
    let tmp = tempfile::tempdir().unwrap();
    let lib_root = tmp.path().join("lib");
    std::fs::create_dir_all(lib_root.join("Someone/Album")).unwrap();

    // The file is exactly where the organize scheme would put it, so
    // copying it would only write a `(2)` duplicate beside it.
    let src = lib_root.join(SCHEME_REL);
    std::fs::write(&src, vec![0xABu8; 1024]).unwrap();

    let db = tuxtunes::db::Db::open(&tmp.path().join("tuxtunes.db"))
        .await
        .unwrap();
    tuxtunes::db::preferences::set_library_root(&db.engine, &lib_root)
        .await
        .unwrap();

    let row_id = insert_track(&db, &src).await;
    // Provenance a prior sync recorded must survive an ingest that
    // makes no copy of its own.
    db.engine
        .raw_sql_execute(
            "UPDATE tracks SET original_path = '/mnt/device/Song.flac' WHERE id = ?",
            &[prax_query::filter::FilterValue::Int(row_id)],
        )
        .await
        .unwrap();

    run_ingest(&db, row_id, &src)
        .await
        .expect("ingest-complete");

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
    assert_eq!(
        original.as_deref(),
        Some("/mnt/device/Song.flac"),
        "an ingest that copied nothing must not clear original_path"
    );

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

#[tokio::test(flavor = "multi_thread")]
async fn ingest_organizes_a_file_dropped_elsewhere_under_the_library_root() {
    let tmp = tempfile::tempdir().unwrap();
    let lib_root = tmp.path().join("lib");
    // The "rsync new albums into <root>/_incoming, then Add Folder"
    // shape: inside the root, but not where the scheme says it goes.
    std::fs::create_dir_all(lib_root.join("_incoming")).unwrap();
    let src = lib_root.join("_incoming/whatever.flac");
    std::fs::write(&src, vec![0xABu8; 1024]).unwrap();

    let db = tuxtunes::db::Db::open(&tmp.path().join("tuxtunes.db"))
        .await
        .unwrap();
    tuxtunes::db::preferences::set_library_root(&db.engine, &lib_root)
        .await
        .unwrap();
    let row_id = insert_track(&db, &src).await;

    run_ingest(&db, row_id, &src)
        .await
        .expect("ingest-complete");

    let row = tuxtunes::db::tracks::get(&db.engine, row_id).await.unwrap();
    assert_eq!(
        row.file_path,
        lib_root.join(SCHEME_REL).display().to_string()
    );
    assert!(PathBuf::from(&row.file_path).exists());
}

#[tokio::test(flavor = "multi_thread")]
async fn ingest_does_not_mark_missing_source_when_the_source_still_reads() {
    let tmp = tempfile::tempdir().unwrap();
    // A library root that cannot be written to: the copy fails, but the
    // source the row points at is perfectly playable.
    let lib_root = tmp.path().join("lib");
    std::fs::create_dir_all(&lib_root).unwrap();
    let mut perms = std::fs::metadata(&lib_root).unwrap().permissions();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        perms.set_mode(0o500);
    }
    std::fs::set_permissions(&lib_root, perms).unwrap();

    let src = tmp.path().join("source.flac");
    std::fs::write(&src, vec![0xABu8; 1024]).unwrap();

    let db = tuxtunes::db::Db::open(&tmp.path().join("tuxtunes.db"))
        .await
        .unwrap();
    tuxtunes::db::preferences::set_library_root(&db.engine, &lib_root)
        .await
        .unwrap();
    let row_id = insert_track(&db, &src).await;

    assert!(
        run_ingest(&db, row_id, &src).await.is_none(),
        "a read-only library root should fail the copy"
    );

    let row = tuxtunes::db::tracks::get(&db.engine, row_id).await.unwrap();
    assert_eq!(
        row.import_status, "ok",
        "the source still reads, so the track must stay playable"
    );
    assert_eq!(row.file_path, src.display().to_string());

    // Leave the dir writable so tempfile can clean up.
    let mut perms = std::fs::metadata(&lib_root).unwrap().permissions();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        perms.set_mode(0o700);
    }
    #[cfg(not(unix))]
    #[allow(clippy::permissions_set_readonly_false)]
    perms.set_readonly(false);
    std::fs::set_permissions(&lib_root, perms).unwrap();
}

#[tokio::test(flavor = "multi_thread")]
async fn ingest_makes_a_copy_of_a_read_only_source_writable() {
    let tmp = tempfile::tempdir().unwrap();
    let lib_root = tmp.path().join("lib");
    std::fs::create_dir_all(&lib_root).unwrap();

    // Files extracted from an ISO or copied off an `ro` share come in
    // at 0444; std::fs::copy would carry that onto the managed copy and
    // every later tag edit would fail EACCES.
    let src = tmp.path().join("source.flac");
    std::fs::write(&src, vec![0xABu8; 1024]).unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&src, std::fs::Permissions::from_mode(0o444)).unwrap();
    }

    let db = tuxtunes::db::Db::open(&tmp.path().join("tuxtunes.db"))
        .await
        .unwrap();
    tuxtunes::db::preferences::set_library_root(&db.engine, &lib_root)
        .await
        .unwrap();
    let row_id = insert_track(&db, &src).await;

    run_ingest(&db, row_id, &src)
        .await
        .expect("ingest-complete");

    let row = tuxtunes::db::tracks::get(&db.engine, row_id).await.unwrap();
    let managed = PathBuf::from(&row.file_path);
    assert!(managed.starts_with(&lib_root));
    assert!(
        !std::fs::metadata(&managed)
            .unwrap()
            .permissions()
            .readonly(),
        "the managed copy must stay writable for later tag edits"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn ingest_removes_its_copy_when_the_track_is_deleted_mid_flight() {
    let tmp = tempfile::tempdir().unwrap();
    let lib_root = tmp.path().join("lib");
    std::fs::create_dir_all(&lib_root).unwrap();
    let src = tmp.path().join("source.flac");
    std::fs::write(&src, vec![0xABu8; 1024]).unwrap();

    let db = tuxtunes::db::Db::open(&tmp.path().join("tuxtunes.db"))
        .await
        .unwrap();
    tuxtunes::db::preferences::set_library_root(&db.engine, &lib_root)
        .await
        .unwrap();
    let row_id = insert_track(&db, &src).await;

    // "Remove from Library" while the copy is still queued. The worker
    // must not leave the file it wrote behind, since nothing will ever
    // reference it.
    db.engine
        .raw_sql_execute(
            "DELETE FROM tracks WHERE id = ?",
            &[prax_query::filter::FilterValue::Int(row_id)],
        )
        .await
        .unwrap();

    assert!(
        run_ingest(&db, row_id, &src).await.is_none(),
        "a deleted track cannot complete an ingest"
    );

    assert!(
        !lib_root.join(SCHEME_REL).exists(),
        "orphaned copy left under the library root"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn ingest_avoids_a_target_name_another_row_still_owns() {
    let tmp = tempfile::tempdir().unwrap();
    let lib_root = tmp.path().join("lib");
    std::fs::create_dir_all(&lib_root).unwrap();

    let db = tuxtunes::db::Db::open(&tmp.path().join("tuxtunes.db"))
        .await
        .unwrap();
    tuxtunes::db::preferences::set_library_root(&db.engine, &lib_root)
        .await
        .unwrap();

    // An older row owns the scheme path but its file is gone (deleted
    // outside the app). file_path is UNIQUE, so reusing the name would
    // fail the write *after* the copy landed.
    let stale = lib_root.join(SCHEME_REL);
    insert_track(&db, &stale).await;

    let src = tmp.path().join("source.flac");
    std::fs::write(&src, vec![0xABu8; 1024]).unwrap();
    let row_id = insert_track(&db, &src).await;

    run_ingest(&db, row_id, &src)
        .await
        .expect("ingest-complete");

    let row = tuxtunes::db::tracks::get(&db.engine, row_id).await.unwrap();
    assert_ne!(row.file_path, stale.display().to_string());
    assert!(row.file_path.starts_with(&lib_root.display().to_string()));
    assert!(PathBuf::from(&row.file_path).exists());
}
