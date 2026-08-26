//! Playback Tauri commands.

use crate::db::tracks;
use crate::playback::config::TrackAudioFormat;
use crate::playback::{EngineCommand, EngineError};
use crate::runtime::AppState;

fn to_string_err<E: std::fmt::Display>(e: E) -> String {
    e.to_string()
}

/// Load and start playback of the given track id.
#[tauri::command]
pub async fn play_track(state: tauri::State<'_, AppState>, track_id: i64) -> Result<(), String> {
    let track = tracks::get(&state.db.engine, track_id)
        .await
        .map_err(to_string_err)?;

    // mpv accepts a nonexistent path at `loadfile` time and only fails
    // asynchronously, which the UI never hears about. Check up front so
    // the click gets an immediate, specific error and the row is
    // flagged for the track list.
    if !std::path::Path::new(&track.file_path).is_file() {
        if let Err(e) = tracks::mark_missing_source(&state.db.engine, track_id).await {
            log::warn!("mark_missing_source for {track_id} failed: {e}");
        }
        return Err(format!("File not found: {}", track.file_path));
    }

    // Must be the persisted prefs, not defaults: `build_properties`
    // re-applies audio-device / audio-exclusive / replaygain before every
    // loadfile, so defaults here would clobber what ApplyDevice set.
    let prefs = crate::commands::audio::load_playback_prefs(&state.db.engine).await;

    let fmt = TrackAudioFormat {
        sample_rate: track.sample_rate.map(|r| r as u32),
        bit_depth: track.bit_depth.map(|b| b as u8),
        is_dsd: track
            .kind
            .as_deref()
            .map(|k| k.eq_ignore_ascii_case("DSD"))
            .unwrap_or(false),
    };

    state
        .engine
        .send(EngineCommand::LoadAndPlay {
            track_id,
            file_path: track.file_path,
            prefs,
            fmt,
        })
        .map_err(|e: EngineError| e.to_string())
}

#[tauri::command]
pub async fn pause(state: tauri::State<'_, AppState>) -> Result<(), String> {
    state
        .engine
        .send(EngineCommand::Pause)
        .map_err(to_string_err)
}

#[tauri::command]
pub async fn resume(state: tauri::State<'_, AppState>) -> Result<(), String> {
    state
        .engine
        .send(EngineCommand::Resume)
        .map_err(to_string_err)
}

#[tauri::command]
pub async fn stop(state: tauri::State<'_, AppState>) -> Result<(), String> {
    state
        .engine
        .send(EngineCommand::Stop)
        .map_err(to_string_err)
}

#[tauri::command]
pub async fn seek(state: tauri::State<'_, AppState>, position_ms: i64) -> Result<(), String> {
    state
        .engine
        .send(EngineCommand::Seek { position_ms })
        .map_err(to_string_err)
}

#[tauri::command]
pub async fn set_volume(state: tauri::State<'_, AppState>, volume: u8) -> Result<(), String> {
    state
        .engine
        .send(EngineCommand::SetVolume { volume })
        .map_err(to_string_err)
}
