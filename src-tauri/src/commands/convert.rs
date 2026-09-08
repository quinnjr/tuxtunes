//! File-conversion Tauri commands.

use crate::db::preferences::{self, KEY_CONVERT_PREFS};
use crate::fs::convert::{ConvertFormat, ConvertPrefs};
use crate::runtime::AppState;

/// Whether conversion is usable at all. The settings tab greys itself
/// out on `false` rather than letting every conversion fail one by one
/// with the same "ffmpeg not found".
#[tauri::command]
pub async fn convert_available() -> Result<bool, String> {
    tokio::task::spawn_blocking(crate::fs::convert::ffmpeg_available)
        .await
        .map_err(|e| e.to_string())
}

#[tauri::command]
pub async fn get_convert_prefs(state: tauri::State<'_, AppState>) -> Result<ConvertPrefs, String> {
    Ok(
        preferences::get::<ConvertPrefs>(&state.db.engine, KEY_CONVERT_PREFS)
            .await
            .map_err(|e| e.to_string())?
            .unwrap_or_default()
            .sanitized(),
    )
}

#[tauri::command]
pub async fn set_convert_prefs(
    state: tauri::State<'_, AppState>,
    prefs: ConvertPrefs,
) -> Result<ConvertPrefs, String> {
    let prefs = prefs.sanitized();
    preferences::set(&state.db.engine, KEY_CONVERT_PREFS, &prefs)
        .await
        .map_err(|e| e.to_string())?;
    // Hand the clamped values back so the UI shows what was actually
    // stored rather than what it optimistically sent.
    Ok(prefs)
}

#[derive(Debug, serde::Deserialize)]
pub struct ConvertTracksArgs {
    pub track_ids: Vec<i64>,
    pub format: ConvertFormat,
    /// One-off overrides for this batch. Omitted means "use the saved
    /// preferences", which is what the context menu sends.
    #[serde(default)]
    pub prefs: Option<ConvertPrefs>,
}

/// Queue a conversion batch. Returns as soon as it is queued; the
/// outcome arrives on the `fs:convert-*` events.
#[tauri::command]
pub async fn convert_tracks(
    state: tauri::State<'_, AppState>,
    args: ConvertTracksArgs,
) -> Result<(), String> {
    if args.track_ids.is_empty() {
        return Err("no tracks selected".into());
    }
    let prefs = match args.prefs {
        Some(p) => p.sanitized(),
        None => preferences::get::<ConvertPrefs>(&state.db.engine, KEY_CONVERT_PREFS)
            .await
            .map_err(|e| e.to_string())?
            .unwrap_or_default()
            .sanitized(),
    };
    state.fs.convert_tracks(args.track_ids, args.format, prefs)
}
