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

    let row = engine
        .raw_sql_first(
            "SELECT COUNT(*) AS track_count, \
                    COALESCE(SUM(duration_ms), 0) AS total_duration_ms, \
                    COALESCE(SUM(size_bytes), 0) AS total_size_bytes \
             FROM tracks",
            &[],
        )
        .await
        .map_err(|e| e.to_string())?;

    let v = row.into_json();
    Ok(LibraryStats {
        track_count: v["track_count"].as_i64().unwrap_or(0),
        total_duration_ms: v["total_duration_ms"].as_i64().unwrap_or(0),
        total_size_bytes: v["total_size_bytes"].as_i64().unwrap_or(0),
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

/// Shared body for the artwork commands: probe the album's first few
/// files for an embedded picture or a sidecar image, copy the hit into
/// `$APPDATA/artwork/`, stamp `artwork_path` on the album's tracks,
/// and return the cached path — or None when nothing was found. Cheap
/// to call repeatedly: the cache is content-addressed and the DB write
/// is a no-op once stamped.
async fn resolve_artwork_for_album(
    app: &tauri::AppHandle,
    engine: &prax_sqlite::raw::SqliteRawEngine,
    album_artist: &str,
    album: &str,
) -> Result<Option<String>, String> {
    use tauri::Manager;
    let cache_dir = app
        .path()
        .app_data_dir()
        .map_err(|e| e.to_string())?
        .join("artwork");
    let tracks = albums::tracks_for_album(engine, album_artist, album)
        .await
        .map_err(|e| e.to_string())?;
    // Already resolved for this album (by another track's lookup)? Only
    // trust a path that lives under our own artwork cache: the asset
    // protocol scope is pinned to `$APPDATA/artwork/**`, so a stale
    // path pointing at a managed-library sidecar (e.g. a `cover.jpg`
    // written by fs/artwork.rs, outside that scope) would 403 in the
    // webview. Anything else falls through to `resolve_for_files`,
    // which re-derives and copies the art into the cache.
    if let Some(existing) = tracks.iter().find_map(|t| t.artwork_path.clone()) {
        let existing_path = std::path::Path::new(&existing);
        if existing_path.is_file() && existing_path.starts_with(&cache_dir) {
            return Ok(Some(existing));
        }
    }
    let paths: Vec<std::path::PathBuf> = tracks
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
    albums::set_album_artwork(engine, album_artist, album, &path_str)
        .await
        .map_err(|e| e.to_string())?;
    Ok(Some(path_str))
}

/// Find (and cache) cover art for an album on demand (album grid).
#[tauri::command]
pub async fn resolve_album_artwork(
    app: tauri::AppHandle,
    state: tauri::State<'_, AppState>,
    album_artist: String,
    album: String,
) -> Result<Option<String>, String> {
    resolve_artwork_for_album(&app, &state.db.engine, &album_artist, &album).await
}

/// Find (and cache) cover art for the album a track belongs to
/// (transport bar / Now Playing). Uses the same grouping as
/// `list_albums` so the grid and the player share one cached image.
#[tauri::command]
pub async fn resolve_track_artwork(
    app: tauri::AppHandle,
    state: tauri::State<'_, AppState>,
    track_id: i64,
) -> Result<Option<String>, String> {
    let engine = &state.db.engine;
    let row = engine
        .raw_sql_optional(
            "SELECT COALESCE(NULLIF(album_artist, ''), NULLIF(artist, ''), 'Unknown Artist') \
                    AS album_artist, \
                    COALESCE(NULLIF(album, ''), 'Unknown Album') AS album \
             FROM tracks WHERE id = ?",
            &[FilterValue::Int(track_id)],
        )
        .await
        .map_err(|e| e.to_string())?
        .ok_or_else(|| format!("track {track_id} not found"))?;
    let v = row.into_json();
    let album_artist = v["album_artist"]
        .as_str()
        .unwrap_or("Unknown Artist")
        .to_string();
    let album = v["album"].as_str().unwrap_or("Unknown Album").to_string();
    resolve_artwork_for_album(&app, engine, &album_artist, &album).await
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

/// Pick a folder and add every audio file under it (recursively) that
/// the library doesn't already reference. Returns counts; the UI
/// refreshes its lists afterwards.
#[tauri::command]
pub async fn pick_and_add_folder(
    app: tauri::AppHandle,
    state: tauri::State<'_, AppState>,
) -> Result<Option<ingest::AddFolderSummary>, String> {
    use tauri_plugin_dialog::DialogExt;

    let Some(folder) = app.dialog().file().blocking_pick_folder() else {
        return Ok(None);
    };
    let dir = folder.into_path().map_err(|e| e.to_string())?;
    ingest::add_folder(&state.db.engine, &dir)
        .await
        .map(Some)
        .map_err(|e| e.to_string())
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

/// Owned mirror of `db::tracks::MetadataEdit` for the IPC boundary.
#[derive(Debug, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TrackMetadataPatch {
    pub title: String,
    pub artist: Option<String>,
    pub album: Option<String>,
    pub album_artist: Option<String>,
    pub genre: Option<String>,
    pub year: Option<i64>,
    pub track_number: Option<i64>,
    pub disc_number: Option<i64>,
}

/// Edit a track's descriptive metadata. The file's own tags are
/// written first — an edit that cannot reach the file fails whole, so
/// the DB never claims metadata the file doesn't carry. The DB row is
/// then updated and flagged `user_edited` so a sync won't revert it.
#[tauri::command]
pub async fn update_track_metadata(
    state: tauri::State<'_, AppState>,
    track_id: i64,
    edit: TrackMetadataPatch,
) -> Result<(), String> {
    let row = tracks::get(&state.db.engine, track_id)
        .await
        .map_err(|e| e.to_string())?;
    let e = crate::db::tracks::MetadataEdit {
        title: &edit.title,
        artist: edit.artist.as_deref(),
        album: edit.album.as_deref(),
        album_artist: edit.album_artist.as_deref(),
        genre: edit.genre.as_deref(),
        year: edit.year,
        track_number: edit.track_number,
        disc_number: edit.disc_number,
    };
    crate::fs::tags::write_metadata(std::path::Path::new(&row.file_path), &e)
        .map_err(|err| err.to_string())?;
    crate::db::tracks::update_metadata(&state.db.engine, track_id, &e)
        .await
        .map_err(|err| err.to_string())
}

#[tauri::command]
pub async fn remove_track(state: tauri::State<'_, AppState>, track_id: i64) -> Result<(), String> {
    let sql = "DELETE FROM tracks WHERE id = ?";
    state
        .db
        .engine
        .raw_sql_execute(sql, &[FilterValue::Int(track_id)])
        .await
        .map_err(|e| e.to_string())?;
    // Leave no dangling playlist entry behind — SQLite reuses rowids,
    // so a stale id could later resolve to an unrelated track.
    crate::db::playlists::prune_track(&state.db.engine, track_id)
        .await
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
    // Tests exercise this command against temp paths; popping a file
    // manager window on the developer's desktop is never wanted there.
    if std::env::var_os("TUXTUNES_NO_XDG_OPEN").is_some() {
        log::info!("TUXTUNES_NO_XDG_OPEN set; not opening {}", parent.display());
        return Ok(());
    }
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

        let row = engine
            .raw_sql_first(
                "SELECT COUNT(*) AS track_count, \
                        COALESCE(SUM(duration_ms), 0) AS total_duration_ms, \
                        COALESCE(SUM(size_bytes), 0) AS total_size_bytes \
                 FROM tracks",
                &[],
            )
            .await
            .unwrap();
        let v = row.into_json();

        assert_eq!(
            (
                v["track_count"].as_i64().unwrap(),
                v["total_duration_ms"].as_i64().unwrap(),
                v["total_size_bytes"].as_i64().unwrap(),
            ),
            (0, 0, 0),
        );
    }
}
