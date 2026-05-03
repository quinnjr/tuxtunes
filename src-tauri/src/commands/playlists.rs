//! Playlist Tauri commands.
//!
//! User-side smart-playlist CRUD. Synced playlists are managed by the
//! sync layer (see `db::playlists::upsert` / the sync coordinator);
//! these commands handle the user's own creations.

use crate::db::playlists::{self, PlaylistRow};
use crate::db::smart::{self, SmartRule};
use crate::db::tracks::TrackRow;
use crate::runtime::AppState;

#[tauri::command]
pub async fn list_playlists(
    state: tauri::State<'_, AppState>,
) -> Result<Vec<PlaylistRow>, String> {
    playlists::list_all(&state.db.engine)
        .await
        .map_err(|e| e.to_string())
}

#[tauri::command]
pub async fn create_smart_playlist(
    state: tauri::State<'_, AppState>,
    name: String,
    rule: SmartRule,
) -> Result<i64, String> {
    // Round-trip the rule through serde so a malformed value gets a
    // 4xx-shaped error before we touch the DB. The DB layer takes a
    // string so it stays decoupled from the rule shape.
    let rule_json = serde_json::to_string(&rule).map_err(|e| e.to_string())?;
    playlists::create_smart(&state.db.engine, &name, &rule_json)
        .await
        .map_err(|e| e.to_string())
}

#[tauri::command]
pub async fn update_smart_playlist(
    state: tauri::State<'_, AppState>,
    playlist_id: i64,
    rule: SmartRule,
) -> Result<(), String> {
    let rule_json = serde_json::to_string(&rule).map_err(|e| e.to_string())?;
    playlists::update_smart_rule(&state.db.engine, playlist_id, &rule_json)
        .await
        .map_err(|e| e.to_string())
}

#[tauri::command]
pub async fn delete_playlist(
    state: tauri::State<'_, AppState>,
    playlist_id: i64,
) -> Result<(), String> {
    playlists::delete(&state.db.engine, playlist_id)
        .await
        .map_err(|e| e.to_string())
}

/// Open a smart playlist: load its rule, evaluate it against the
/// current library, refresh the cached count for the sidebar, return
/// the matching tracks.
#[tauri::command]
pub async fn open_smart_playlist(
    state: tauri::State<'_, AppState>,
    playlist_id: i64,
) -> Result<Vec<TrackRow>, String> {
    let rule_json = playlists::get_smart_rule(&state.db.engine, playlist_id)
        .await
        .map_err(|e| e.to_string())?
        .ok_or_else(|| format!("playlist {playlist_id} is not a smart playlist"))?;
    let rule: SmartRule = serde_json::from_str(&rule_json).map_err(|e| e.to_string())?;
    let rows = smart::evaluate(&state.db.engine, &rule)
        .await
        .map_err(|e| e.to_string())?;
    if let Err(e) =
        playlists::set_cached_count(&state.db.engine, playlist_id, rows.len() as i64).await
    {
        log::warn!("set_cached_count for {playlist_id} failed: {e}");
    }
    Ok(rows)
}
