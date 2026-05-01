//! Tauri commands for managed-library preferences and rename triggers.

use crate::db::preferences;
use crate::runtime::AppState;

#[tauri::command]
pub async fn get_library_root(state: tauri::State<'_, AppState>) -> Result<String, String> {
    preferences::get_library_root(&state.db.engine)
        .await
        .map(|p| p.display().to_string())
        .map_err(|e| e.to_string())
}

#[tauri::command]
pub async fn set_library_root(
    state: tauri::State<'_, AppState>,
    path: String,
) -> Result<(), String> {
    preferences::set_library_root(&state.db.engine, std::path::Path::new(&path))
        .await
        .map_err(|e| e.to_string())
}

#[tauri::command]
pub async fn get_organize_scheme(state: tauri::State<'_, AppState>) -> Result<String, String> {
    preferences::get_organize_scheme(&state.db.engine)
        .await
        .map_err(|e| e.to_string())
}

#[tauri::command]
pub async fn set_organize_scheme(
    state: tauri::State<'_, AppState>,
    scheme: String,
) -> Result<(), String> {
    preferences::set_organize_scheme(&state.db.engine, &scheme)
        .await
        .map_err(|e| e.to_string())
}

#[tauri::command]
pub async fn get_keep_organized(state: tauri::State<'_, AppState>) -> Result<bool, String> {
    preferences::get_keep_organized(&state.db.engine)
        .await
        .map_err(|e| e.to_string())
}

#[tauri::command]
pub async fn set_keep_organized(
    state: tauri::State<'_, AppState>,
    keep: bool,
) -> Result<(), String> {
    preferences::set_keep_organized(&state.db.engine, keep)
        .await
        .map_err(|e| e.to_string())
}

#[tauri::command]
pub async fn reorganize_track(
    state: tauri::State<'_, AppState>,
    track_id: i64,
) -> Result<(), String> {
    state.fs.reorganize_track(track_id)
}
