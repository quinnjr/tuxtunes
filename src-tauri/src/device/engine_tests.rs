//! Engine tests.
//!
//! Most cases run against [`FakeTransport`], whose fault injection
//! covers the cable-pull, out-of-space, collision and cancellation
//! paths with no hardware and no platform dependency. One case runs
//! against [`FsTransport`](crate::device::transport::fs::FsTransport)
//! as well, because the fake buffers writes while a real transport
//! materialises the destination on open — only the latter can prove
//! the `.tuxpart` cleanup actually holds.

use super::engine::{build_plan, resolve_selection, run, EngineError};
use super::events::{DevicePhase, DeviceWarningKind};
use super::observer::RecordingObserver;
use super::transport::fake::{FakeTransport, Fault};
use super::transport::{DevicePath, DeviceTransport};
use crate::db::device_objects;
use crate::db::devices::{self, DeviceRow, SelectionEntry};
use crate::db::{playlists as db_playlists, Db};
use prax_query::filter::FilterValue;
use std::sync::atomic::AtomicBool;

/// A library with three tracks (real files on disk) in one album, plus
/// a regular playlist holding all three, and a filesystem device whose
/// selection is that playlist.
struct Fixture {
    _db_file: tempfile::NamedTempFile,
    _lib: tempfile::TempDir,
    db: Db,
    device_id: i64,
    playlist_id: i64,
    track_ids: Vec<i64>,
    track_paths: Vec<std::path::PathBuf>,
}

impl Fixture {
    async fn device(&self) -> DeviceRow {
        devices::get(&self.db.engine, self.device_id).await.unwrap()
    }

    async fn manifest_len(&self) -> usize {
        device_objects::list_for_device(&self.db.engine, self.device_id)
            .await
            .unwrap()
            .len()
    }
}

const TITLES: [&str; 3] = ["Kerala", "Outlier", "Break Apart"];

async fn fixture() -> Fixture {
    let db_file = tempfile::NamedTempFile::new().unwrap();
    let db = Db::open(db_file.path()).await.unwrap();
    let lib = tempfile::tempdir().unwrap();

    let mut track_ids = Vec::new();
    let mut track_paths = Vec::new();
    for (i, title) in TITLES.iter().enumerate() {
        let path = lib.path().join(format!("{i}.flac"));
        // Distinct lengths make "which file landed where" assertable.
        std::fs::write(&path, vec![b'a' + i as u8; 100 + i]).unwrap();
        let id = insert_track(&db, title, i as i64 + 1, path.to_str().unwrap()).await;
        track_ids.push(id);
        track_paths.push(path);
    }

    let playlist_id = db_playlists::create_regular(&db.engine, "Favourites", None)
        .await
        .unwrap();
    db_playlists::add_tracks(&db.engine, playlist_id, &track_ids)
        .await
        .unwrap();

    let device_id =
        devices::upsert_by_key(&db.engine, "fs:/mnt/dap", "DAP", "filesystem", None, false)
            .await
            .unwrap();
    devices::update_selection(
        &db.engine,
        device_id,
        &[SelectionEntry::Playlist { id: playlist_id }],
    )
    .await
    .unwrap();

    Fixture {
        _db_file: db_file,
        _lib: lib,
        db,
        device_id,
        playlist_id,
        track_ids,
        track_paths,
    }
}

async fn insert_track(db: &Db, title: &str, track_number: i64, file_path: &str) -> i64 {
    let sql = "INSERT INTO tracks \
               (title, artist, album_artist, album, track_number, disc_number, duration_ms, \
                size_bytes, file_path, file_hash, kind, playlist_ids) \
               VALUES (?, 'Bonobo', 'Bonobo', 'Migration', ?, 1, 243000, 100, ?, ?, 'flac', '[]') \
               RETURNING id";
    let params = vec![
        FilterValue::String(title.to_string()),
        FilterValue::Int(track_number),
        FilterValue::String(file_path.to_string()),
        FilterValue::String(format!("hash-{title}")),
    ];
    db.engine
        .raw_sql_first(sql, &params)
        .await
        .unwrap()
        .into_json()
        .get("id")
        .and_then(|v| v.as_i64())
        .unwrap()
}

async fn set_flag(db: &Db, device_id: i64, column: &str, value: i64) {
    db.engine
        .raw_sql_execute(
            &format!("UPDATE devices SET {column} = ? WHERE id = ?"),
            &[FilterValue::Int(value), FilterValue::Int(device_id)],
        )
        .await
        .unwrap();
}

