//! Smart-playlist Tauri commands.
//!
//! Two surfaces: `evaluate_smart_rule` returns the matching tracks (used
//! when the user opens the playlist), and `preview_smart_rule` returns
//! the count (used by the editor's live "✓ N tracks match" badge).

use crate::db::smart::{self, SmartRule};
use crate::db::tracks::TrackRow;
use crate::runtime::AppState;

#[tauri::command]
pub async fn evaluate_smart_rule(
    state: tauri::State<'_, AppState>,
    rule: SmartRule,
) -> Result<Vec<TrackRow>, String> {
    smart::evaluate(&state.db.engine, &rule)
        .await
        .map_err(|e| e.to_string())
}

#[tauri::command]
pub async fn preview_smart_rule(
    state: tauri::State<'_, AppState>,
    rule: SmartRule,
) -> Result<i64, String> {
    smart::preview_count(&state.db.engine, &rule)
        .await
        .map_err(|e| e.to_string())
}
