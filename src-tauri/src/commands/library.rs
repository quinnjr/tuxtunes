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

/// Hand a freshly added track to the copy-on-add worker, which copies
/// it under the library root and rewrites `file_path` when it lands.
/// The row the command returns still points at the source; the UI
/// picks up the managed path from `fs:ingest-complete`.
///
/// A dead worker is logged rather than failed on — the track is in the
/// library and playable from where it is either way.
fn queue_copy(state: &AppState, track_id: i64, source: std::path::PathBuf) {
    if let Err(e) = state.fs.copy_for_track(track_id, source) {
        log::warn!("could not queue copy-on-add for track {track_id}: {e}");
    }
}

/// Outcome of [`pick_and_add_track`]; serialized for the UI.
///
/// Shaped like [`ingest::AddFolderSummary`] so both add paths can say
/// what actually happened: a bare list of rows cannot express "picked
/// 20, added 17, 2 were already here, 1 could not be read", which left
/// a selection of unreadable files looking like the app had done
/// nothing at all.
#[derive(Debug, Default, Clone, serde::Serialize)]
pub struct AddTracksSummary {
    /// Rows for files that were not in the library before.
    pub added: Vec<TrackRow>,
    /// Files that were already here; their rows are untouched.
    pub existing: u64,
    /// Names of files that could not be read, so the UI can say which.
    pub failed: Vec<String>,
}

/// Pick one or more audio files and add each to the library. Returns
/// `None` if the dialog was cancelled.
#[tauri::command]
pub async fn pick_and_add_track(
    app: tauri::AppHandle,
    state: tauri::State<'_, AppState>,
) -> Result<Option<AddTracksSummary>, String> {
    use tauri_plugin_dialog::DialogExt;

    let picked = app
        .dialog()
        .file()
        .add_filter(
            "Audio",
            &[
                "flac", "mp3", "m4a", "wav", "ogg", "opus", "aiff", "dsf", "dff",
            ],
        )
        .blocking_pick_files();

    let Some(paths) = picked else {
        return Ok(None);
    };

    let mut resolved = Vec::with_capacity(paths.len());
    for path_resp in paths {
        match path_resp.into_path() {
            Ok(p) => resolved.push(p),
            // A pick the portal cannot turn into a path (a remote URL)
            // is one bad file, not a reason to drop the selection —
            // and rows added before it are already committed.
            Err(e) => {
                log::warn!("pick_and_add_track: unusable pick: {e}");
                resolved.push(std::path::PathBuf::new());
            }
        }
    }
    Ok(Some(add_picked_files(&state, resolved).await))
}

/// Body of [`pick_and_add_track`] once the paths are known, free of the
/// file dialog so it can be exercised against a temp database. An empty
/// path stands for a pick that could not be resolved.
pub async fn add_picked_files(
    state: &AppState,
    paths: Vec<std::path::PathBuf>,
) -> AddTracksSummary {
    let mut summary = AddTracksSummary::default();
    for path in paths {
        let name = file_label(&path);
        if path.as_os_str().is_empty() {
            summary.failed.push(name);
            continue;
        }
        match add_one_picked_file(state, path).await {
            Ok(AddOutcome::Added(row)) => summary.added.push(*row),
            Ok(AddOutcome::Existing) => summary.existing += 1,
            Err(ingest::IngestError::Probe { path, source }) => {
                log::warn!("pick_and_add_track: skipping {path}: {source}");
                summary.failed.push(name);
            }
            // Matching add_folder's policy: a database error is not
            // per-file, so grinding through the rest would only produce
            // one doomed probe per file. Stop, and report what landed.
            Err(e) => {
                log::warn!(
                    "pick_and_add_track: stopping after {} files: {e}",
                    summary.added.len()
                );
                summary.failed.push(name);
                break;
            }
        }
    }
    summary
}

/// What adding one picked file did. `Added` boxes its row because a
/// `TrackRow` dwarfs the unit variant.
enum AddOutcome {
    Added(Box<TrackRow>),
    Existing,
}

fn file_label(path: &std::path::Path) -> String {
    path.file_name()
        .and_then(|n| n.to_str())
        .map(str::to_owned)
        .unwrap_or_else(|| path.display().to_string())
}

