//! The persisted feed (`new_releases.json`) and the user's feed
//! preferences.

use crate::library::genres::musicbrainz::fold;
use chrono::{DateTime, NaiveDate, Utc};
use serde::{Deserialize, Deserializer, Serialize};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

pub const FILE_NAME: &str = "new_releases.json";
pub const VERSION: u32 = 1;

pub const DEFAULT_WINDOW_DAYS: u32 = 90;
/// The shortest window the settings offer.
pub const MIN_WINDOW_DAYS: u32 = 30;
pub const MAX_WINDOW_DAYS: u32 = 365;

/// Stored under `preferences.release_prefs`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ReleasePrefs {
    /// How far back a release still counts as new.
    #[serde(default = "default_window", deserialize_with = "clamp_window")]
    pub window_days: u32,
    /// Qobuz catalog app_id. Optional: without it cards link to a
    /// Qobuz search instead of the album itself.
    #[serde(default, deserialize_with = "trim_app_id")]
    pub qobuz_app_id: Option<String>,
}

impl Default for ReleasePrefs {
    fn default() -> Self {
        Self {
            window_days: DEFAULT_WINDOW_DAYS,
            qobuz_app_id: None,
        }
    }
}

fn default_window() -> u32 {
    DEFAULT_WINDOW_DAYS
}

fn clamp_window<'de, D: Deserializer<'de>>(d: D) -> Result<u32, D::Error> {
    Ok(u32::deserialize(d)?.clamp(MIN_WINDOW_DAYS, MAX_WINDOW_DAYS))
}

/// A pasted app_id often carries whitespace; a blank one means none.
fn trim_app_id<'de, D: Deserializer<'de>>(d: D) -> Result<Option<String>, D::Error> {
    Ok(Option::<String>::deserialize(d)?
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty()))
}

/// Whether saving `new` over `old` should start a check: only once the
/// feed is in use, and only when it would fetch something the feed does
/// not have. That is a window wider than the one the feed actually
/// covers (not merely wider than the old setting: 365 -> 90 -> 180 has
/// nothing to fetch), or a newly set Qobuz app_id. Clearing the app_id
/// fetches nothing; earlier matches are kept.
pub fn needs_recheck(old: &ReleasePrefs, new: &ReleasePrefs, feed: Option<&Feed>) -> bool {
    let Some(feed) = feed else {
        return false;
    };
    let fetched = feed.checked_at.map(|_| feed.window_days);
    let widened = new.window_days != old.window_days && fetched.is_none_or(|w| new.window_days > w);
    let new_app_id = new.qobuz_app_id.is_some() && new.qobuz_app_id != old.qobuz_app_id;
    widened || new_app_id
}

/// A library artist's MusicBrainz identity, cached between runs.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ArtistRef {
    pub name: String,
    /// `None` when MusicBrainz had no confident match.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mbid: Option<String>,
    pub looked_up: NaiveDate,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct QobuzAlbum {
    pub id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub image: Option<String>,
}

/// One release group, as the feed stores it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Release {
    /// MusicBrainz release-group MBID.
    pub id: String,
    pub title: String,
    /// The artist credit as printed, e.g. "Artist feat. Guest".
    pub artist: String,
    /// The library artist this release was found through.
    pub artist_key: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub primary_type: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub secondary_types: Vec<String>,
    /// `YYYY-MM-DD`, or a shorter prefix when MusicBrainz only knows
    /// the month or year.
    pub release_date: String,
    /// The library already holds an album of this name by this artist.
    #[serde(default)]
    pub in_library: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub qobuz: Option<QobuzAlbum>,
    /// When Qobuz was last searched for this release, hit or miss, so a
    /// release Qobuz does not carry is not searched again every day.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub qobuz_checked: Option<NaiveDate>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Feed {
    pub version: u32,
    /// When the last refresh finished; `None` while the first is running.
    #[serde(default)]
    pub checked_at: Option<DateTime<Utc>>,
    /// The window the stored releases were fetched for.
    #[serde(default = "default_window")]
    pub window_days: u32,
    /// Keyed by the artist fold (see `genres::resolve::fold_key`).
    #[serde(default)]
    pub artists: BTreeMap<String, ArtistRef>,
    /// Newest first.
    #[serde(default)]
    pub releases: Vec<Release>,
    /// Why the last refresh stopped early, if it did.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_error: Option<String>,
    /// Why Qobuz matching was abandoned in the last refresh, if it was.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub qobuz_error: Option<String>,
    /// Fingerprint of the app_id the stored Qobuz misses were found with
    /// (see `qobuz::app_id_key`); a different app_id asks again.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub qobuz_key: Option<String>,
}

