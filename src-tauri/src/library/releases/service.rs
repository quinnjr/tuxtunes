//! The app-side owner of the feed: at most one refresh at a time, run
//! on the async runtime, reporting through `releases:*` events.

use super::feed::{title_folds, Feed, ReleasePrefs};
use super::qobuz::{self, Qobuz};
use super::refresh::{refresh, LibraryArtist, Progress, RefreshInput};
use crate::db::preferences::{self, KEY_RELEASE_PREFS};
use crate::library::genres::map::GenreMap;
use crate::library::genres::musicbrainz::MusicBrainz;
use crate::library::genres::resolve::{artist_stats, fold_key, is_non_artist, ARTIST_FOLD_SQL};
use chrono::{Duration, Utc};
use prax_sqlite::raw::SqliteRawEngine;
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use tauri::{AppHandle, Emitter, Runtime};

/// Payload [`Progress`] after each batch.
pub const PROGRESS: &str = "releases:progress";
/// The feed file changed; re-read it.
pub const UPDATED: &str = "releases:updated";
/// A refresh ended. Payload [`Finished`].
pub const FINISHED: &str = "releases:finished";

/// A feed older than this is refreshed in the background.
pub const STALE_AFTER_HOURS: i64 = 24;

#[derive(Debug, Clone, serde::Serialize)]
pub struct Finished {
    /// `None` on success.
    pub error: Option<String>,
}

struct RunningGuard<'a>(&'a AtomicBool);

impl Drop for RunningGuard<'_> {
    fn drop(&mut self) {
        self.0.store(false, Ordering::SeqCst);
    }
}

pub struct ReleaseFeedService {
    feed_path: PathBuf,
    genre_map_path: PathBuf,
    running: AtomicBool,
    /// Another run was asked for while one was going; the running task
    /// goes again when it finishes, so a run that read the old prefs
    /// does not stand in for one asked for with new ones.
    rerun: AtomicBool,
}

impl ReleaseFeedService {
    pub fn new(db_path: &Path) -> Self {
        Self {
            feed_path: Feed::path_for(db_path),
            genre_map_path: GenreMap::path_for(db_path),
            running: AtomicBool::new(false),
            rerun: AtomicBool::new(false),
        }
    }

    pub fn is_running(&self) -> bool {
        self.running.load(Ordering::SeqCst)
    }

    /// The stored feed, read off the async workers: it is re-read on
    /// every `releases:updated`, and a big library makes a big file.
    pub async fn load(&self) -> Result<Option<Feed>, String> {
        let path = self.feed_path.clone();
        tokio::task::spawn_blocking(move || Feed::load(&path))
            .await
            .map_err(|e| e.to_string())?
            .map_err(|e| e.to_string())
    }

    /// Whether the background check should refresh now: only once the
    /// user has asked for a feed at least once (it sends every artist
    /// name to MusicBrainz, so the first run is theirs to start), and
    /// only when the last finished run is a day old.
    pub async fn is_stale(&self) -> bool {
        match self.load().await {
            Ok(Some(feed)) => feed
                .checked_at
                .is_none_or(|t| Utc::now() - t >= Duration::hours(STALE_AFTER_HOURS)),
            _ => false,
        }
    }

