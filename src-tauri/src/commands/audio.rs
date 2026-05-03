//! Audio-device Tauri commands.

use crate::db::preferences::{
    self, KEY_AUDIO_DEVICE, KEY_AUDIO_EXCLUSIVE, KEY_REPLAYGAIN_MODE, KEY_VOLUME,
};
use crate::playback::config::{PlaybackPrefs, ReplayGainMode};
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
    /// Optional ReplayGain mode. Frontend may omit this on a device-only
    /// change to preserve the existing setting; we then load it from
    /// the preferences table.
    #[serde(default)]
    pub replaygain_mode: Option<ReplayGainMode>,
}

/// Apply audio prefs and persist them. Called from settings-audio
/// (device dropdown, exclusive toggle, replaygain dropdown). Each call
/// writes all three keys so a partial change doesn't drop the rest.
#[tauri::command]
pub async fn set_audio_device(
    state: tauri::State<'_, AppState>,
    args: SetAudioDeviceArgs,
) -> Result<(), String> {
    let engine_db = &state.db.engine;

    // Resolve ReplayGain mode: the arg if present, otherwise the
    // currently-stored value, otherwise the default (Off).
    let replaygain_mode = match args.replaygain_mode {
        Some(m) => m,
        None => preferences::get::<ReplayGainMode>(engine_db, KEY_REPLAYGAIN_MODE)
            .await
            .ok()
            .flatten()
            .unwrap_or(ReplayGainMode::Off),
    };

    // Volume comes from its own pref so the engine uses the user's
    // current loudness rather than hard-coding 100 here.
    let volume = preferences::get::<i64>(engine_db, KEY_VOLUME)
        .await
        .ok()
        .flatten()
        .map(|v| v.clamp(0, 100) as u8)
        .unwrap_or(100);

    let prefs = PlaybackPrefs {
        selected_device_id: Some(args.device_id.clone()),
        exclusive_mode: args.exclusive,
        replaygain_mode,
        volume,
    };

    state
        .engine
        .send(EngineCommand::ApplyDevice { prefs })
        .map_err(|e| e.to_string())?;

    // Persist on success — order doesn't matter since the engine has
    // already accepted the new prefs.
    preferences::set(engine_db, KEY_AUDIO_DEVICE, &args.device_id)
        .await
        .map_err(|e| e.to_string())?;
    preferences::set(engine_db, KEY_AUDIO_EXCLUSIVE, &args.exclusive)
        .await
        .map_err(|e| e.to_string())?;
    preferences::set(engine_db, KEY_REPLAYGAIN_MODE, &replaygain_mode)
        .await
        .map_err(|e| e.to_string())?;

    Ok(())
}

/// Read back the currently-persisted audio prefs so the settings UI
/// can hydrate its controls without a separate per-key call.
#[derive(Debug, serde::Serialize)]
pub struct AudioPrefsSnapshot {
    pub device_id: Option<String>,
    pub exclusive: bool,
    pub replaygain_mode: ReplayGainMode,
}

#[tauri::command]
pub async fn get_audio_prefs(
    state: tauri::State<'_, AppState>,
) -> Result<AudioPrefsSnapshot, String> {
    let engine_db = &state.db.engine;
    let device_id = preferences::get::<String>(engine_db, KEY_AUDIO_DEVICE)
        .await
        .map_err(|e| e.to_string())?;
    let exclusive = preferences::get::<bool>(engine_db, KEY_AUDIO_EXCLUSIVE)
        .await
        .map_err(|e| e.to_string())?
        .unwrap_or(false);
    let replaygain_mode = preferences::get::<ReplayGainMode>(engine_db, KEY_REPLAYGAIN_MODE)
        .await
        .map_err(|e| e.to_string())?
        .unwrap_or(ReplayGainMode::Off);
    Ok(AudioPrefsSnapshot {
        device_id,
        exclusive,
        replaygain_mode,
    })
}
