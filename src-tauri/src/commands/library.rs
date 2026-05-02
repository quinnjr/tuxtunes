//! Library-scoped Tauri commands.

use crate::db::albums::{self, AlbumSummary, ArtistSummary};
use crate::db::tracks::{self, TrackRow};
use crate::library::ingest;
use crate::runtime::AppState;
use prax_query::filter::FilterValue;
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
    search: Option<String>,
) -> Result<Vec<TrackRow>, String> {
    tracks::list(&state.db.engine, limit, offset, search.as_deref())
        .await
        .map_err(|e| e.to_string())
}

#[tauri::command]
pub async fn list_albums(state: tauri::State<'_, AppState>) -> Result<Vec<AlbumSummary>, String> {
    albums::list_albums(&state.db.engine)
        .await
        .map_err(|e| e.to_string())
}

#[tauri::command]
pub async fn list_artists(state: tauri::State<'_, AppState>) -> Result<Vec<ArtistSummary>, String> {
    albums::list_artists(&state.db.engine)
        .await
        .map_err(|e| e.to_string())
}

#[tauri::command]
pub async fn tracks_for_album(
    state: tauri::State<'_, AppState>,
    album_artist: String,
    album: String,
) -> Result<Vec<TrackRow>, String> {
    albums::tracks_for_album(&state.db.engine, &album_artist, &album)
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

#[tauri::command]
pub async fn verify_library(
    state: tauri::State<'_, AppState>,
    app: tauri::AppHandle,
) -> Result<(), String> {
    let engine = std::sync::Arc::clone(&state.db.engine);
    tokio::spawn(async move {
        let _ = crate::fs::verify::verify_all(&engine, &app).await;
    });
    Ok(())
}

#[tauri::command]
pub async fn remove_track(state: tauri::State<'_, AppState>, track_id: i64) -> Result<(), String> {
    let sql = "DELETE FROM tracks WHERE id = ?";
    state
        .db
        .engine
        .raw_sql_execute(sql, &[FilterValue::Int(track_id)])
        .await
        .map(|_| ())
        .map_err(|e| e.to_string())
}

#[tauri::command]
pub async fn trash_track(state: tauri::State<'_, AppState>, track_id: i64) -> Result<(), String> {
    let row = crate::db::tracks::get(&state.db.engine, track_id)
        .await
        .map_err(|e| e.to_string())?;
    // Best-effort: send to trash. Already-missing files shouldn't block
    // the DB cleanup.
    if std::path::Path::new(&row.file_path).exists() {
        trash::delete(&row.file_path).map_err(|e| e.to_string())?;
    }
    remove_track(state, track_id).await
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