    /// Start a refresh unless one is running. Returns whether this call
    /// started it; the outcome arrives as [`FINISHED`]. With
    /// `rerun_if_running`, a call that finds a run going asks it to run
    /// once more when it is done (the user changed a setting, or asked
    /// explicitly); the background check passes `false`.
    pub fn start<R: Runtime>(
        self: &Arc<Self>,
        app: AppHandle<R>,
        engine: Arc<SqliteRawEngine>,
        rerun_if_running: bool,
    ) -> bool {
        if self.running.swap(true, Ordering::SeqCst) {
            if rerun_if_running {
                self.rerun.store(true, Ordering::SeqCst);
            }
            return false;
        }
        let this = Arc::clone(self);
        tauri::async_runtime::spawn(async move {
            // Clears `running` however this task ends; otherwise every
            // later start would be refused until restart.
            let mut _guard = RunningGuard(&this.running);
            loop {
                this.rerun.store(false, Ordering::SeqCst);
                let error = this.run_once(&app, &engine).await.err();
                if let Some(e) = &error {
                    log::warn!("new releases: refresh failed: {e}");
                    this.record_error(e).await;
                    let _ = app.emit(UPDATED, ());
                }
                if this.rerun.load(Ordering::SeqCst) {
                    continue;
                }
                drop(_guard);
                // A request can land after the check above but before
                // `running` cleared; `start` then saw a run going and only
                // set `rerun`. Take it up here unless a fresh `start` has
                // already claimed `running`. The flag is left alone in that
                // case: the fresh run clears it as it begins, before reading
                // the prefs, so a request that lands later still reruns it.
                if this.rerun.load(Ordering::SeqCst) && !this.running.swap(true, Ordering::SeqCst) {
                    _guard = RunningGuard(&this.running);
                    continue;
                }
                let _ = app.emit(FINISHED, Finished { error });
                break;
            }
        });
        true
    }

    /// One run, in its own task so that a panic comes back here as an
    /// error and [`FINISHED`] still goes out, rather than leaving the UI
    /// showing a check that will never end.
    async fn run_once<R: Runtime>(
        self: &Arc<Self>,
        app: &AppHandle<R>,
        engine: &Arc<SqliteRawEngine>,
    ) -> Result<(), String> {
        let (this, app, engine) = (Arc::clone(self), app.clone(), Arc::clone(engine));
        match tauri::async_runtime::spawn(async move { this.run(&app, &engine).await }).await {
            Ok(result) => result.map_err(|e| format!("{e:#}")),
            Err(e) => Err(format!("the refresh task stopped unexpectedly: {e}")),
        }
    }

    /// Keep the error with the feed so the view can show it after a
    /// restart, not only to whoever saw the event. A first run that
    /// failed before writing anything still gets a file, so the user who
    /// pressed Check sees why it did nothing. Called under `running`,
    /// so no run is writing the file at the same time.
    async fn record_error(&self, error: &str) {
        let (path, error) = (self.feed_path.clone(), error.to_string());
        let write = move || {
            let mut feed = Feed::load(&path)?.unwrap_or_default();
            feed.last_error = Some(error);
            feed.save(&path)
        };
        match tokio::task::spawn_blocking(write).await {
            Ok(Ok(())) => {}
            Ok(Err(e)) => log::warn!("new releases: cannot record the error in the feed: {e}"),
            Err(e) => log::warn!("new releases: cannot record the error in the feed: {e}"),
        }
    }

    async fn run<R: Runtime>(
        &self,
        app: &AppHandle<R>,
        engine: &SqliteRawEngine,
    ) -> anyhow::Result<()> {
        let prefs = preferences::get::<ReleasePrefs>(engine, KEY_RELEASE_PREFS)
            .await?
            .unwrap_or_default();
        // File work in this run blocks in place: the run is a task on the
        // multi-threaded Tauri runtime, and the checkpoint below is a
        // plain closure called from inside `refresh`.
        use tokio::task::block_in_place;
        let input = RefreshInput {
            artists: library_artists(engine).await?,
            owned: owned_albums(engine).await?,
            known_mbids: block_in_place(|| self.genre_map_mbids()),
            now: Utc::now(),
            window_days: prefs.window_days,
            qobuz_key: prefs.qobuz_app_id.as_deref().map(qobuz::app_id_key),
        };
        let prev = block_in_place(|| -> std::io::Result<Feed> {
            let prev = Feed::load(&self.feed_path)?.unwrap_or_default();
            // Save once up front: the file existing is what opts the
            // background check in (see `is_stale`).
            prev.save(&self.feed_path)?;
            Ok(prev)
        })?;
        // Say so at once: the first checkpoint can be a batch search and
        // a page of Qobuz lookups away, and until the view hears of the
        // run it offers Check Now, which would queue a duplicate run.
        let _ = app.emit(
            PROGRESS,
            Progress {
                done: 0,
                total: input.artists.len(),
                releases: prev.releases.len(),
            },
        );
        let _ = app.emit(UPDATED, ());
        let mb = MusicBrainz::new(HashSet::new())?;
        let qobuz = prefs.qobuz_app_id.map(Qobuz::new).transpose()?;
        let path = self.feed_path.clone();
        refresh(
            &mb,
            qobuz.as_ref(),
            input,
            prev,
            |feed, progress: Progress| {
                if let Err(e) = block_in_place(|| feed.save(&path)) {
                    log::warn!("new releases: cannot save {}: {e}", path.display());
                }
                let _ = app.emit(PROGRESS, progress);
                let _ = app.emit(UPDATED, ());
            },
        )
        .await?;
        Ok(())
    }

