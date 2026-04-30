//! Library-scoped Tauri commands.

use crate::db::tracks::{self, TrackRow};
use crate::library::ingest;
use crate::runtime::AppState;
use serde::Serialize;

#[derive(Debug, Serialize, Clone, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub struct LibraryStats {
    pub track_count: i64,
    pub total_duration_ms: i64,
    pub total_size_bytes: i64,
}

#[tauri::command]
pub async fn get_library_stats(state: tauri::State<'_, AppState>) -> Result<LibraryStats, String> {
    let engine = &state.db.engine;

    let track_count: i64 = engine
        .raw_sql_scalar("SELECT COUNT(*) FROM tracks", &[])
        .await
        .map_err(|e| e.to_string())?;

    let total_duration_ms: i64 = engine
        .raw_sql_scalar("SELECT COALESCE(SUM(duration_ms), 0) FROM tracks", &[])
        .await
        .map_err(|e| e.to_string())?;

    let total_size_bytes: i64 = engine
        .raw_sql_scalar("SELECT COALESCE(SUM(size_bytes), 0) FROM tracks", &[])
        .await
        .map_err(|e| e.to_string())?;

    Ok(LibraryStats {
        track_count,
        total_duration_ms,
        total_size_bytes,
    })
}

#[tauri::command]
pub async fn list_tracks(
    state: tauri::State<'_, AppState>,
    limit: i64,
    offset: i64,
) -> Result<Vec<TrackRow>, String> {
    tracks::list(&state.db.engine, limit, offset)
        .await
        .map_err(|e| e.to_string())
}

#[tauri::command]
pub async fn pick_and_add_track(
    app: tauri::AppHandle,
    state: tauri::State<'_, AppState>,
) -> Result<Option<TrackRow>, String> {
    use tauri_plugin_dialog::DialogExt;

    let file_opt = app
        .dialog()
        .file()
        .add_filter(
            "Audio",
            &[
                "flac", "mp3", "m4a", "wav", "ogg", "opus", "aiff", "dsf", "dff",
            ],
        )
        .blocking_pick_file();

    let Some(path_resp) = file_opt else {
        return Ok(None);
    };
    let path_buf = path_resp.into_path().map_err(|e| e.to_string())?;

    let id = ingest::probe_and_add(&state.db.engine, &path_buf)
        .await
        .map_err(|e| e.to_string())?;

    let row = tracks::get(&state.db.engine, id)
        .await
        .map_err(|e| e.to_string())?;
    Ok(Some(row))
}

#[cfg(test)]
mod tests {
    use crate::db::Db;

    #[tokio::test]
    async fn library_stats_zero_on_fresh_db() {
        let tmp = tempfile::NamedTempFile::new().unwrap();
        let db = Db::open(tmp.path()).await.unwrap();
        let engine = &db.engine;

        let track_count: i64 = engine
            .raw_sql_scalar("SELECT COUNT(*) FROM tracks", &[])
            .await
            .unwrap();
        let total_duration_ms: i64 = engine
            .raw_sql_scalar("SELECT COALESCE(SUM(duration_ms), 0) FROM tracks", &[])
            .await
            .unwrap();
        let total_size_bytes: i64 = engine
            .raw_sql_scalar("SELECT COALESCE(SUM(size_bytes), 0) FROM tracks", &[])
            .await
            .unwrap();

        assert_eq!(
            (track_count, total_duration_ms, total_size_bytes),
            (0, 0, 0),
        );
    }
}
