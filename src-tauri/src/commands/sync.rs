//! Tauri commands for the ITL sync.

use crate::db::sync_sources::{self, SyncSourceRow};
use crate::runtime::AppState;
use crate::sync::conflict::ConflictRules;
use crate::sync::path_map::PathMapping;

#[tauri::command]
pub async fn list_sync_sources(
    state: tauri::State<'_, AppState>,
) -> Result<Vec<SyncSourceRow>, String> {
    sync_sources::list(&state.db.engine)
        .await
        .map_err(|e| e.to_string())
}

#[derive(Debug, serde::Deserialize)]
pub struct AddSyncSourceArgs {
    pub name: String,
    pub source_path: String,
    pub path_mappings: Vec<PathMapping>,
    pub conflict_rules: ConflictRules,
    pub auto_copy_files: bool,
}

#[tauri::command]
pub async fn add_sync_source(
    state: tauri::State<'_, AppState>,
    args: AddSyncSourceArgs,
) -> Result<i64, String> {
    sync_sources::insert(
        &state.db.engine,
        &args.name,
        &args.source_path,
        &args.path_mappings,
        &args.conflict_rules,
        args.auto_copy_files,
    )
    .await
    .map_err(|e| e.to_string())
}

#[tauri::command]
pub async fn run_sync_now(state: tauri::State<'_, AppState>, source_id: i64) -> Result<(), String> {
    state.sync.run_now(source_id)
}
