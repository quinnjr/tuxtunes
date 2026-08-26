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
pub async fn list_playlists(state: tauri::State<'_, AppState>) -> Result<Vec<PlaylistRow>, String> {
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

/// The editor loads an existing smart playlist's rule through this.
/// Ok(None) for regular playlists / folders / unknown ids.
#[tauri::command]
pub async fn get_smart_playlist_rule(
    state: tauri::State<'_, AppState>,
    playlist_id: i64,
) -> Result<Option<SmartRule>, String> {
    let json = playlists::get_smart_rule(&state.db.engine, playlist_id)
        .await
        .map_err(|e| e.to_string())?;
    match json {
        Some(j) => serde_json::from_str::<SmartRule>(&j)
            .map(Some)
            .map_err(|e| e.to_string()),
        None => Ok(None),
    }
}

#[tauri::command]
pub async fn rename_playlist(
    state: tauri::State<'_, AppState>,
    playlist_id: i64,
    name: String,
) -> Result<(), String> {
    let trimmed = name.trim();
    if trimmed.is_empty() {
        return Err("playlist name cannot be empty".to_string());
    }
    playlists::rename(&state.db.engine, playlist_id, trimmed)
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

/// Evaluate a smart playlist's rule against the current library and
/// refresh its cached count for the sidebar. Shared by
/// `open_smart_playlist` and the smart branch of `open_playlist`.
async fn evaluate_and_cache(
    engine: &prax_sqlite::raw::SqliteRawEngine,
    playlist_id: i64,
    rule_json: &str,
) -> Result<Vec<TrackRow>, String> {
    let rule: SmartRule = serde_json::from_str(rule_json).map_err(|e| e.to_string())?;
    let rows = smart::evaluate(engine, &rule)
        .await
        .map_err(|e| e.to_string())?;
    if let Err(e) = playlists::set_cached_count(engine, playlist_id, rows.len() as i64).await {
        log::warn!("set_cached_count for {playlist_id} failed: {e}");
    }
    Ok(rows)
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
    evaluate_and_cache(&state.db.engine, playlist_id, &rule_json).await
}

/// Open any playlist by id and return its tracks: smart playlists are
/// evaluated live, regular playlists resolve their stored ordered
/// entries, folders yield nothing. Refreshes the sidebar's cached
/// count either way.
#[tauri::command]
pub async fn open_playlist(
    state: tauri::State<'_, AppState>,
    playlist_id: i64,
) -> Result<Vec<TrackRow>, String> {
    let engine = &state.db.engine;
    let rule_json = playlists::get_smart_rule(engine, playlist_id)
        .await
        .map_err(|e| e.to_string())?;
    match rule_json {
        Some(json) => evaluate_and_cache(engine, playlist_id, &json).await,
        None => {
            let rows = playlists::tracks_for_regular(engine, playlist_id)
                .await
                .map_err(|e| e.to_string())?;
            if let Err(e) =
                playlists::set_cached_count(engine, playlist_id, rows.len() as i64).await
            {
                log::warn!("set_cached_count for {playlist_id} failed: {e}");
            }
            Ok(rows)
        }
    }
}
