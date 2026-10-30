//! "New From Your Artists" Tauri commands.

use crate::db::preferences::{self, KEY_RELEASE_PREFS};
use crate::library::releases::feed::{needs_recheck, Release};
use crate::library::releases::musicbrainz::{cover_art_url, musicbrainz_url};
use crate::library::releases::qobuz;
use crate::library::releases::ReleasePrefs;
use crate::runtime::AppState;
use chrono::{DateTime, Duration, Utc};

/// One grid card, with every link already built so the view never
/// assembles a URL itself.
#[derive(Debug, Clone, serde::Serialize)]
pub struct ReleaseCard {
    pub id: String,
    pub title: String,
    pub artist: String,
    pub release_date: String,
    pub primary_type: Option<String>,
    pub secondary_types: Vec<String>,
    pub in_library: bool,
    /// Qobuz's cover when matched, else the Cover Art Archive's (which
    /// may 404; the view falls back to a placeholder).
    pub cover_url: String,
    /// The Qobuz album page when matched, else a Qobuz search.
    pub qobuz_url: String,
    pub qobuz_exact: bool,
    pub musicbrainz_url: String,
}

impl From<&Release> for ReleaseCard {
    fn from(r: &Release) -> Self {
        let (qobuz_url, cover) = match &r.qobuz {
            Some(q) => (qobuz::album_url(&q.id), q.image.clone()),
            None => (qobuz::search_url(&r.artist_key, &r.title), None),
        };
        Self {
            id: r.id.clone(),
            title: r.title.clone(),
            artist: r.artist.clone(),
            release_date: r.release_date.clone(),
            primary_type: r.primary_type.clone(),
            secondary_types: r.secondary_types.clone(),
            in_library: r.in_library,
            cover_url: cover.unwrap_or_else(|| cover_art_url(&r.id)),
            qobuz_exact: r.qobuz.is_some(),
            qobuz_url,
            musicbrainz_url: musicbrainz_url(&r.id),
        }
    }
}

#[derive(Debug, Clone, serde::Serialize)]
pub struct FeedView {
    /// `None` until a refresh has finished.
    pub checked_at: Option<DateTime<Utc>>,
    pub running: bool,
    /// False until the user has checked once; the view shows its
    /// opt-in prompt instead of an empty grid.
    pub started: bool,
    /// The window the releases shown cover: the preference, unless the
    /// last finished check fetched a narrower one (a widened window
    /// takes a check to fill in).
    pub window_days: u32,
    /// The window the last finished check fetched; `None` before one has
    /// finished. Settings compare a new window against this to decide
    /// whether a re-check would fetch anything.
    pub fetched_window_days: Option<u32>,
    pub releases: Vec<ReleaseCard>,
    pub last_error: Option<String>,
    pub qobuz_error: Option<String>,
}

async fn load_prefs(state: &AppState) -> Result<ReleasePrefs, String> {
    Ok(
        preferences::get::<ReleasePrefs>(&state.db.engine, KEY_RELEASE_PREFS)
            .await
            .map_err(|e| e.to_string())?
            .unwrap_or_default(),
    )
}

/// The feed as stored, cut to the current window. Shrinking the window
/// takes effect here at once; widening it needs a refresh to fetch the
/// older releases, and until then the view reports the narrower window
/// it actually has.
#[tauri::command]
pub async fn get_new_releases(state: tauri::State<'_, AppState>) -> Result<FeedView, String> {
    let prefs = load_prefs(&state).await?;
    let feed = state.releases.load().await?;
    let fetched = feed
        .as_ref()
        .filter(|f| f.checked_at.is_some())
        .map(|f| f.window_days);
    let window_days = fetched.map_or(prefs.window_days, |w| w.min(prefs.window_days));
    let since = Utc::now().date_naive() - Duration::days(i64::from(window_days));
    Ok(FeedView {
        running: state.releases.is_running(),
        started: feed.is_some(),
        window_days,
        fetched_window_days: fetched,
        checked_at: feed.as_ref().and_then(|f| f.checked_at),
        releases: feed
            .as_ref()
            .map(|f| {
                f.visible(since)
                    .into_iter()
                    .map(ReleaseCard::from)
                    .collect()
            })
            .unwrap_or_default(),
        last_error: feed.as_ref().and_then(|f| f.last_error.clone()),
        qobuz_error: feed.and_then(|f| f.qobuz_error),
    })
}