impl Default for Feed {
    fn default() -> Self {
        Self {
            version: VERSION,
            checked_at: None,
            window_days: DEFAULT_WINDOW_DAYS,
            artists: BTreeMap::new(),
            releases: Vec::new(),
            last_error: None,
            qobuz_error: None,
            qobuz_key: None,
        }
    }
}

impl Feed {
    /// `new_releases.json` next to the database file.
    pub fn path_for(db_path: &Path) -> PathBuf {
        db_path
            .parent()
            .map(Path::to_path_buf)
            .unwrap_or_default()
            .join(FILE_NAME)
    }

    /// `Ok(None)` when there is no feed yet. The file is a cache, so an
    /// unreadable or older-version one is dropped (with a warning)
    /// rather than reported: the next refresh rebuilds it.
    pub fn load(path: &Path) -> std::io::Result<Option<Feed>> {
        let text = match std::fs::read_to_string(path) {
            Ok(t) => t,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Err(e) => return Err(e),
        };
        match serde_json::from_str::<Feed>(&text) {
            Ok(feed) if feed.version == VERSION => Ok(Some(feed)),
            Ok(feed) => {
                log::warn!(
                    "{} is version {}, expected {VERSION}; starting over",
                    path.display(),
                    feed.version
                );
                Ok(Some(Feed::default()))
            }
            Err(e) => {
                log::warn!("{} is unreadable ({e}); starting over", path.display());
                Ok(Some(Feed::default()))
            }
        }
    }

    /// Write atomically; see [`crate::library::atomic::write_atomic`].
    pub fn save(&self, path: &Path) -> std::io::Result<()> {
        let text = serde_json::to_string_pretty(self).map_err(std::io::Error::other)?;
        crate::library::atomic::write_atomic(path, text.as_bytes())
    }

    /// Releases dated on or after `since`, newest first. A date
    /// MusicBrainz only knows to the month or year counts as the last
    /// day of it, so a release from "this month" is not dropped for
    /// lack of a day.
    pub fn visible(&self, since: NaiveDate) -> Vec<&Release> {
        let since = since.format("%Y-%m-%d").to_string();
        let mut out: Vec<&Release> = self
            .releases
            .iter()
            .filter(|r| latest_reading(&r.release_date) >= since)
            .collect();
        sort_newest_first(&mut out);
        out
    }
}

/// Newest first; same-day releases by artist, then title. A partial
/// date sorts as the last day it could be, as [`Feed::visible`] reads
/// it, so "2026-10" sits with late October rather than below it.
pub fn sort_newest_first<R: std::borrow::Borrow<Release>>(releases: &mut [R]) {
    releases.sort_by(|a, b| {
        let (a, b) = (a.borrow(), b.borrow());
        latest_reading(&b.release_date)
            .cmp(&latest_reading(&a.release_date))
            .then_with(|| a.artist.to_lowercase().cmp(&b.artist.to_lowercase()))
            .then_with(|| a.title.cmp(&b.title))
    });
}

/// Words that make a bracketed suffix an edition of the same album
/// rather than a different release ("(Remixes)", "(Live at X)").
/// "Version" is left out on purpose: "(Taylor's Version)" is a new record.
const EDITION_WORDS: &[&str] = &[
    "deluxe",
    "edition",
    "remaster",
    "expanded",
    "anniversary",
    "bonus",
    "reissue",
];

/// The folded forms an album title is known under: the title itself
/// and, for an edition such as "Album (Deluxe Edition)" or "Album
/// [Remastered]", the title before the bracket. Compared exactly, so
/// "Love" never claims "Lovesexy", and "Blackstar (Remixes)" is not
/// "Blackstar".
pub fn title_folds(album: &str) -> impl Iterator<Item = String> {
    let whole = fold(album);
    let base = album.find(['(', '[']).and_then(|at| {
        let suffix = album[at..].to_lowercase();
        EDITION_WORDS
            .iter()
            .any(|w| suffix.contains(w))
            .then(|| fold(&album[..at]))
    });
    let base = base.filter(|b| !b.is_empty() && *b != whole);
    std::iter::once(whole).chain(base)
}

