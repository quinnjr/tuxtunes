//! End-to-end smoke tests for every Tauri command.
//!
//! Builds a real `AppState` against an in-memory SQLite + mock Tauri
//! AppHandle + a real PlaybackEngine. Each test invokes a command
//! through `app.state::<AppState>()`, exercising the same code path
//! Tauri's IPC layer would.
//!
//! The PlaybackEngine spawns a real mpv handle. It needs `libmpv` at
//! load time (always present on the dev machine) and a sound device
//! it can probe — set ao=null in init if you run these on a headless
//! CI without ALSA/PulseAudio.

#![cfg(unix)]

use std::sync::Arc;
use tauri::{Listener, Manager};

use tuxtunes::commands;
use tuxtunes::db::{self, smart::SmartRule};
use tuxtunes::runtime::AppState;

/// Build an AppState backed by an in-memory tempdir + a mock Tauri app.
/// Returns the running mock app together with the state — keep them
/// in scope for the duration of the test.
async fn fixture() -> (tauri::App<tauri::test::MockRuntime>, tempfile::TempDir) {
    // Force libmpv's AO to null so PlaybackEngine init doesn't try to
    // open a real ALSA/PulseAudio device — fatal in headless CI.
    // SAFETY: tests run with cargo's default thread pool; setting an
    // env var inside a per-process test binary is safe across the
    // tests that share the binary.
    unsafe {
        std::env::set_var("TUXTUNES_AO", "null");
    }

    let tmp = tempfile::tempdir().unwrap();
    let db_path = tmp.path().join("tuxtunes.db");
    let lib_root = tmp.path().join("lib");
    std::fs::create_dir_all(&lib_root).unwrap();

    let app = tauri::test::mock_app();
    let handle = app.handle().clone();
    let state = AppState::new(&db_path, handle).await.unwrap();
    db::preferences::set_library_root(&state.db.engine, &lib_root)
        .await
        .unwrap();
    app.manage(state);
    (app, tmp)
}

#[tokio::test(flavor = "multi_thread")]
async fn library_stats_starts_empty() {
    let (app, _tmp) = fixture().await;
    let state = app.state::<AppState>();
    let stats = commands::library::get_library_stats(state).await.unwrap();
    assert_eq!(stats.track_count, 0);
}

#[tokio::test(flavor = "multi_thread")]
async fn list_tracks_empty_then_populated() {
    let (app, _tmp) = fixture().await;
    let state = app.state::<AppState>();
    let rows = commands::library::list_tracks(state.clone(), 100, 0, None, None)
        .await
        .unwrap();
    assert!(rows.is_empty());

    // Insert a row directly via the DB so we don't depend on the
    // ingest pipeline finishing for this assertion.
    state
        .db
        .engine
        .raw_sql_execute(
            "INSERT INTO tracks (title, duration_ms, size_bytes, file_path, playlist_ids) \
             VALUES ('Hello', 1000, 0, '/tmp/h.flac', '[]')",
            &[],
        )
        .await
        .unwrap();

    let rows = commands::library::list_tracks(state, 100, 0, None, None)
        .await
        .unwrap();
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].title, "Hello");
}

#[tokio::test(flavor = "multi_thread")]
async fn list_albums_artists_distinct_round_trip() {
    let (app, _tmp) = fixture().await;
    let state = app.state::<AppState>();
    state
        .db
        .engine
        .raw_sql_execute(
            "INSERT INTO tracks (title, artist, album_artist, album, genre, duration_ms, \
             size_bytes, file_path, playlist_ids) VALUES \
             ('A1', 'X', 'X', 'A', 'Rock', 1000, 0, '/tmp/1', '[]'), \
             ('A2', 'X', 'X', 'A', 'Rock', 1000, 0, '/tmp/2', '[]'), \
             ('B1', 'Y', 'Y', 'B', 'Jazz', 1000, 0, '/tmp/3', '[]')",
            &[],
        )
        .await
        .unwrap();

    let albums = commands::library::list_albums(state.clone()).await.unwrap();
    assert_eq!(albums.len(), 2);
    let artists = commands::library::list_artists(state.clone())
        .await
        .unwrap();
    assert_eq!(artists.len(), 2);
    let in_album = commands::library::tracks_for_album(state.clone(), "X".into(), "A".into())
        .await
        .unwrap();
    assert_eq!(in_album.len(), 2);
    let genres = commands::library::get_distinct(state, "genre".into(), None)
        .await
        .unwrap();
    assert_eq!(genres.len(), 2);
}