fn no_cancel() -> AtomicBool {
    AtomicBool::new(false)
}

// ---------------------------------------------------------------- //
// Selection resolution
// ---------------------------------------------------------------- //

#[tokio::test]
async fn resolve_selection_deduplicates_across_entries() {
    let f = fixture().await;
    let sel = vec![
        SelectionEntry::Playlist { id: f.playlist_id },
        SelectionEntry::Album {
            album_artist: "Bonobo".into(),
            album: "Migration".into(),
        },
    ];
    let rows = resolve_selection(&f.db.engine, &sel).await.unwrap();
    assert_eq!(
        rows.len(),
        3,
        "a track named twice is pushed once, not twice"
    );
}

#[tokio::test]
async fn resolve_selection_of_nothing_is_empty() {
    let f = fixture().await;
    assert!(resolve_selection(&f.db.engine, &[])
        .await
        .unwrap()
        .is_empty());
}

// ---------------------------------------------------------------- //
// Happy path
// ---------------------------------------------------------------- //

#[tokio::test]
async fn run_uploads_selected_tracks_and_records_the_manifest() {
    let f = fixture().await;
    let t = FakeTransport::new();
    let obs = RecordingObserver::default();

    let done = run(&f.db.engine, &t, &obs, &f.device().await, &no_cancel())
        .await
        .unwrap();

    assert_eq!(done.added, 3);
    assert_eq!(done.skipped, 0);
    assert_eq!(done.playlists_written, 1);
    assert_eq!(f.manifest_len().await, 4, "3 tracks + 1 playlist");

    let files = t.files();
    assert!(
        files
            .iter()
            .any(|(p, _)| p == "/Music/Bonobo/Migration/01 Kerala.flac"),
        "unexpected layout: {:?}",
        files.iter().map(|(p, _)| p).collect::<Vec<_>>()
    );
    assert!(files.iter().all(|(p, _)| !p.ends_with(".tuxpart")));
}

#[tokio::test]
async fn uploaded_bytes_match_the_source_exactly() {
    let f = fixture().await;
    let t = FakeTransport::new();
    run(
        &f.db.engine,
        &t,
        &RecordingObserver::default(),
        &f.device().await,
        &no_cancel(),
    )
    .await
    .unwrap();

    let expected = std::fs::read(&f.track_paths[0]).unwrap();
    let (_, got) = t
        .files()
        .into_iter()
        .find(|(p, _)| p.ends_with("01 Kerala.flac"))
        .expect("Kerala should be on the device");
    assert_eq!(got, expected, "a supported codec must be copied bit-exact");
}

#[tokio::test]
async fn phases_are_reported_in_order() {
    let f = fixture().await;
    let obs = RecordingObserver::default();
    run(
        &f.db.engine,
        &FakeTransport::new(),
        &obs,
        &f.device().await,
        &no_cancel(),
    )
    .await
    .unwrap();
    let phases = obs.phases();
    assert_eq!(phases.first(), Some(&DevicePhase::Enumerating));
    assert_eq!(phases.last(), Some(&DevicePhase::Finalizing));
    assert!(phases.contains(&DevicePhase::Uploading));
    assert!(phases.contains(&DevicePhase::Playlists));
}

#[tokio::test]
async fn a_second_run_with_no_changes_uploads_nothing() {
    let f = fixture().await;
    let t = FakeTransport::new();
    run(
        &f.db.engine,
        &t,
        &RecordingObserver::default(),
        &f.device().await,
        &no_cancel(),
    )
    .await
    .unwrap();

    let done = run(
        &f.db.engine,
        &t,
        &RecordingObserver::default(),
        &f.device().await,
        &no_cancel(),
    )
    .await
    .unwrap();

    assert_eq!(done.added, 0);
    assert_eq!(done.replaced, 0);
    assert_eq!(done.unchanged, 3);
    assert_eq!(done.bytes_written, 0);
    assert_eq!(f.manifest_len().await, 4, "no duplicate manifest rows");
}

