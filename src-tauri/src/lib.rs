mod commands;
pub mod db;
pub mod fs;
pub mod integration;
pub mod library;
pub mod playback;
mod runtime;
pub mod sync;

use runtime::AppState;
use std::path::PathBuf;
use std::sync::Arc;
use tauri::{Listener, Manager};

fn data_dir(app: &tauri::App) -> PathBuf {
    app.path().app_data_dir().expect("app data dir resolves")
}

/// Public test hook: open a Db at the given path. Used only by
/// integration tests that need to stand up the schema outside the
/// normal Tauri setup() flow.
#[doc(hidden)]
pub async fn smoke_open_db(db_path: &std::path::Path) -> Result<(), db::DbError> {
    db::Db::open(db_path).await.map(|_| ())
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    let runtime = tokio::runtime::Runtime::new().expect("tokio runtime");

    tauri::Builder::default()
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_shell::init())
        .invoke_handler(tauri::generate_handler![
            commands::library::get_library_stats,
            commands::library::list_tracks,
            commands::library::list_albums,
            commands::library::list_artists,
            commands::library::tracks_for_album,
            commands::library::get_distinct,
            commands::library::pick_and_add_track,
            commands::library::verify_library,
            commands::library::remove_track,
            commands::library::trash_track,
            commands::library::show_in_files,
            commands::playback::play_track,
            commands::playback::pause,
            commands::playback::resume,
            commands::playback::stop,
            commands::playback::seek,
            commands::playback::set_volume,
            commands::audio::list_audio_devices,
            commands::audio::set_audio_device,
            commands::sync::list_sync_sources,
            commands::sync::add_sync_source,
            commands::sync::run_sync_now,
            commands::preferences::get_library_root,
            commands::preferences::set_library_root,
            commands::preferences::get_organize_scheme,
            commands::preferences::set_organize_scheme,
            commands::preferences::get_keep_organized,
            commands::preferences::set_keep_organized,
            commands::preferences::reorganize_track,
        ])
        .setup(move |app| {
            let dir = data_dir(app);
            std::fs::create_dir_all(&dir).expect("create app data dir");
            let db_path = dir.join("tuxtunes.db");
            let handle = app.handle().clone();
            let state = runtime
                .block_on(AppState::new(&db_path, handle))
                .expect("AppState init");
            app.manage(state);

            let state_ref = app.state::<AppState>();

            // Mount the system tray. Failures are logged inside install()
            // — the app continues without a tray rather than crashing.
            integration::tray::install(app.handle());

            // Track-changed → tray "Now Playing" label sync. Listens on
            // the same Tauri event channel the frontend's PlaybackService
            // uses, so the tray and UI never disagree about what's
            // playing. The lookup goes through TrackRow so we have the
            // already-formatted artist string.
            {
                let app_for_listener = app.handle().clone();
                let db = Arc::clone(&state_ref.db);
                app.handle().listen("playback:track-changed", move |event| {
                    let app = app_for_listener.clone();
                    let db = Arc::clone(&db);
                    let payload: serde_json::Value =
                        serde_json::from_str(event.payload()).unwrap_or(serde_json::Value::Null);
                    let track_id = payload.get("track_id").and_then(|v| v.as_i64());
                    tauri::async_runtime::spawn(async move {
                        let row = match track_id {
                            Some(id) => match crate::db::tracks::get(&db.engine, id).await {
                                Ok(r) => Some(r),
                                Err(e) => {
                                    log::warn!("track-changed lookup failed for {id}: {e}");
                                    None
                                }
                            },
                            None => None,
                        };
                        let label = row.as_ref().map(integration::tray::track_label);
                        integration::tray::set_now_playing_label(&app, label.as_deref());

                        // Desktop notification — only when the window
                        // is unfocused, to avoid notifying a user
                        // who's actively looking at the player.
                        if let Some(row) = &row {
                            let focused = app
                                .get_webview_window("main")
                                .and_then(|w| w.is_focused().ok())
                                .unwrap_or(false);
                            if !focused {
                                if let Err(e) = integration::notify::show_track(row) {
                                    log::warn!("notification failed: {e}");
                                }
                            }
                        }
                    });
                });
            }

            // Restore persisted volume. Sending SetVolume tells mpv to set
            // the property; the property observer then fires a VolumeChanged
            // event (idempotent — same value won't persist twice). If no
            // preference exists, leave mpv at its boot default (100).
            {
                let db = std::sync::Arc::clone(&state_ref.db);
                let engine = std::sync::Arc::clone(&state_ref.engine);
                runtime.spawn(async move {
                    use crate::db::preferences::{self, KEY_VOLUME};
                    use crate::playback::EngineCommand;
                    match preferences::get::<i64>(&db.engine, KEY_VOLUME).await {
                        Ok(Some(v)) => {
                            let clamped = v.clamp(0, 100) as u8;
                            let _ = engine.send(EngineCommand::SetVolume { volume: clamped });
                        }
                        Ok(None) => {}
                        Err(e) => log::warn!("read persisted volume failed: {e}"),
                    }
                });
            }

            // Tracking consumer: reads TrackEnded + VolumeChanged events
            // from the engine thread and writes to the DB.
            if let Some(mut rx) = state_ref.engine.take_tracking_rx() {
                let db = std::sync::Arc::clone(&state_ref.db);
                runtime.spawn(async move {
                    use crate::db::{preferences, tracks};
                    use crate::playback::stats::{decide, CountDecision};
                    use crate::playback::PlaybackTracking;

                    while let Some(event) = rx.recv().await {
                        match event {
                            PlaybackTracking::TrackEnded {
                                track_id,
                                position_ms,
                                duration_ms,
                            } => match decide(position_ms, duration_ms) {
                                CountDecision::Play => {
                                    if let Err(e) =
                                        tracks::bump_play_count(&db.engine, track_id).await
                                    {
                                        log::warn!("bump play_count failed for {track_id}: {e}");
                                    }
                                }
                                CountDecision::Skip => {
                                    if let Err(e) =
                                        tracks::bump_skip_count(&db.engine, track_id).await
                                    {
                                        log::warn!("bump skip_count failed for {track_id}: {e}");
                                    }
                                }
                                CountDecision::None => {}
                            },
                            PlaybackTracking::VolumeChanged { volume } => {
                                if let Err(e) = preferences::set(
                                    &db.engine,
                                    preferences::KEY_VOLUME,
                                    &(volume as i64),
                                )
                                .await
                                {
                                    log::warn!("persist volume failed: {e}");
                                }
                            }
                        }
                    }
                });
            }

            Ok(())
        })
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
