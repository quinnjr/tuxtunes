//! Audio-device Tauri commands.

use crate::playback::config::PlaybackPrefs;
use crate::playback::device::AudioDevice;
use crate::playback::EngineCommand;
use crate::runtime::AppState;

#[tauri::command]
pub async fn list_audio_devices(
    state: tauri::State<'_, AppState>,
) -> Result<Vec<AudioDevice>, String> {
    Ok(state.engine.devices_snapshot())
}

#[derive(Debug, serde::Deserialize)]
pub struct SetAudioDeviceArgs {
    pub device_id: String,
    pub exclusive: bool,
}

#[tauri::command]
pub async fn set_audio_device(
    state: tauri::State<'_, AppState>,
    args: SetAudioDeviceArgs,
) -> Result<(), String> {
    let prefs = PlaybackPrefs {
        selected_device_id: Some(args.device_id),
        exclusive_mode: args.exclusive,
        ..Default::default()
    };
    state
        .engine
        .send(EngineCommand::ApplyDevice { prefs })
        .map_err(|e| e.to_string())
}