#[tokio::test]
async fn an_edited_file_is_replaced_not_duplicated() {
    let f = fixture().await;
    let t = FakeTransport::new();
    run(
        &f.db.engine,
        &t,
        &RecordingObserver::default(),
        &f.device().await,
        &no_cancel(),
    )
    .await
    .unwrap();

    f.db.engine
        .raw_sql_execute(
            "UPDATE tracks SET file_hash = 'changed' WHERE id = ?",
            &[FilterValue::Int(f.track_ids[0])],
        )
        .await
        .unwrap();

    let done = run(
        &f.db.engine,
        &t,
        &RecordingObserver::default(),
        &f.device().await,
        &no_cancel(),
    )
    .await
    .unwrap();

    assert_eq!(done.replaced, 1);
    assert_eq!(done.unchanged, 2);
    assert_eq!(f.manifest_len().await, 4);
}

// ---------------------------------------------------------------- //
// Pruning safety
// ---------------------------------------------------------------- //

#[tokio::test]
async fn deselecting_everything_prunes_only_what_we_wrote() {
    let f = fixture().await;
    let t = FakeTransport::new();
    run(
        &f.db.engine,
        &t,
        &RecordingObserver::default(),
        &f.device().await,
        &no_cancel(),
    )
    .await
    .unwrap();

    // A file the user put there by hand.
    let theirs = DevicePath::new("/Music/Manual/theirs.mp3");
    t.mkdir_all(&DevicePath::new("/Music/Manual")).unwrap();
    {
        let mut w = t.open_write(&theirs, 1).unwrap();
        std::io::Write::write_all(&mut w, b"x").unwrap();
        std::io::Write::flush(&mut w).unwrap();
    }

    devices::update_selection(&f.db.engine, f.device_id, &[])
        .await
        .unwrap();
    let done = run(
        &f.db.engine,
        &t,
        &RecordingObserver::default(),
        &f.device().await,
        &no_cancel(),
    )
    .await
    .unwrap();

    assert_eq!(done.deleted, 3, "the three tracks we pushed");
    assert_eq!(done.playlists_written, 0);
    assert_eq!(f.manifest_len().await, 0, "playlist row pruned too");
    assert!(
        t.stat(&theirs).unwrap().is_some(),
        "a file TuxTunes never wrote must survive a mirroring sync"
    );
}

#[tokio::test]
async fn pruning_removes_the_directories_it_emptied() {
    let f = fixture().await;
    let t = FakeTransport::new();
    run(
        &f.db.engine,
        &t,
        &RecordingObserver::default(),
        &f.device().await,
        &no_cancel(),
    )
    .await
    .unwrap();
    devices::update_selection(&f.db.engine, f.device_id, &[])
        .await
        .unwrap();
    run(
        &f.db.engine,
        &t,
        &RecordingObserver::default(),
        &f.device().await,
        &no_cancel(),
    )
    .await
    .unwrap();

    assert_eq!(
        t.stat(&DevicePath::new("/Music/Bonobo")).unwrap(),
        None,
        "emptied album directories should not linger"
    );
    assert!(
        t.stat(&DevicePath::new("/Music")).unwrap().is_some(),
        "the device root is never removed"
    );
}

#[tokio::test]
async fn mirror_deletes_off_leaves_orphans_in_place() {
    let f = fixture().await;
    let t = FakeTransport::new();
    run(
        &f.db.engine,
        &t,
        &RecordingObserver::default(),
        &f.device().await,
        &no_cancel(),
    )
    .await
    .unwrap();

    set_flag(&f.db, f.device_id, "mirror_deletes", 0).await;
    devices::update_selection(&f.db.engine, f.device_id, &[])
        .await
        .unwrap();
    let done = run(
        &f.db.engine,
        &t,
        &RecordingObserver::default(),
        &f.device().await,
        &no_cancel(),
    )
    .await
    .unwrap();

    assert_eq!(done.deleted, 0);
    assert!(t
        .stat(&DevicePath::new("/Music/Bonobo/Migration/01 Kerala.flac"))
        .unwrap()
        .is_some());
}

#[tokio::test]
async fn a_weak_device_key_suppresses_pruning() {
    let f = fixture().await;
    let t = FakeTransport::new();
    run(
        &f.db.engine,
        &t,
        &RecordingObserver::default(),
        &f.device().await,
        &no_cancel(),
    )
    .await
    .unwrap();

    set_flag(&f.db, f.device_id, "key_is_weak", 1).await;
    devices::update_selection(&f.db.engine, f.device_id, &[])
        .await
        .unwrap();
    let done = run(
        &f.db.engine,
        &t,
        &RecordingObserver::default(),
        &f.device().await,
        &no_cancel(),
    )
    .await
    .unwrap();

    assert_eq!(
        done.deleted, 0,
        "a key that could match the wrong hardware must never delete"
    );
}