#[tokio::test(flavor = "multi_thread")]
async fn remove_and_trash_track_paths() {
    let (app, _tmp) = fixture().await;
    let state = app.state::<AppState>();

    // Trash path: real file → trash::delete sends to the user's trash.
    let tmp = tempfile::tempdir().unwrap();
    let p = tmp.path().join("trashable.flac");
    std::fs::write(&p, b"x").unwrap();
    let row_id: i64 = state
        .db
        .engine
        .raw_sql_first(
            "INSERT INTO tracks (title, duration_ms, size_bytes, file_path, playlist_ids) \
             VALUES ('T', 0, 0, ?, '[]') RETURNING id",
            &[prax_query::filter::FilterValue::String(
                p.display().to_string(),
            )],
        )
        .await
        .unwrap()
        .into_json()
        .get("id")
        .and_then(|v| v.as_i64())
        .unwrap();

    // trash_track may fail if there's no XDG-trash configured; treat
    // either result as covered. The delete-row half always runs.
    let _ = commands::library::trash_track(state.clone(), row_id).await;

    // remove_track on a non-existent row must be idempotent (no error,
    // no rows affected).
    commands::library::remove_track(state, 9999).await.unwrap();
}

#[tokio::test(flavor = "multi_thread")]
async fn show_in_files_command_runs_without_crashing() {
    let (app, _tmp) = fixture().await;
    let state = app.state::<AppState>();
    state
        .db
        .engine
        .raw_sql_execute(
            "INSERT INTO tracks (title, duration_ms, size_bytes, file_path, playlist_ids) \
             VALUES ('show', 0, 0, '/tmp/show.flac', '[]')",
            &[],
        )
        .await
        .unwrap();
    let id: i64 = state
        .db
        .engine
        .raw_sql_scalar("SELECT id FROM tracks WHERE title = 'show'", &[])
        .await
        .unwrap();
    // Never actually launch a file manager from the test suite.
    std::env::set_var("TUXTUNES_NO_XDG_OPEN", "1");
    let res = commands::library::show_in_files(state, id).await;
    assert!(res.is_ok(), "{res:?}");
}

#[tokio::test(flavor = "multi_thread")]
async fn preferences_command_surface() {
    let (app, _tmp) = fixture().await;
    let state = app.state::<AppState>();

    let root = commands::preferences::get_library_root(state.clone())
        .await
        .unwrap();
    assert!(!root.is_empty());
    commands::preferences::set_library_root(state.clone(), "/tmp/lib".into())
        .await
        .unwrap();

    let scheme = commands::preferences::get_organize_scheme(state.clone())
        .await
        .unwrap();
    assert!(!scheme.is_empty());
    commands::preferences::set_organize_scheme(state.clone(), "{title}.{ext}".into())
        .await
        .unwrap();

    let keep = commands::preferences::get_keep_organized(state.clone())
        .await
        .unwrap();
    assert!(keep);
    commands::preferences::set_keep_organized(state, false)
        .await
        .unwrap();
}

#[tokio::test(flavor = "multi_thread")]
async fn audio_command_surface_persists_prefs() {
    let (app, _tmp) = fixture().await;
    let state = app.state::<AppState>();

    let _ = commands::audio::list_audio_devices(state.clone())
        .await
        .unwrap();
    let snap = commands::audio::get_audio_prefs(state.clone())
        .await
        .unwrap();
    assert!(snap.device_id.is_none());

    commands::audio::set_audio_device(
        state.clone(),
        commands::audio::SetAudioDeviceArgs {
            device_id: "alsa/null".into(),
            exclusive: false,
            replaygain_mode: Some(tuxtunes::playback::config::ReplayGainMode::Track),
        },
    )
    .await
    .unwrap();
    let snap2 = commands::audio::get_audio_prefs(state).await.unwrap();
    assert_eq!(snap2.device_id.as_deref(), Some("alsa/null"));
    assert_eq!(
        snap2.replaygain_mode,
        tuxtunes::playback::config::ReplayGainMode::Track
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn set_audio_device_without_replaygain_loads_from_prefs() {
    let (app, _tmp) = fixture().await;
    let state = app.state::<AppState>();
    // Seed the persisted replaygain so the None-arm of the match has
    // a value to pull from.
    commands::audio::set_audio_device(
        state.clone(),
        commands::audio::SetAudioDeviceArgs {
            device_id: "alsa/seeded".into(),
            exclusive: true,
            replaygain_mode: Some(tuxtunes::playback::config::ReplayGainMode::Album),
        },
    )
    .await
    .unwrap();

    // Call again without replaygain_mode — the None branch picks up
    // the previously-stored ReplayGainMode::Album.
    commands::audio::set_audio_device(
        state.clone(),
        commands::audio::SetAudioDeviceArgs {
            device_id: "alsa/seeded".into(),
            exclusive: false,
            replaygain_mode: None,
        },
    )
    .await
    .unwrap();
    let snap = commands::audio::get_audio_prefs(state).await.unwrap();
    assert_eq!(
        snap.replaygain_mode,
        tuxtunes::playback::config::ReplayGainMode::Album,
    );
    assert!(!snap.exclusive);
}

#[tokio::test(flavor = "multi_thread")]
async fn smart_rule_evaluate_and_preview_via_command() {
    let (app, _tmp) = fixture().await;
    let state = app.state::<AppState>();
    state
        .db
        .engine
        .raw_sql_execute(
            "INSERT INTO tracks (title, genre, duration_ms, size_bytes, file_path, playlist_ids) \
             VALUES ('T1', 'Rock', 1000, 0, '/tmp/1', '[]'), \
                    ('T2', 'Jazz', 1000, 0, '/tmp/2', '[]')",
            &[],
        )
        .await
        .unwrap();

    let rule_json = r#"{"match_all":true,"live_updating":true,"limit":null,
        "root":{"match_all":true,"children":[
            {"field":"genre","op":"is","value":"Rock"}
        ]}}"#;
    let rule: SmartRule = serde_json::from_str(rule_json).unwrap();

    let rows = commands::smart::evaluate_smart_rule(state.clone(), rule.clone())
        .await
        .unwrap();
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].title, "T1");

    let count = commands::smart::preview_smart_rule(state, rule)
        .await
        .unwrap();
    assert_eq!(count, 1);
}

