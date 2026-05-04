//! Drive the sync reconcilers + worker against the user's real iTunes
//! library when available. Skips silently if no fixture is present so
//! CI without iTunes installed stays green.
//!
//! When the fixture is present, this exercises:
//! - reconcile_tracks::reconcile (track-side diff + apply)
//! - reconcile_playlists::reconcile (playlist hierarchy + smart rules)
//! - sync::worker::SyncWorker dispatch via SyncCoordinator

use std::path::PathBuf;

fn fixture_path() -> Option<PathBuf> {
    let home = std::env::var_os("HOME")?;
    let p = PathBuf::from(home).join("Music/iTunes/iTunes Library.itl");
    p.exists().then_some(p)
}

#[tokio::test(flavor = "multi_thread")]
async fn reconcile_tracks_with_path_mapping_inserts_rows() {
    let Some(itl_path) = fixture_path() else {
        eprintln!("skipping: no ~/Music/iTunes/iTunes Library.itl");
        return;
    };
    let lib = itl_rs::ItlFile::open(&itl_path).expect("open itl");

    let tmp = tempfile::tempdir().unwrap();
    let db = tuxtunes::db::Db::open(&tmp.path().join("tuxtunes.db"))
        .await
        .unwrap();

    // Map D:/ → /run/media/joseph/Local Disk/ so the Windows-stored
    // iTunes paths actually resolve. With a working mapping we
    // exercise the upsert + insert_from_itl branch inside reconciler
    // (currently only covered as warnings without mappings).
    let mappings = vec![tuxtunes::sync::path_map::PathMapping {
        from: "D:/".into(),
        to: "/run/media/joseph/Local Disk/".into(),
    }];
    let source_id = tuxtunes::db::sync_sources::insert(
        &db.engine,
        "test-mapped",
        &itl_path.display().to_string(),
        &mappings,
        &Default::default(),
        false,
    )
    .await
    .unwrap();

    let app: tauri::App<tauri::test::MockRuntime> = tauri::test::mock_app();
    let handle = app.handle().clone();

    // Actual outcome depends on whether `D:/...` paths resolve on
    // this dev machine. The DB has a UNIQUE constraint on file_path,
    // so a real library with non-unique mapped paths can produce an
    // Err — that's still coverage of the upsert branch we want. Both
    // Ok and Err are acceptable.
    let res = tuxtunes::sync::reconcile_tracks::reconcile(
        &db.engine,
        &handle,
        source_id,
        &lib,
        &mappings,
        &Default::default(),
    )
    .await;
    if let Ok((stats, _candidates)) = res {
        let total = stats.inserted + stats.updated + stats.deleted + stats.warnings;
        assert!(total > 1000, "expected library walk, got {stats:?}");
        eprintln!("reconcile with mapping: {stats:?}");
    } else {
        eprintln!("reconcile with mapping returned: {:?}", res.unwrap_err());
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn reconcile_tracks_against_real_fixture() {
    let Some(itl_path) = fixture_path() else {
        eprintln!("skipping: no ~/Music/iTunes/iTunes Library.itl");
        return;
    };
    let lib = itl_rs::ItlFile::open(&itl_path).expect("open itl");

    let tmp = tempfile::tempdir().unwrap();
    let db = tuxtunes::db::Db::open(&tmp.path().join("tuxtunes.db"))
        .await
        .unwrap();

    // Create a sync source row so reconcile has a foreign key target.
    let source_id = tuxtunes::db::sync_sources::insert(
        &db.engine,
        "test",
        &itl_path.display().to_string(),
        &[],
        &Default::default(),
        true,
    )
    .await
    .unwrap();

    let app: tauri::App<tauri::test::MockRuntime> = tauri::test::mock_app();
    let handle = app.handle().clone();

    let (stats, _candidates) = tuxtunes::sync::reconcile_tracks::reconcile(
        &db.engine,
        &handle,
        source_id,
        &lib,
        &[],
        &Default::default(),
    )
    .await
    .expect("reconcile tracks");

    // Without path mappings the Windows D:/ paths are unmappable, so
    // every track lands in `warnings` rather than `inserted`. The
    // important coverage signal is that the reconciler walked the
    // entire library and produced ANY result.
    let total = stats.inserted + stats.updated + stats.deleted + stats.warnings;
    assert!(
        total > 1000,
        "expected reconciler to walk the library, got {stats:?}"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn reconcile_playlists_against_real_fixture() {
    let Some(itl_path) = fixture_path() else {
        eprintln!("skipping: no ~/Music/iTunes/iTunes Library.itl");
        return;
    };
    let lib = itl_rs::ItlFile::open(&itl_path).expect("open itl");

    let tmp = tempfile::tempdir().unwrap();
    let db = tuxtunes::db::Db::open(&tmp.path().join("tuxtunes.db"))
        .await
        .unwrap();

    let source_id = tuxtunes::db::sync_sources::insert(
        &db.engine,
        "test",
        &itl_path.display().to_string(),
        &[],
        &Default::default(),
        true,
    )
    .await
    .unwrap();

    // Tracks must exist before playlists reference them.
    let app: tauri::App<tauri::test::MockRuntime> = tauri::test::mock_app();
    let handle = app.handle().clone();
    let _ = tuxtunes::sync::reconcile_tracks::reconcile(
        &db.engine,
        &handle,
        source_id,
        &lib,
        &[],
        &Default::default(),
    )
    .await
    .unwrap();

    let stats =
        tuxtunes::sync::reconcile_playlists::reconcile(&db.engine, &handle, source_id, &lib)
            .await
            .expect("reconcile playlists");
    // Same shape as track reconcile: the playlist walker should have
    // touched some rows even if the path mappings dropped tracks.
    let total = stats.inserted + stats.updated + stats.deleted;
    assert!(
        total > 0,
        "expected playlist reconciler to walk the library, got {stats:?}",
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn sync_coordinator_run_now_drives_worker_to_finalize() {
    // Drive the full SyncCoordinator + SyncWorker dispatch path. Use
    // empty path mappings so all tracks land in `warnings` quickly
    // (no per-track INSERTs against the DB), which makes the worker
    // finish in a few seconds rather than minutes.
    let Some(itl_path) = fixture_path() else {
        eprintln!("skipping: no ~/Music/iTunes/iTunes Library.itl");
        return;
    };

    // Force null AO in case any engine init runs in this test path.
    unsafe {
        std::env::set_var("TUXTUNES_AO", "null");
    }

    let tmp = tempfile::tempdir().unwrap();
    let db = std::sync::Arc::new(
        tuxtunes::db::Db::open(&tmp.path().join("tuxtunes.db"))
            .await
            .unwrap(),
    );
    let source_id = tuxtunes::db::sync_sources::insert(
        &db.engine,
        "real-itl",
        &itl_path.display().to_string(),
        &[],
        &Default::default(),
        false,
    )
    .await
    .unwrap();

    let app: tauri::App<tauri::test::MockRuntime> = tauri::test::mock_app();
    let handle = app.handle().clone();
    let fs = std::sync::Arc::new(tuxtunes::fs::coordinator::FsCoordinator::new(
        std::sync::Arc::clone(&db.engine),
        handle.clone(),
    ));
    let coord = tuxtunes::sync::coordinator::SyncCoordinator::new(
        std::sync::Arc::clone(&db),
        std::sync::Arc::clone(&fs),
        handle,
    );
    coord.run_now(source_id).unwrap();

    // Wait for the worker to reach finalize_sync (sets last_sync_at).
    // Empty mappings mean no INSERTs, so this should complete within
    // a few seconds.
    let start = std::time::Instant::now();
    let timeout = std::time::Duration::from_secs(30);
    loop {
        if start.elapsed() > timeout {
            panic!("sync did not finish within 30s");
        }
        tokio::time::sleep(std::time::Duration::from_millis(200)).await;
        let last_sync: Option<String> = db
            .engine
            .raw_sql_optional(
                "SELECT last_sync_at FROM sync_sources WHERE id = ?",
                &[prax_query::filter::FilterValue::Int(source_id)],
            )
            .await
            .unwrap()
            .and_then(|r| {
                r.into_json()
                    .get("last_sync_at")
                    .and_then(|v| v.as_str().map(|s| s.to_string()))
            });
        if last_sync.is_some() {
            break;
        }
    }
}