// ---------------------------------------------------------------- //
// Failure paths
// ---------------------------------------------------------------- //

#[tokio::test]
async fn an_upload_failure_warns_and_leaves_no_manifest_row() {
    let f = fixture().await;
    let t = FakeTransport::new();
    t.fail_next_write_with(Fault::Other);
    let obs = RecordingObserver::default();

    let done = run(&f.db.engine, &t, &obs, &f.device().await, &no_cancel())
        .await
        .unwrap();

    assert_eq!(done.added, 2, "the two that succeeded");
    assert_eq!(done.skipped, 1);
    assert!(obs.has_warning(DeviceWarningKind::UploadFailed));
    assert_eq!(
        f.manifest_len().await,
        3,
        "2 tracks + 1 playlist; the failed track has no row"
    );
}

#[tokio::test]
async fn a_failed_upload_leaves_no_partial_file_on_the_device() {
    let f = fixture().await;
    let t = FakeTransport::new();
    t.fail_next_write_with(Fault::Other);
    run(
        &f.db.engine,
        &t,
        &RecordingObserver::default(),
        &f.device().await,
        &no_cancel(),
    )
    .await
    .unwrap();

    assert!(
        t.files().iter().all(|(p, _)| !p.ends_with(".tuxpart")),
        "a partial transfer must not survive: {:?}",
        t.files().iter().map(|(p, _)| p).collect::<Vec<_>>()
    );
}

#[tokio::test]
async fn a_real_filesystem_sync_leaves_no_partial_and_copies_exactly() {
    // FakeTransport buffers writes; FsTransport materialises the
    // destination on open. Only this proves the .tuxpart promise holds
    // against a transport that really touches a filesystem.
    let f = fixture().await;
    let mount = tempfile::tempdir().unwrap();
    let t = crate::device::transport::fs::FsTransport::new(mount.path().to_path_buf());

    // Make one source unreadable so the upload of it fails mid-run.
    std::fs::remove_file(&f.track_paths[1]).unwrap();

    let done = run(
        &f.db.engine,
        &t,
        &RecordingObserver::default(),
        &f.device().await,
        &no_cancel(),
    )
    .await
    .unwrap();

    assert_eq!(done.added, 2);
    let mut leftovers = Vec::new();
    let mut stack = vec![mount.path().to_path_buf()];
    while let Some(dir) = stack.pop() {
        for entry in std::fs::read_dir(&dir).unwrap() {
            let path = entry.unwrap().path();
            if path.is_dir() {
                stack.push(path);
            } else if path.to_string_lossy().ends_with(".tuxpart") {
                leftovers.push(path);
            }
        }
    }
    assert!(
        leftovers.is_empty(),
        "partial files left behind: {leftovers:?}"
    );

    let landed = mount.path().join("Music/Bonobo/Migration/01 Kerala.flac");
    assert_eq!(
        std::fs::read(&landed).unwrap(),
        std::fs::read(&f.track_paths[0]).unwrap(),
        "a supported codec must reach a real device bit-exact"
    );
}

#[tokio::test]
async fn an_add_never_overwrites_a_file_we_did_not_write() {
    let f = fixture().await;
    let t = FakeTransport::new();

    // The user put their own file at exactly the path we will render.
    let theirs = DevicePath::new("/Music/Bonobo/Migration/01 Kerala.flac");
    t.mkdir_all(&DevicePath::new("/Music/Bonobo/Migration"))
        .unwrap();
    {
        let mut w = t.open_write(&theirs, 5).unwrap();
        std::io::Write::write_all(&mut w, b"mine!").unwrap();
        std::io::Write::flush(&mut w).unwrap();
    }

    let obs = RecordingObserver::default();
    let done = run(&f.db.engine, &t, &obs, &f.device().await, &no_cancel())
        .await
        .unwrap();

    let (_, bytes) = t
        .files()
        .into_iter()
        .find(|(p, _)| p == theirs.as_str())
        .expect("their file must still be there");
    assert_eq!(
        bytes, b"mine!",
        "an unrecorded file must never be clobbered"
    );
    assert_eq!(done.added, 2, "the other two tracks still sync");
    assert!(obs.has_warning(DeviceWarningKind::UploadFailed));
}