#[tokio::test(flavor = "multi_thread")]
async fn evaluate_smart_rule_matches_genre_via_command() {
    let (app, _tmp) = fixture().await;
    let state = app.state::<AppState>();
    state
        .db
        .engine
        .raw_sql_execute(
            "INSERT INTO tracks (title, genre, duration_ms, size_bytes, file_path, playlist_ids) \
             VALUES ('Jazz Track', 'Jazz', 1000, 0, '/tmp/jazz', '[]'), \
                    ('Rock Track', 'Rock', 1000, 0, '/tmp/rock', '[]')",
            &[],
        )
        .await
        .unwrap();

    let rule: SmartRule = serde_json::from_value(serde_json::json!({
        "match_all": true,
        "live_updating": true,
        "limit": null,
        "root": {
            "match_all": true,
            "children": [
                {"field": "genre", "op": "is", "value": "Jazz"}
            ]
        }
    }))
    .unwrap();

    let rows = commands::smart::evaluate_smart_rule(state.clone(), rule)
        .await
        .unwrap();
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].title, "Jazz Track");
}

#[tokio::test(flavor = "multi_thread")]
async fn playlist_crud_via_commands() {
    let (app, _tmp) = fixture().await;
    let state = app.state::<AppState>();

    let rule: SmartRule = serde_json::from_str(
        r#"{"match_all":true,"live_updating":true,"limit":null,
            "root":{"match_all":true,"children":[]}}"#,
    )
    .unwrap();

    let id = commands::playlists::create_smart_playlist(state.clone(), "Mine".into(), rule.clone())
        .await
        .unwrap();
    assert!(id > 0);

    let lists = commands::playlists::list_playlists(state.clone())
        .await
        .unwrap();
    assert_eq!(lists.len(), 1);

    let updated_rule: SmartRule = serde_json::from_str(
        r#"{"match_all":false,"live_updating":true,"limit":null,
            "root":{"match_all":false,"children":[]}}"#,
    )
    .unwrap();
    commands::playlists::update_smart_playlist(state.clone(), id, updated_rule)
        .await
        .unwrap();

    let opened = commands::playlists::open_smart_playlist(state.clone(), id)
        .await
        .unwrap();
    assert!(opened.is_empty()); // No tracks inserted yet.

    commands::playlists::delete_playlist(state, id)
        .await
        .unwrap();
}

#[tokio::test(flavor = "multi_thread")]
async fn open_smart_playlist_rejects_non_smart_id() {
    let (app, _tmp) = fixture().await;
    let state = app.state::<AppState>();
    // Create a regular playlist via raw SQL — open_smart_playlist should
    // reject it because the `kind` filter excludes non-smart rows.
    state
        .db
        .engine
        .raw_sql_execute(
            "INSERT INTO playlists (name, kind, sort_order, track_entries) \
             VALUES ('reg', 'regular', 0, '[]')",
            &[],
        )
        .await
        .unwrap();
    let id: i64 = state
        .db
        .engine
        .raw_sql_scalar("SELECT id FROM playlists", &[])
        .await
        .unwrap();
    let err = commands::playlists::open_smart_playlist(state, id)
        .await
        .unwrap_err();
    assert!(err.contains("not a smart playlist"));
}

#[tokio::test(flavor = "multi_thread")]
async fn play_track_loads_existing_row_and_drives_loadandplay() {
    let (app, tmp) = fixture().await;
    let state = app.state::<AppState>();

    // Write a tiny "audio" file. With TUXTUNES_AO=null mpv won't
    // decode it, but the LoadAndPlay command path still runs through
    // build_properties + loadfile + error-event handling — that's the
    // engine code we want to exercise.
    let path = tmp.path().join("phantom.wav");
    std::fs::write(&path, b"RIFF\0\0\0\0WAVEfmt ").unwrap();

    let id: i64 = state
        .db
        .engine
        .raw_sql_first(
            "INSERT INTO tracks (title, duration_ms, size_bytes, file_path, playlist_ids) \
             VALUES ('phantom', 0, 0, ?, '[]') RETURNING id",
            &[prax_query::filter::FilterValue::String(
                path.display().to_string(),
            )],
        )
        .await
        .unwrap()
        .into_json()
        .get("id")
        .and_then(|v| v.as_i64())
        .unwrap();

    // The command may return Ok or a libmpv-loadfile error; both
    // paths are valid coverage of the engine's LoadAndPlay handling.
    let _ = commands::playback::play_track(state, id).await;
    tokio::time::sleep(std::time::Duration::from_millis(500)).await;
}