/// Add one picked file. Deduplication lives in
/// [`ingest::ensure_track`] (shared with the headless import); a file
/// the library already has is left alone.
async fn add_one_picked_file(
    state: &AppState,
    path_buf: std::path::PathBuf,
) -> Result<AddOutcome, ingest::IngestError> {
    let Some(id) = ingest::ensure_track(&state.db.engine, &path_buf).await? else {
        return Ok(AddOutcome::Existing);
    };

    queue_copy(state, id, path_buf);

    tracks::get(&state.db.engine, id)
        .await
        .map(|row| AddOutcome::Added(Box::new(row)))
        .map_err(|e| ingest::IngestError::Db(anyhow::Error::from(e)))
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
    let mut summary = ingest::add_folder(&state.db.engine, &dir)
        .await
        .map_err(|e| e.to_string())?;

    // `added_tracks` is `#[serde(skip)]`, so draining it here keeps it
    // out of the payload the UI sees.
    for (id, source) in std::mem::take(&mut summary.added_tracks) {
        queue_copy(&state, id, source);
    }

    Ok(Some(summary))
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
    // A cover TuxTunes resolved for this album lives only in its cache
    // until something puts it in the file; saving an edit is the
    // natural moment. Only when the file has none of its own — an edit
    // is no reason to replace art the file already carries.
    write_back_cover(&row);
    crate::db::tracks::update_metadata(&state.db.engine, track_id, &e)
        .await
        .map_err(|err| err.to_string())?;
    // The file's bytes changed, so the stored hash no longer describes
    // it and Verify would call it corrupt.
    crate::db::tracks::clear_file_hash(&state.db.engine, track_id)
        .await
        .map_err(|err| err.to_string())
}

/// Outcome of [`write_tags_to_files`]; serialized for the UI.
#[derive(Debug, Default, Clone, serde::Serialize)]
pub struct WriteBackSummary {
    /// Files whose tags were rewritten from the library's values.
    pub written: u64,
    /// Of those, how many also got a cover they did not have.
    pub covers: u64,
    /// Titles of tracks that could not be written, for the UI to name.
    pub failed: Vec<String>,
}

/// Push what the library knows about each track into its file's own
/// tags: the descriptive fields, plus the resolved cover for a file
/// that has no embedded picture.
///
/// Metadata corrected in TuxTunes — or carried in from an iTunes
/// import, which is full of edits that were never in the files — lives
/// only in this database until this writes it out. That makes the file
/// self-describing again: other players see it, and a re-import after
/// the library is gone brings the corrections back with it.
#[tauri::command]
pub async fn write_tags_to_files(
    state: tauri::State<'_, AppState>,
    track_ids: Vec<i64>,
) -> Result<WriteBackSummary, String> {
    Ok(write_tags_for(&state.db.engine, &track_ids).await)
}

/// Body of [`write_tags_to_files`], free of `tauri::State` so it can be
/// exercised directly against a temp database.
pub async fn write_tags_for(
    engine: &prax_sqlite::raw::SqliteRawEngine,
    track_ids: &[i64],
) -> WriteBackSummary {
    let mut summary = WriteBackSummary::default();
    for &id in track_ids {
        let row = match tracks::get(engine, id).await {
            Ok(row) => row,
            Err(e) => {
                log::warn!("write_tags_to_files: track {id} is gone: {e}");
                continue;
            }
        };
        let edit = crate::db::tracks::MetadataEdit {
            title: &row.title,
            artist: row.artist.as_deref(),
            album: row.album.as_deref(),
            album_artist: row.album_artist.as_deref(),
            genre: row.genre.as_deref(),
            year: row.year,
            track_number: row.track_number,
            disc_number: row.disc_number,
        };
        if let Err(e) = crate::fs::tags::write_metadata(std::path::Path::new(&row.file_path), &edit)
        {
            log::warn!("write_tags_to_files: {}: {e}", row.file_path);
            summary.failed.push(row.title.clone());
            continue;
        }
        if write_back_cover(&row) {
            summary.covers += 1;
        }
        summary.written += 1;
        if let Err(e) = crate::db::tracks::clear_file_hash(engine, id).await {
            log::warn!("write_tags_to_files: could not clear the hash for {id}: {e}");
        }
    }
    summary
}

/// Embed the row's cover in its file when the file has none. Returns
/// whether a cover was written. Best-effort: a file that will not take
/// a picture is not worth failing an otherwise good tag write over.
fn write_back_cover(row: &TrackRow) -> bool {
    let Some(art) = row.artwork_path.as_deref() else {
        return false;
    };
    let art = std::path::Path::new(art);
    let audio = std::path::Path::new(&row.file_path);
    if !art.is_file() || crate::fs::tags::has_embedded_cover(audio) {
        return false;
    }
    match crate::fs::tags::write_cover(audio, art) {
        Ok(()) => true,
        Err(e) => {
            log::warn!("could not embed a cover in {}: {e}", row.file_path);
            false
        }
    }
}

#[tauri::command]
pub async fn remove_track(state: tauri::State<'_, AppState>, track_id: i64) -> Result<(), String> {
    remove_track_from(&state.db.engine, track_id).await
}