/// Start a refresh. `false` when one was already running, in which case
/// that run goes once more when it ends (the caller may have changed a
/// setting it read at its start); either way the outcome arrives on
/// `releases:finished`.
#[tauri::command]
pub async fn check_new_releases(
    app: tauri::AppHandle,
    state: tauri::State<'_, AppState>,
) -> Result<bool, String> {
    Ok(state
        .releases
        .start(app, std::sync::Arc::clone(&state.db.engine), true))
}

#[tauri::command]
pub async fn get_release_prefs(state: tauri::State<'_, AppState>) -> Result<ReleasePrefs, String> {
    load_prefs(&state).await
}

/// Store the prefs and, when the change has something to fetch (see
/// [`needs_recheck`]), start a check (or queue one behind a running
/// check, which read the old prefs). Decided here rather than in the
/// view, which may hold a stale or not-yet-loaded copy of the feed.
#[tauri::command]
pub async fn set_release_prefs(
    app: tauri::AppHandle,
    state: tauri::State<'_, AppState>,
    prefs: ReleasePrefs,
) -> Result<ReleasePrefs, String> {
    let old = load_prefs(&state).await?;
    preferences::set(&state.db.engine, KEY_RELEASE_PREFS, &prefs)
        .await
        .map_err(|e| e.to_string())?;
    let feed = state.releases.load().await?;
    if needs_recheck(&old, &prefs, feed.as_ref()) {
        state
            .releases
            .start(app, std::sync::Arc::clone(&state.db.engine), true);
    }
    // Deserialization clamped the window and trimmed the app_id.
    Ok(prefs)
}

/// Hosts a release card may open. Anything else is refused, so a
/// stray value in the feed file cannot make the app launch arbitrary
/// URLs.
const LINK_HOSTS: &[&str] = &[
    "open.qobuz.com",
    "play.qobuz.com",
    "www.qobuz.com",
    "musicbrainz.org",
];

pub fn is_allowed_link(url: &str) -> bool {
    reqwest::Url::parse(url).is_ok_and(|u| {
        u.scheme() == "https" && u.host_str().is_some_and(|h| LINK_HOSTS.contains(&h))
    })
}

/// Open a release link in the default browser; see [`super::xdg_open`].
#[tauri::command]
pub async fn open_release_link(url: String) -> Result<(), String> {
    if !is_allowed_link(&url) {
        return Err(format!("not a Qobuz or MusicBrainz link: {url}"));
    }
    super::xdg_open(std::ffi::OsStr::new(&url))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::library::releases::feed::QobuzAlbum;

    #[test]
    fn only_https_qobuz_and_musicbrainz_links_open() {
        assert!(is_allowed_link("https://open.qobuz.com/album/123"));
        assert!(is_allowed_link("https://play.qobuz.com/search?q=a%20b"));
        assert!(is_allowed_link("https://musicbrainz.org/release-group/x"));
        assert!(!is_allowed_link("http://open.qobuz.com/album/123"));
        assert!(!is_allowed_link(
            "https://open.qobuz.com.evil.example/album/1"
        ));
        assert!(!is_allowed_link("file:///etc/passwd"));
        assert!(!is_allowed_link("not a url"));
    }

    #[test]
    fn card_links_the_album_when_matched_and_a_search_otherwise() {
        let mut r = Release {
            id: "rg".into(),
            title: "Moon".into(),
            artist: "Björk & Guest".into(),
            artist_key: "Björk".into(),
            primary_type: Some("Album".into()),
            secondary_types: vec![],
            release_date: "2026-10-01".into(),
            in_library: false,
            qobuz: None,
            qobuz_checked: None,
        };
        let card = ReleaseCard::from(&r);
        assert!(!card.qobuz_exact);
        assert_eq!(
            card.qobuz_url,
            "https://play.qobuz.com/search?q=Bj%C3%B6rk%20Moon"
        );
        assert_eq!(
            card.cover_url,
            "https://coverartarchive.org/release-group/rg/front-250"
        );

        r.qobuz = Some(QobuzAlbum {
            id: "q1".into(),
            image: Some("https://static.qobuz.com/x_600.jpg".into()),
        });
        let card = ReleaseCard::from(&r);
        assert!(card.qobuz_exact);
        assert_eq!(card.qobuz_url, "https://open.qobuz.com/album/q1");
        assert_eq!(card.cover_url, "https://static.qobuz.com/x_600.jpg");
        assert!(is_allowed_link(&card.qobuz_url) && is_allowed_link(&card.musicbrainz_url));
    }
}