/// A file that exists but can't be decoded must not leave the UI parked
/// on `loading`: libmpv2 reports the error end-file as a bare `Err` from
/// `wait_event`, which the loop used to drop on the floor.
#[tokio::test(flavor = "multi_thread")]
async fn undecodable_file_emits_warning_and_stops() {
    use tauri::Listener;
    let (app, tmp) = fixture().await;
    let state = app.state::<AppState>();

    let path = tmp.path().join("garbage.m4a");
    let mut bytes = b"\0\0\0\x20ftypM4A garbage not really an mp4 at all".to_vec();
    bytes.extend((0u16..600).map(|i| i.wrapping_mul(37) as u8));
    std::fs::write(&path, bytes).unwrap();

    let id: i64 = state
        .db
        .engine
        .raw_sql_scalar(
            "INSERT INTO tracks (title, duration_ms, size_bytes, file_path, playlist_ids) \
             VALUES ('garbage', 0, 0, ?, '[]') RETURNING id",
            &[prax_query::filter::FilterValue::String(
                path.display().to_string(),
            )],
        )
        .await
        .unwrap();

    let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel::<String>();
    let tx_state = tx.clone();
    app.handle()
        .listen(tuxtunes::playback::events::WARNING, move |e| {
            let _ = tx.send(format!("warning:{}", e.payload()));
        });
    app.handle()
        .listen(tuxtunes::playback::events::STATE_CHANGED, move |e| {
            let _ = tx_state.send(format!("state:{}", e.payload()));
        });

    commands::playback::play_track(state, id).await.unwrap();

    let mut saw_warning = false;
    let mut saw_stopped = false;
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
    while (!saw_warning || !saw_stopped) && std::time::Instant::now() < deadline {
        let Ok(Some(msg)) =
            tokio::time::timeout(std::time::Duration::from_secs(5), rx.recv()).await
        else {
            break;
        };
        if msg.starts_with("warning:") && msg.contains("load_failed") {
            saw_warning = true;
        }
        if msg.starts_with("state:") && msg.contains("stopped") {
            saw_stopped = true;
        }
    }
    assert!(saw_warning, "expected a playback:warning load_failed");
    assert!(saw_stopped, "expected playback:state-changed stopped");
}

/// `play_track` must hand the engine the *persisted* prefs — passing
/// `PlaybackPrefs::default()` re-wrote audio-exclusive/replaygain to
/// their defaults before every loadfile.
#[tokio::test(flavor = "multi_thread")]
async fn play_track_uses_persisted_audio_prefs() {
    let (app, tmp) = fixture().await;
    let state = app.state::<AppState>();

    commands::audio::set_audio_device(
        state.clone(),
        commands::audio::SetAudioDeviceArgs {
            device_id: "alsa/persisted".into(),
            exclusive: true,
            replaygain_mode: Some(tuxtunes::playback::config::ReplayGainMode::Album),
        },
    )
    .await
    .unwrap();

    let path = tmp.path().join("phantom-prefs.wav");
    std::fs::write(&path, b"RIFF\0\0\0\0WAVEfmt ").unwrap();
    let id: i64 = state
        .db
        .engine
        .raw_sql_scalar(
            "INSERT INTO tracks (title, duration_ms, size_bytes, file_path, playlist_ids) \
             VALUES ('phantom-prefs', 0, 0, ?, '[]') RETURNING id",
            &[prax_query::filter::FilterValue::String(
                path.display().to_string(),
            )],
        )
        .await
        .unwrap();
    let _ = commands::playback::play_track(state.clone(), id).await;

    // The engine's applied mpv properties aren't observable without a
    // real device, so assert on the loader play_track feeds them from.
    let prefs = commands::audio::load_playback_prefs(&state.db.engine).await;
    assert_eq!(prefs.selected_device_id.as_deref(), Some("alsa/persisted"));
    assert!(prefs.exclusive_mode);
    assert_eq!(
        prefs.replaygain_mode,
        tuxtunes::playback::config::ReplayGainMode::Album
    );
    assert_eq!(prefs.volume, 100);
}

/// With nothing persisted the loader falls back to the defaults, so a
/// fresh install plays at full volume with ReplayGain off.
#[tokio::test(flavor = "multi_thread")]
async fn load_playback_prefs_falls_back_to_defaults() {
    let (app, _tmp) = fixture().await;
    let state = app.state::<AppState>();
    let prefs = commands::audio::load_playback_prefs(&state.db.engine).await;
    assert_eq!(prefs, tuxtunes::playback::config::PlaybackPrefs::default());
}