#[tokio::test]
async fn running_out_of_space_mid_copy_fails_rather_than_reporting_success() {
    let f = fixture().await;
    let t = FakeTransport::new();
    // Passes the pre-flight check, then the device fills up mid-write.
    t.fail_next_write_with(Fault::NoSpace);

    let err = run(
        &f.db.engine,
        &t,
        &RecordingObserver::default(),
        &f.device().await,
        &no_cancel(),
    )
    .await
    .expect_err("a device that fills up must not report a clean sync");

    assert!(matches!(err, EngineError::NoSpace { .. }), "{err:?}");
}

#[tokio::test]
async fn out_of_space_aborts_before_writing_anything() {
    let f = fixture().await;
    let t = FakeTransport::new();
    t.set_free_bytes(10);

    let err = run(
        &f.db.engine,
        &t,
        &RecordingObserver::default(),
        &f.device().await,
        &no_cancel(),
    )
    .await
    .expect_err("a plan larger than free space must not start");

    assert!(matches!(err, EngineError::NoSpace { .. }), "{err:?}");
    assert_eq!(f.manifest_len().await, 0);
}

#[tokio::test]
async fn a_missing_source_file_is_skipped_with_a_warning() {
    let f = fixture().await;
    std::fs::remove_file(&f.track_paths[1]).unwrap();
    let t = FakeTransport::new();
    let obs = RecordingObserver::default();

    let done = run(&f.db.engine, &t, &obs, &f.device().await, &no_cancel())
        .await
        .unwrap();

    assert_eq!(done.added, 2);
    assert_eq!(done.skipped, 1);
    assert!(obs.has_warning(DeviceWarningKind::MissingSourceFile));
}

#[tokio::test]
async fn cancellation_stops_before_uploading() {
    let f = fixture().await;
    let t = FakeTransport::new();
    let obs = RecordingObserver::default();

    let err = run(
        &f.db.engine,
        &t,
        &obs,
        &f.device().await,
        &AtomicBool::new(true),
    )
    .await
    .expect_err("a cancelled run must not report success");

    assert!(matches!(err, EngineError::Cancelled), "{err:?}");
    assert!(t.files().is_empty());
    assert_eq!(
        obs.phases().last(),
        Some(&DevicePhase::Finalizing),
        "it still reaches Finalizing, so the UI stops showing progress"
    );
}

#[tokio::test]
async fn a_cancelled_run_does_not_stamp_last_sync_at() {
    let f = fixture().await;
    let t = FakeTransport::new();
    let _ = run(
        &f.db.engine,
        &t,
        &RecordingObserver::default(),
        &f.device().await,
        &AtomicBool::new(true),
    )
    .await;

    assert_eq!(
        f.device().await.last_sync_at,
        None,
        "a partial run must stay distinguishable from a finished one"
    );
}

#[tokio::test]
async fn a_leftover_partial_from_an_earlier_run_is_cleaned_up() {
    let f = fixture().await;
    let t = FakeTransport::new();
    t.mkdir_all(&DevicePath::new("/Music/Bonobo/Migration"))
        .unwrap();
    let stale = DevicePath::new("/Music/Bonobo/Migration/99 Ghost.flac.tuxpart");
    {
        let mut w = t.open_write(&stale, 1).unwrap();
        std::io::Write::write_all(&mut w, b"x").unwrap();
        std::io::Write::flush(&mut w).unwrap();
    }

    run(
        &f.db.engine,
        &t,
        &RecordingObserver::default(),
        &f.device().await,
        &no_cancel(),
    )
    .await
    .unwrap();

    assert_eq!(
        t.stat(&stale).unwrap(),
        None,
        "an interrupted run's leftovers hold no manifest row, so only this cleans them"
    );
}

// ---------------------------------------------------------------- //
// Playlists
// ---------------------------------------------------------------- //

#[tokio::test]
async fn the_m3u8_lists_only_tracks_that_reached_the_device() {
    let f = fixture().await;
    std::fs::remove_file(&f.track_paths[1]).unwrap();
    let t = FakeTransport::new();
    run(
        &f.db.engine,
        &t,
        &RecordingObserver::default(),
        &f.device().await,
        &no_cancel(),
    )
    .await
    .unwrap();

    let body = t
        .read_to_string("/Music/Playlists/Favourites.m3u8")
        .expect("playlist written");
    assert!(body.starts_with("#EXTM3U\n"));
    assert!(body.contains("../Bonobo/Migration/01 Kerala.flac"));
    assert!(
        !body.contains("Outlier"),
        "a track that never landed must not be listed: {body}"
    );
}

