//! Library-scoped Tauri commands.

use crate::db::albums::{self, AlbumSummary, ArtistSummary};
use crate::db::distinct::{self, DistinctValue, TrackFilters};
use crate::db::tracks::{self, TrackRow, TrackSort};
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
    filters: Option<TrackFilters>,
    sort: Option<TrackSort>,
) -> Result<Vec<TrackRow>, String> {
    let f = filters.unwrap_or_default();
    tracks::list(&state.db.engine, limit, offset, &f, sort.as_ref())
        .await
        .map_err(|e| e.to_string())
}

#[tauri::command]
pub async fn get_distinct(
    state: tauri::State<'_, AppState>,
    column: String,
    filters: Option<TrackFilters>,
) -> Result<Vec<DistinctValue>, String> {
    let f = filters.unwrap_or_default();
    distinct::get_distinct(&state.db.engine, &column, &f)
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

/// Find (and cache) cover art for an album on demand. Probes the
/// album's first few files for an embedded picture or a sidecar image,
/// copies the hit into `$APPDATA/artwork/`, stamps `artwork_path` on
/// the album's tracks, and returns the cached path — or None when the
/// album has no discoverable art. Cheap to call repeatedly: the cache
/// is content-addressed and the DB write is a no-op once stamped.
#[tauri::command]
pub async fn resolve_album_artwork(
    app: tauri::AppHandle,
    state: tauri::State<'_, AppState>,
    album_artist: String,
    album: String,
) -> Result<Option<String>, String> {
    use tauri::Manager;
    let cache_dir = app
        .path()
        .app_data_dir()
        .map_err(|e| e.to_string())?
        .join("artwork");
    let engine = &state.db.engine;
    let paths: Vec<std::path::PathBuf> = albums::tracks_for_album(engine, &album_artist, &album)
        .await
        .map_err(|e| e.to_string())?
        .into_iter()
        .map(|t| std::path::PathBuf::from(t.file_path))
        .collect();
    let found = tokio::task::spawn_blocking(move || {
        crate::library::artwork::resolve_for_files(&cache_dir, &paths)
    })
    .await
    .map_err(|e| e.to_string())?
    .map_err(|e| e.to_string())?;
    let Some(path) = found else {
        return Ok(None);
    };
    let path_str = path.to_string_lossy().into_owned();
    albums::set_album_artwork(engine, &album_artist, &album, &path_str)
        .await
        .map_err(|e| e.to_string())?;
    Ok(Some(path_str))
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

/// Runs the verify walk and reports failures on the `fs:verify-failed`
/// channel. Runtime-generic (rather than pinned to `tauri::Wry`) so it
/// can be exercised directly under `tauri::test::mock_app()` — the
/// `verify_library` command itself can't be, since the `#[tauri::command]`
/// macro binds its `AppHandle` parameter to the real Wry runtime.
pub async fn run_verify_and_report<R: tauri::Runtime>(
    engine: &std::sync::Arc<prax_sqlite::raw::SqliteRawEngine>,
    app: &tauri::AppHandle<R>,
) {
    if let Err(e) = crate::fs::verify::verify_all(engine, app).await {
        log::warn!("verify_library failed: {e}");
        if let Err(emit_err) = tauri::Emitter::emit(
            app,
            crate::fs::events::VERIFY_FAILED,
            crate::fs::events::VerifyFailed {
                message: e.to_string(),
            },
        ) {
            log::warn!("failed to notify frontend of verify failure: {emit_err}");
        }
    }
}

#[tauri::command]
pub async fn verify_library(
    state: tauri::State<'_, AppState>,
    app: tauri::AppHandle,
) -> Result<(), String> {
    let engine = std::sync::Arc::clone(&state.db.engine);
    tokio::spawn(async move {
        run_verify_and_report(&engine, &app).await;
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

/// Reveal the track's containing folder in the user's file manager
/// via `xdg-open`. The crate is Linux-only (see CLAUDE.md / design doc
/// non-goals), so xdg-open is the standard cross-DE entry point —
/// `tauri-plugin-shell::Shell::open` is deprecated.
#[tauri::command]
pub async fn show_in_files(state: tauri::State<'_, AppState>, track_id: i64) -> Result<(), String> {
    let row = tracks::get(&state.db.engine, track_id)
        .await
        .map_err(|e| e.to_string())?;
    let parent = std::path::Path::new(&row.file_path)
        .parent()
        .map(std::path::Path::to_path_buf)
        .ok_or_else(|| "no parent directory".to_string())?;
    std::process::Command::new("xdg-open")
        .arg(parent)
        .spawn()
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