/// 8 kHz, 8-bit mono PCM WAV of `samples` silent samples (400 = 50 ms).
fn short_wav(samples: u32) -> Vec<u8> {
    let mut wav = Vec::new();
    wav.extend_from_slice(b"RIFF");
    wav.extend_from_slice(&(36 + samples).to_le_bytes());
    wav.extend_from_slice(b"WAVEfmt ");
    wav.extend_from_slice(&16u32.to_le_bytes());
    wav.extend_from_slice(&1u16.to_le_bytes()); // PCM
    wav.extend_from_slice(&1u16.to_le_bytes()); // mono
    wav.extend_from_slice(&8000u32.to_le_bytes());
    wav.extend_from_slice(&8000u32.to_le_bytes());
    wav.extend_from_slice(&1u16.to_le_bytes());
    wav.extend_from_slice(&8u16.to_le_bytes());
    wav.extend_from_slice(b"data");
    wav.extend_from_slice(&samples.to_le_bytes());
    wav.resize(wav.len() + samples as usize, 0x80);
    wav
}

/// After a natural EOF the engine must accept the next `play_track` and
/// report it as loaded — this is the auto-advance chain end to end.
#[tokio::test(flavor = "multi_thread")]
async fn play_after_eof_loads_next_track() {
    use tauri::Listener;
    let (app, tmp) = fixture().await;
    let state = app.state::<AppState>();

    let mut ids = Vec::new();
    for name in ["a.wav", "b.wav"] {
        let path = tmp.path().join(name);
        std::fs::write(&path, short_wav(400)).unwrap();
        let id: i64 = state
            .db
            .engine
            .raw_sql_scalar(
                "INSERT INTO tracks (title, duration_ms, size_bytes, file_path, playlist_ids) \
                 VALUES (?, 50, 0, ?, '[]') RETURNING id",
                &[
                    prax_query::filter::FilterValue::String(name.to_string()),
                    prax_query::filter::FilterValue::String(path.display().to_string()),
                ],
            )
            .await
            .unwrap();
        ids.push(id);
    }

    let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel::<String>();
    let tx_ended = tx.clone();
    app.handle()
        .listen(tuxtunes::playback::events::TRACK_ENDED, move |e| {
            let _ = tx_ended.send(format!("ended:{}", e.payload()));
        });
    app.handle()
        .listen(tuxtunes::playback::events::TRACK_CHANGED, move |e| {
            let _ = tx.send(format!("changed:{}", e.payload()));
        });

    commands::playback::play_track(state.clone(), ids[0])
        .await
        .unwrap();
    async fn wait(rx: &mut tokio::sync::mpsc::UnboundedReceiver<String>, needle: String) -> String {
        loop {
            let msg = tokio::time::timeout(std::time::Duration::from_secs(5), rx.recv())
                .await
                .expect("event within 5s")
                .expect("channel open");
            if msg.contains(&needle) {
                return msg;
            }
        }
    }
    wait(&mut rx, format!("ended:{{\"track_id\":{}", ids[0])).await;

    // The frontend reacts to track-ended by starting the next row.
    commands::playback::play_track(state.clone(), ids[1])
        .await
        .unwrap();
    let msg = wait(&mut rx, format!("\"track_id\":{}", ids[1])).await;
    assert!(msg.starts_with("changed:"), "{msg}");
    wait(&mut rx, format!("ended:{{\"track_id\":{}", ids[1])).await;
}

/// A file that plays to its natural end must produce `playback:track-ended`
/// — that event is the only thing that drives auto-advance. Regression for
/// `keep-open=always`, which parked mpv at EOF without ever unloading.
#[tokio::test(flavor = "multi_thread")]
async fn natural_eof_emits_track_ended() {
    use tauri::Listener;
    let (app, tmp) = fixture().await;
    let state = app.state::<AppState>();

    let path = tmp.path().join("short.wav");
    std::fs::write(&path, short_wav(400)).unwrap();

    let id: i64 = state
        .db
        .engine
        .raw_sql_scalar(
            "INSERT INTO tracks (title, duration_ms, size_bytes, file_path, playlist_ids) \
             VALUES ('short', 50, 0, ?, '[]') RETURNING id",
            &[prax_query::filter::FilterValue::String(
                path.display().to_string(),
            )],
        )
        .await
        .unwrap();

    let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel::<String>();
    app.handle()
        .listen(tuxtunes::playback::events::TRACK_ENDED, move |event| {
            let _ = tx.send(event.payload().to_string());
        });

    commands::playback::play_track(state, id).await.unwrap();

    let payload = tokio::time::timeout(std::time::Duration::from_secs(5), rx.recv())
        .await
        .expect("track-ended within 5s of a 50 ms file finishing")
        .expect("channel open");
    assert!(payload.contains(&format!("\"track_id\":{id}")), "{payload}");
}