#[tokio::test]
async fn a_native_playlist_object_is_registered_when_supported() {
    let f = fixture().await;
    let t = FakeTransport::new();
    run(
        &f.db.engine,
        &t,
        &RecordingObserver::default(),
        &f.device().await,
        &no_cancel(),
    )
    .await
    .unwrap();

    let objects = t.playlist_objects();
    assert_eq!(objects.len(), 1);
    assert_eq!(objects[0].1.len(), 3);
}

#[tokio::test]
async fn a_playlist_object_failure_is_a_warning_not_a_failure() {
    let f = fixture().await;
    let t = FakeTransport::new();
    t.fail_playlist_objects();
    let obs = RecordingObserver::default();

    let done = run(&f.db.engine, &t, &obs, &f.device().await, &no_cancel())
        .await
        .unwrap();

    assert_eq!(done.playlists_written, 1);
    assert!(obs.has_warning(DeviceWarningKind::PlaylistObjectFailed));
    assert!(
        t.read_to_string("/Music/Playlists/Favourites.m3u8")
            .is_some(),
        "the .m3u8 is the guarantee and must still be written"
    );
}

#[tokio::test]
async fn playlist_objects_are_skipped_when_the_device_opts_out() {
    let f = fixture().await;
    set_flag(&f.db, f.device_id, "write_playlist_objects", 0).await;
    let t = FakeTransport::new();
    run(
        &f.db.engine,
        &t,
        &RecordingObserver::default(),
        &f.device().await,
        &no_cancel(),
    )
    .await
    .unwrap();
    assert!(t.playlist_objects().is_empty());
    assert!(t
        .read_to_string("/Music/Playlists/Favourites.m3u8")
        .is_some());
}

#[tokio::test]
async fn a_smart_playlist_resolves_at_sync_time() {
    let f = fixture().await;
    let rule = serde_json::json!({
        "match_all": true,
        "root": {
            "match_all": true,
            "children": [
                { "field": "album", "op": "is", "value": "Migration" }
            ]
        }
    });
    let smart_id = db_playlists::create_smart(&f.db.engine, "Migration", &rule.to_string())
        .await
        .unwrap();
    devices::update_selection(
        &f.db.engine,
        f.device_id,
        &[SelectionEntry::Smart { id: smart_id }],
    )
    .await
    .unwrap();

    let t = FakeTransport::new();
    let done = run(
        &f.db.engine,
        &t,
        &RecordingObserver::default(),
        &f.device().await,
        &no_cancel(),
    )
    .await
    .unwrap();

    assert_eq!(done.added, 3);
    assert!(t
        .read_to_string("/Music/Playlists/Migration.m3u8")
        .is_some());
}

// ---------------------------------------------------------------- //
// Planning
// ---------------------------------------------------------------- //

#[tokio::test]
async fn build_plan_reports_bytes_without_writing() {
    let f = fixture().await;
    let t = FakeTransport::new();
    let (plan, skips) = build_plan(&f.db.engine, &f.device().await, &t)
        .await
        .unwrap();

    assert_eq!(plan.adds.len(), 3);
    assert!(skips.is_empty());
    // 100 + 101 + 102 bytes, as written by the fixture.
    assert_eq!(plan.bytes_out, 303);
    assert!(t.files().is_empty(), "a dry run must not touch the device");
}

#[tokio::test]
async fn two_tracks_rendering_to_one_path_are_both_kept() {
    let f = fixture().await;
    // Give two tracks identical metadata so the template collides.
    f.db.engine
        .raw_sql_execute(
            "UPDATE tracks SET title = 'Kerala', track_number = 1 WHERE id = ?",
            &[FilterValue::Int(f.track_ids[1])],
        )
        .await
        .unwrap();

    let t = FakeTransport::new();
    let (plan, _) = build_plan(&f.db.engine, &f.device().await, &t)
        .await
        .unwrap();
    let paths: Vec<&str> = plan.adds.iter().map(|d| d.device_path.as_str()).collect();

    assert_eq!(paths.len(), 3);
    assert!(
        paths.contains(&"/Music/Bonobo/Migration/01 Kerala (2).flac"),
        "a colliding name must be suffixed, not dropped: {paths:?}"
    );
}