/// Body of [`remove_track`], free of `tauri::State` so the bulk command
/// and tests can drive it directly.
///
/// Removing a track leaves nothing of it behind: the row carries every
/// correction the user made that the file itself never had, so it goes
/// whole rather than lingering to be re-adopted by a later import.
pub async fn remove_track_from(
    engine: &prax_sqlite::raw::SqliteRawEngine,
    track_id: i64,
) -> Result<(), String> {
    // Read the row first: once it is gone there is no way back to the
    // cover it was using, and a cached image no track references is
    // just a file the user cannot see or reach.
    let artwork = tracks::get(engine, track_id)
        .await
        .ok()
        .and_then(|row| row.artwork_path);

    let sql = "DELETE FROM tracks WHERE id = ?";
    engine
        .raw_sql_execute(sql, &[FilterValue::Int(track_id)])
        .await
        .map_err(|e| e.to_string())?;

    if let Some(art) = artwork {
        prune_cached_artwork(engine, &art).await;
    }
    // Leave no dangling playlist entry behind — SQLite reuses rowids,
    // so a stale id could later resolve to an unrelated track.
    crate::db::playlists::prune_track(engine, track_id)
        .await
        .map_err(|e| e.to_string())?;
    // Same hazard on the device manifest: a reused rowid would make an
    // unrelated track look already-synced at the old track's path.
    crate::db::device_objects::detach_track(engine, track_id)
        .await
        .map_err(|e| e.to_string())
}

/// Delete a cached cover once no track row points at it any more.
///
/// The cache lives under `$APPDATA/artwork/`, keyed by content hash, so
/// several albums can share one file — only the last reference takes it
/// with it. Anything outside the cache (a sidecar in the user's own
/// music folder) is left alone: it is not ours to delete.
async fn prune_cached_artwork(engine: &prax_sqlite::raw::SqliteRawEngine, artwork_path: &str) {
    let still_used: i64 = match engine
        .raw_sql_scalar(
            "SELECT COUNT(*) FROM tracks WHERE artwork_path = ?",
            &[FilterValue::String(artwork_path.to_string())],
        )
        .await
    {
        Ok(n) => n,
        Err(e) => {
            log::warn!("could not check whether {artwork_path} is still in use: {e}");
            return;
        }
    };
    if still_used > 0 {
        return;
    }
    let path = std::path::Path::new(artwork_path);
    // Only files we put in the cache: the parent directory is named
    // `artwork` and sits in the app's data dir.
    if path.parent().and_then(|p| p.file_name()) != Some(std::ffi::OsStr::new("artwork")) {
        return;
    }
    if let Err(e) = std::fs::remove_file(path) {
        if e.kind() != std::io::ErrorKind::NotFound {
            log::warn!("could not remove the cached cover {artwork_path}: {e}");
        }
    }
}

/// What a bulk removal actually did.
///
/// The caller acts on `removed` — stopping playback, pruning the queue,
/// forgetting cached state — so it has to be the ids that really went,
/// not the ids that were asked for. Deleting is not all-or-nothing: a
/// file on a read-only mount fails while its neighbours succeed.
#[derive(Debug, Default, Clone, serde::Serialize)]
pub struct RemoveSummary {
    pub removed: Vec<i64>,
    /// Titles of tracks that could not be removed, for the UI to name.
    pub failed: Vec<String>,
}

/// Remove several tracks from the library, leaving their files alone.
#[tauri::command]
pub async fn remove_tracks(
    state: tauri::State<'_, AppState>,
    track_ids: Vec<i64>,
) -> Result<RemoveSummary, String> {
    let mut summary = RemoveSummary::default();
    for id in track_ids {
        match remove_track_from(&state.db.engine, id).await {
            Ok(()) => summary.removed.push(id),
            Err(e) => {
                log::warn!("remove_tracks: track {id}: {e}");
                summary.failed.push(track_label(&state, id).await);
            }
        }
    }
    Ok(summary)
}

/// Send several tracks' files to the trash and drop their rows. A file
/// that cannot be trashed keeps its row: the library should not claim
/// to have deleted something that is still on disk.
#[tauri::command]
pub async fn trash_tracks(
    state: tauri::State<'_, AppState>,
    track_ids: Vec<i64>,
) -> Result<RemoveSummary, String> {
    let mut summary = RemoveSummary::default();
    for id in track_ids {
        match trash_one(&state, id).await {
            Ok(()) => summary.removed.push(id),
            Err(e) => {
                log::warn!("trash_tracks: track {id}: {e}");
                summary.failed.push(track_label(&state, id).await);
            }
        }
    }
    Ok(summary)
}

async fn trash_one(state: &AppState, track_id: i64) -> Result<(), String> {
    let row = tracks::get(&state.db.engine, track_id)
        .await
        .map_err(|e| e.to_string())?;
    // Best-effort: send to trash. Already-missing files shouldn't block
    // the DB cleanup.
    if std::path::Path::new(&row.file_path).exists() {
        trash::delete(&row.file_path).map_err(|e| e.to_string())?;
    }
    remove_track_from(&state.db.engine, track_id).await
}

/// A name for a track the UI can show in an error. Falls back to the
/// id when the row is unreadable — which is often why it failed.
async fn track_label(state: &AppState, track_id: i64) -> String {
    match tracks::get(&state.db.engine, track_id).await {
        Ok(row) => row.title,
        Err(_) => format!("track {track_id}"),
    }
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
    trash_one(&state, track_id).await
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