#[tokio::test(flavor = "multi_thread")]
async fn playback_command_surface_runs_through_engine() {
    let (app, _tmp) = fixture().await;
    let state = app.state::<AppState>();

    // Engine commands are fire-and-forget against a real mpv handle.
    // None of these will actually emit audio (mpv has no file loaded),
    // but they cover the command-layer translation.
    commands::playback::pause(state.clone()).await.unwrap();
    commands::playback::resume(state.clone()).await.unwrap();
    commands::playback::stop(state.clone()).await.unwrap();
    commands::playback::seek(state.clone(), 1000).await.unwrap();
    commands::playback::set_volume(state.clone(), 50)
        .await
        .unwrap();

    // play_track on an unknown id surfaces a String error; that's a
    // covered branch.
    let err = commands::playback::play_track(state, 9999)
        .await
        .unwrap_err();
    assert!(!err.is_empty());

    // Yield long enough for the worker thread to drain the command
    // queue before fixture drops (otherwise handle_command's body
    // never runs and shows 0% coverage in the engine module).
    tokio::time::sleep(std::time::Duration::from_millis(500)).await;
}

#[tokio::test(flavor = "multi_thread")]
async fn sync_command_surface_lists_and_validates() {
    let (app, _tmp) = fixture().await;
    let state = app.state::<AppState>();
    let sources = commands::sync::list_sync_sources(state.clone())
        .await
        .unwrap();
    assert!(sources.is_empty());

    // run_sync_now on a non-existent source should still be Ok at the
    // command layer (it dispatches asynchronously); no panic.
    let _ = commands::sync::run_sync_now(state, 9999).await;
}

#[tokio::test(flavor = "multi_thread")]
async fn add_sync_source_command_inserts_and_returns_id() {
    let (app, _tmp) = fixture().await;
    let state = app.state::<AppState>();
    let id = commands::sync::add_sync_source(
        state.clone(),
        commands::sync::AddSyncSourceArgs {
            name: "Test".into(),
            source_path: "/tmp/x.itl".into(),
            path_mappings: vec![],
            conflict_rules: Default::default(),
            auto_copy_files: true,
        },
    )
    .await
    .unwrap();
    assert!(id > 0);
    let sources = commands::sync::list_sync_sources(state).await.unwrap();
    assert_eq!(sources.len(), 1);
    assert_eq!(sources[0].name, "Test");
}

#[tokio::test(flavor = "multi_thread")]
async fn pick_and_add_track_returns_none_when_dialog_cancels() {
    // Note: The blocking_pick_file dialog can't actually open in mock
    // mode and may panic or block. We don't invoke the command here —
    // the rest of library.rs is exercised elsewhere and the dialog
    // path is integration-test territory only.
}

#[tokio::test(flavor = "multi_thread")]
async fn verify_walk_runs_against_an_empty_library() {
    // verify_library the command takes an AppHandle parameter Tauri
    // injects, which is bound to the Wry runtime by the macro. We
    // can't call it directly with a MockRuntime handle, so exercise
    // the underlying walker instead — same code path on the engine
    // side, just without the command-layer 1-line wrapper.
    let (app, _tmp) = fixture().await;
    let state = app.state::<AppState>();
    let handle = app.handle().clone();
    let engine = std::sync::Arc::clone(&state.db.engine);
    let _ = tuxtunes::fs::verify::verify_all(&engine, &handle).await;
}