/// Pad a partial `YYYY[-MM[-DD]]` date to the latest day it could be.
fn latest_reading(date: &str) -> String {
    match date.len() {
        4 => format!("{date}-12-31"),
        7 => format!("{date}-31"),
        _ => date.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn release(id: &str, date: &str, artist: &str) -> Release {
        Release {
            id: id.into(),
            title: format!("Title {id}"),
            artist: artist.into(),
            artist_key: artist.into(),
            primary_type: Some("Album".into()),
            secondary_types: vec![],
            release_date: date.into(),
            in_library: false,
            qobuz: None,
            qobuz_checked: None,
        }
    }

    #[test]
    fn prefs_clamp_the_window_and_blank_the_app_id() {
        let p: ReleasePrefs =
            serde_json::from_value(serde_json::json!({"window_days": 3, "qobuz_app_id": "  "}))
                .unwrap();
        assert_eq!(p.window_days, MIN_WINDOW_DAYS);
        assert_eq!(p.qobuz_app_id, None);
        let p: ReleasePrefs = serde_json::from_value(
            serde_json::json!({"window_days": 9000, "qobuz_app_id": " 42 "}),
        )
        .unwrap();
        assert_eq!(p.window_days, MAX_WINDOW_DAYS);
        assert_eq!(p.qobuz_app_id.as_deref(), Some("42"));
        let p: ReleasePrefs = serde_json::from_value(serde_json::json!({})).unwrap();
        assert_eq!(p, ReleasePrefs::default());
    }

    #[test]
    fn only_edition_brackets_are_dropped() {
        let folds = |t: &str| title_folds(t).collect::<Vec<_>>();
        assert_eq!(
            folds("Moon (Deluxe Edition)"),
            ["moondeluxeedition", "moon"]
        );
        assert_eq!(
            folds("Moon [2011 Remastered]"),
            ["moon2011remastered", "moon"]
        );
        assert_eq!(folds("Blackstar (Remixes)"), ["blackstarremixes"]);
        assert_eq!(folds("Red (Taylor's Version)"), ["redtaylorsversion"]);
        assert_eq!(folds("Moon"), ["moon"]);
    }

    #[test]
    fn rechecks_only_when_there_is_something_to_fetch() {
        let prefs = |w: u32, id: Option<&str>| ReleasePrefs {
            window_days: w,
            qobuz_app_id: id.map(str::to_string),
        };
        let fetched = |w: u32| Feed {
            checked_at: Some("2026-10-30T12:00:00Z".parse().unwrap()),
            window_days: w,
            ..Feed::default()
        };
        let p90 = prefs(90, None);
        // Never checked: nothing to refresh.
        assert!(!needs_recheck(&p90, &prefs(365, None), None));
        // Wider than what was fetched.
        assert!(needs_recheck(&p90, &prefs(180, None), Some(&fetched(90))));
        // Wider than the old setting, but within what was fetched.
        assert!(!needs_recheck(&p90, &prefs(180, None), Some(&fetched(365))));
        // Narrower.
        assert!(!needs_recheck(&p90, &prefs(30, None), Some(&fetched(90))));
        // A first check that never finished covers nothing yet.
        assert!(needs_recheck(
            &p90,
            &prefs(180, None),
            Some(&Feed::default())
        ));
        // A new app_id, but not a cleared or unchanged one.
        assert!(needs_recheck(
            &p90,
            &prefs(90, Some("1")),
            Some(&fetched(90))
        ));
        assert!(!needs_recheck(
            &prefs(90, Some("1")),
            &p90,
            Some(&fetched(90))
        ));
        assert!(!needs_recheck(
            &prefs(90, Some("1")),
            &prefs(90, Some("1")),
            Some(&fetched(90))
        ));
    }

    #[test]
    fn visible_filters_by_window_and_sorts_newest_first() {
        let feed = Feed {
            releases: vec![
                release("old", "2026-07-01", "A"),
                release("b", "2026-10-01", "B"),
                release("a", "2026-10-01", "a"),
                release("new", "2026-10-20", "C"),
                release("month", "2026-08", "D"),
            ],
            ..Feed::default()
        };
        let since = NaiveDate::from_ymd_opt(2026, 8, 15).unwrap();
        let ids: Vec<&str> = feed.visible(since).iter().map(|r| r.id.as_str()).collect();
        assert_eq!(ids, ["new", "a", "b", "month"]);
    }

    #[test]
    fn a_month_only_date_sorts_with_the_end_of_its_month() {
        let mut releases = vec![
            release("early", "2026-10-01", "A"),
            release("month", "2026-10", "B"),
            release("late", "2026-10-20", "C"),
        ];
        sort_newest_first(&mut releases);
        let ids: Vec<&str> = releases.iter().map(|r| r.id.as_str()).collect();
        assert_eq!(ids, ["month", "late", "early"]);
    }

    #[test]
    fn round_trips_and_survives_a_corrupt_file() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join(FILE_NAME);
        assert_eq!(Feed::load(&path).unwrap(), None);
        let mut feed = Feed::default();
        feed.releases.push(release("x", "2026-10-01", "A"));
        feed.save(&path).unwrap();
        assert_eq!(Feed::load(&path).unwrap(), Some(feed));
        std::fs::write(&path, "{not json").unwrap();
        assert_eq!(Feed::load(&path).unwrap(), Some(Feed::default()));
    }
}
