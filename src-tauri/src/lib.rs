mod commands;
pub mod db;
pub mod library;
pub mod playback;
mod runtime;

use runtime::AppState;
use std::path::PathBuf;
use tauri::Manager;

fn data_dir(app: &tauri::App) -> PathBuf {
    app.path().app_data_dir().expect("app data dir resolves")
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
            commands::library::pick_and_add_track,
            commands::playback::play_track,
            commands::playback::pause,
            commands::playback::resume,
            commands::playback::stop,
            commands::playback::seek,
            commands::playback::set_volume,
            commands::audio::list_audio_devices,
            commands::audio::set_audio_device,
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

            // Spawn the play-count consumer: reads TrackEnded events from the
            // engine thread and writes play/skip counts to the DB via stats::decide.
            let state_ref: tauri::State<'_, AppState> = app.state::<AppState>();
            if let Some(mut rx) = state_ref.engine.take_tracking_rx() {
                let db = std::sync::Arc::clone(&state_ref.db);
                runtime.spawn(async move {
                    use crate::db::tracks;
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
                        }
                    }
                });
            }

            Ok(())
        })
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