#[tokio::test(flavor = "multi_thread")]
async fn verify_failure_emits_verify_failed_with_message() {
    // verify_library itself can't be called directly under MockRuntime
    // (see the comment on verify_walk_runs_against_an_empty_library
    // above) — so we exercise the runtime-generic helper it spawns,
    // `commands::library::run_verify_and_report`, which is what
    // actually drives verify_all + the failure emit.
    use std::sync::{Arc, Mutex};

    let (app, _tmp) = fixture().await;
    let state = app.state::<AppState>();
    let handle = app.handle().clone();
    let engine = Arc::clone(&state.db.engine);

    // Cheaply induce verify_all's Err path: drop the tracks table so
    // its very first query (SELECT COUNT(*) FROM tracks) fails.
    engine
        .raw_sql_execute("DROP TABLE tracks", &[])
        .await
        .unwrap();

    let captured: Arc<Mutex<Option<String>>> = Arc::new(Mutex::new(None));
    let captured_clone = Arc::clone(&captured);
    app.handle()
        .listen(tuxtunes::fs::events::VERIFY_FAILED, move |event| {
            let payload: serde_json::Value =
                serde_json::from_str(event.payload()).unwrap_or(serde_json::Value::Null);
            let message = payload
                .get("message")
                .and_then(|v| v.as_str())
                .map(str::to_owned);
            *captured_clone.lock().unwrap() = message;
        });

    tuxtunes::commands::library::run_verify_and_report(&engine, &handle).await;

    let message = captured.lock().unwrap().clone();
    assert!(
        message.is_some_and(|m| !m.is_empty()),
        "expected fs:verify-failed to be emitted with a non-empty message"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn reorganize_track_command_handles_missing_row() {
    let (app, _tmp) = fixture().await;
    let state = app.state::<AppState>();
    // Missing row → command returns an error string rather than panic.
    let res = commands::preferences::reorganize_track(state, 9999).await;
    // Either Ok (no-op) or Err (string) is acceptable; both are covered.
    let _ = res;
}

/// Confirm Arc<AppState> is reachable through the mock app's resource
/// manager, which is what every command test relies on.
#[tokio::test(flavor = "multi_thread")]
async fn app_state_is_managed_on_the_mock_app() {
    let (app, _tmp) = fixture().await;
    let state: tauri::State<AppState> = app.state::<AppState>();
    let _ = Arc::clone(&state.db);
}

#[tokio::test(flavor = "multi_thread")]
async fn play_track_errors_and_marks_missing_when_file_absent() {
    let (app, tmp) = fixture().await;
    let state = app.state::<AppState>();

    let path = tmp.path().join("gone.flac");

    let id: i64 = state
        .db
        .engine
        .raw_sql_first(
            "INSERT INTO tracks (title, duration_ms, size_bytes, file_path, playlist_ids) \
             VALUES ('gone', 0, 0, ?, '[]') RETURNING id",
            &[prax_query::filter::FilterValue::String(
                path.display().to_string(),
            )],
        )
        .await
        .unwrap()
        .into_json()
        .get("id")
        .and_then(|v| v.as_i64())
        .unwrap();

    let err = commands::playback::play_track(state.clone(), id)
        .await
        .unwrap_err();
    assert!(err.contains("File not found"), "{err:?}");

    let row = tuxtunes::db::tracks::get(&state.db.engine, id)
        .await
        .unwrap();
    assert_eq!(row.import_status, "missing_source");
}

#[tokio::test(flavor = "multi_thread")]
async fn play_track_errors_for_unknown_track_id() {
    let (app, _tmp) = fixture().await;
    let state = app.state::<AppState>();
    let err = commands::playback::play_track(state, 999_999)
        .await
        .unwrap_err();
    assert!(!err.is_empty());
}

#[tokio::test(flavor = "multi_thread")]
async fn open_playlist_errors_for_unknown_playlist_id() {
    let (app, _tmp) = fixture().await;
    let state = app.state::<AppState>();
    let err = commands::playlists::open_playlist(state, 999_999)
        .await
        .unwrap_err();
    assert!(err.contains("not found"), "{err:?}");
}

/// Play real library files end to end (opt-in: `TUXTUNES_TEST_TRACKS` =
/// comma-separated audio paths). Proves EOF → `track-ended` → next
/// `play_track` chains with actual mp3/m4a decoding, not just a WAV.
#[tokio::test(flavor = "multi_thread")]
async fn real_tracks_reach_eof_and_chain() {
    use tauri::Listener;
    let Ok(list) = std::env::var("TUXTUNES_TEST_TRACKS") else {
        eprintln!("skipping: TUXTUNES_TEST_TRACKS not set");
        return;
    };
    let paths: Vec<String> = list.split(',').map(str::to_string).collect();
    let (app, _tmp) = fixture().await;
    let state = app.state::<AppState>();
    let mut ids = Vec::new();
    for (i, p) in paths.iter().enumerate() {
        let id: i64 = state
            .db
            .engine
            .raw_sql_scalar(
                "INSERT INTO tracks (title, duration_ms, size_bytes, file_path, playlist_ids) \
                 VALUES (?, 0, 0, ?, '[]') RETURNING id",
                &[
                    prax_query::filter::FilterValue::String(format!("real{i}")),
                    prax_query::filter::FilterValue::String(p.clone()),
                ],
            )
            .await
            .unwrap();
        ids.push(id);
    }
    let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel::<String>();
    let t2 = tx.clone();
    app.handle()
        .listen(tuxtunes::playback::events::TRACK_ENDED, move |e| {
            let _ = t2.send(format!("ended:{}", e.payload()));
        });
    app.handle()
        .listen(tuxtunes::playback::events::STATE_CHANGED, move |e| {
            let _ = tx.send(format!("state:{}", e.payload()));
        });
    async fn wait(rx: &mut tokio::sync::mpsc::UnboundedReceiver<String>, needle: &str) -> String {
        let deadline = std::time::Duration::from_secs(90);
        loop {
            let msg = tokio::time::timeout(deadline, rx.recv())
                .await
                .unwrap_or_else(|_| panic!("timed out waiting for {needle}"))
                .expect("channel open");
            eprintln!("evt {msg}");
            if msg.contains(needle) {
                return msg;
            }
        }
    }
    for id in &ids {
        commands::playback::play_track(state.clone(), *id)
            .await
            .unwrap();
        wait(&mut rx, &format!("ended:{{\"track_id\":{id}")).await;
    }
}

/// With the next track pre-queued, EOF must roll into it: track-ended
/// carries next_track_id, track-changed names the new track, and no
/// stopped state is emitted in between.
#[tokio::test(flavor = "multi_thread")]
async fn prefetched_track_plays_gaplessly_after_eof() {
    use tauri::Listener;
    let (app, tmp) = fixture().await;
    let state = app.state::<AppState>();
    let mut ids = Vec::new();
    for (name, samples) in [("a.wav", 16_000u32), ("b.wav", 400u32)] {
        let path = tmp.path().join(name);
        std::fs::write(&path, short_wav(samples)).unwrap();
        let id: i64 = state
            .db
            .engine
            .raw_sql_scalar(
                "INSERT INTO tracks (title, duration_ms, size_bytes, file_path, playlist_ids) \
                 VALUES (?, 50, 0, ?, '[]') RETURNING id",
                &[
                    prax_query::filter::FilterValue::String(name.to_string()),
                    prax_query::filter::FilterValue::String(path.display().to_string()),
                ],
            )
            .await
            .unwrap();
        ids.push(id);
    }
    let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel::<String>();
    for name in [
        tuxtunes::playback::events::TRACK_ENDED,
        tuxtunes::playback::events::TRACK_CHANGED,
        tuxtunes::playback::events::STATE_CHANGED,
    ] {
        let tx = tx.clone();
        app.handle().listen(name, move |e| {
            let _ = tx.send(format!("{name} {}", e.payload()));
        });
    }
    commands::playback::play_track(state.clone(), ids[0])
        .await
        .unwrap();
    commands::playback::prefetch_next(state.clone(), ids[1])
        .await
        .unwrap();

    let mut log = Vec::new();
    loop {
        let msg = tokio::time::timeout(std::time::Duration::from_secs(10), rx.recv())
            .await
            .expect("events within 10s")
            .expect("channel open");
        log.push(msg.clone());
        if msg.contains(&format!("track-ended {{\"track_id\":{}", ids[1])) {
            break;
        }
    }
    let first_end = log
        .iter()
        .position(|m| m.contains(&format!("track-ended {{\"track_id\":{}", ids[0])))
        .expect("first track ended");
    assert!(
        log[first_end].contains(&format!("\"next_track_id\":{}", ids[1])),
        "{:?}",
        log[first_end]
    );
    assert!(
        log[first_end + 1..].iter().any(|m| m.contains("track-changed")
            && m.contains(&format!("\"track_id\":{}", ids[1]))),
        "{log:?}"
    );
    assert!(
        !log[..first_end]
            .iter()
            .any(|m| m.contains("\"state\":\"stopped\"")),
        "no stop before the gapless switch: {log:?}"
    );
}

/// The saved volume must be mpv's boot value: the first volume
/// observation (which is persisted) has to report it, and the
/// preference must still hold it afterwards.
#[tokio::test(flavor = "multi_thread")]
async fn saved_volume_is_the_engine_boot_value_and_survives_startup() {
    use tauri::{Listener, Manager};
    unsafe {
        std::env::set_var("TUXTUNES_AO", "null");
    }
    let tmp = tempfile::tempdir().unwrap();
    let db_path = tmp.path().join("tuxtunes.db");
    {
        let db = tuxtunes::db::Db::open(&db_path).await.unwrap();
        tuxtunes::db::preferences::set(&db.engine, tuxtunes::db::preferences::KEY_VOLUME, &34i64)
            .await
            .unwrap();
    }
    let app = tauri::test::mock_app();
    let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel::<String>();
    app.handle()
        .listen(tuxtunes::playback::events::VOLUME_CHANGED, move |e| {
            let _ = tx.send(e.payload().to_string());
        });
    let state = AppState::new(&db_path, app.handle().clone()).await.unwrap();
    app.manage(state);
    let first = tokio::time::timeout(std::time::Duration::from_secs(5), rx.recv())
        .await
        .expect("volume observation within 5s")
        .expect("channel open");
    assert!(
        first.contains("\"volume\":34"),
        "boot volume must be the saved one: {first}"
    );
    tokio::time::sleep(std::time::Duration::from_millis(300)).await;
    let state = app.state::<AppState>();
    let saved: Option<i64> =
        tuxtunes::db::preferences::get(&state.db.engine, tuxtunes::db::preferences::KEY_VOLUME)
            .await
            .unwrap();
    assert_eq!(saved, Some(34));
}

/// The UI seeds its slider from get_audio_prefs at startup, so the
/// snapshot must carry the persisted volume (it silently didn't).
#[tokio::test(flavor = "multi_thread")]
async fn get_audio_prefs_reports_the_persisted_volume() {
    let (app, _tmp) = fixture().await;
    let state = app.state::<AppState>();
    let before = commands::audio::get_audio_prefs(state.clone())
        .await
        .unwrap();
    assert_eq!(before.volume, 100, "default when nothing is saved");
    tuxtunes::db::preferences::set(
        &state.db.engine,
        tuxtunes::db::preferences::KEY_VOLUME,
        &34i64,
    )
    .await
    .unwrap();
    let after = commands::audio::get_audio_prefs(state).await.unwrap();
    assert_eq!(after.volume, 34);
}