    /// MBIDs the genre resolver already found, by artist fold. A
    /// missing or unreadable map just means none are known.
    fn genre_map_mbids(&self) -> HashMap<String, String> {
        match GenreMap::load(&self.genre_map_path) {
            Ok(Some(map)) => map
                .artists
                .into_iter()
                .filter_map(|(key, entry)| entry.mbid.map(|id| (fold_key(&key), id)))
                .collect(),
            Ok(None) => HashMap::new(),
            Err(e) => {
                log::warn!("new releases: ignoring genre map: {e}");
                HashMap::new()
            }
        }
    }
}

async fn library_artists(engine: &SqliteRawEngine) -> anyhow::Result<Vec<LibraryArtist>> {
    Ok(artist_stats(engine)
        .await?
        .into_iter()
        .filter(|a| !is_non_artist(&a.key))
        .map(|a| LibraryArtist {
            key: a.key,
            fold: a.fold,
            track_count: a.track_count,
        })
        .collect())
}

/// Album titles per artist fold, folded for matching (see
/// [`title_folds`]).
async fn owned_albums(
    engine: &SqliteRawEngine,
) -> anyhow::Result<HashMap<String, HashSet<String>>> {
    let sql = format!(
        "SELECT DISTINCT {ARTIST_FOLD_SQL} AS f, TRIM(album) AS a FROM tracks \
         WHERE album IS NOT NULL AND TRIM(album) != ''"
    );
    let mut out: HashMap<String, HashSet<String>> = HashMap::new();
    for row in engine.raw_sql_query(&sql, &[]).await? {
        let j = row.into_json();
        let (Some(f), Some(a)) = (
            j.get("f").and_then(|v| v.as_str()),
            j.get("a").and_then(|v| v.as_str()),
        ) else {
            continue;
        };
        out.entry(f.to_string()).or_default().extend(title_folds(a));
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::Db;
    use prax_query::filter::FilterValue as FV;

    #[tokio::test]
    async fn owned_albums_groups_folded_titles_by_artist() {
        let tmp = tempfile::NamedTempFile::new().unwrap();
        let db = Db::open(tmp.path()).await.unwrap();
        for (n, (artist, album_artist, album)) in [
            ("Björk", None, Some("Homogenic")),
            ("Guest", Some("Björk"), Some("  Vespertine ")),
            ("Björk", None, None),
            ("ABBA", None, Some("Arrival")),
        ]
        .into_iter()
        .enumerate()
        {
            db.engine
                .raw_sql_execute(
                    "INSERT INTO tracks (title, artist, album_artist, album, duration_ms, \
                     size_bytes, file_path, playlist_ids) VALUES (?, ?, ?, ?, 1000, 0, ?, '[]')",
                    &[
                        FV::String(format!("t{n}")),
                        FV::String(artist.into()),
                        album_artist
                            .map(|s| FV::String(s.into()))
                            .unwrap_or(FV::Null),
                        album.map(|s| FV::String(s.into())).unwrap_or(FV::Null),
                        FV::String(format!("/tmp/rel-t{n}.flac")),
                    ],
                )
                .await
                .unwrap();
        }
        let owned = owned_albums(&db.engine).await.unwrap();
        // SQLite's LOWER is ASCII-only, as `fold_key` is.
        assert_eq!(
            owned[&fold_key("Björk")],
            HashSet::from(["homogenic".to_string(), "vespertine".to_string()])
        );
        assert_eq!(owned["abba"], HashSet::from(["arrival".to_string()]));
    }
}
