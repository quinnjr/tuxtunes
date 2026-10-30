//! Release-group search: one Lucene query covers a batch of artists
//! and the whole date window, so a library of a thousand artists costs
//! about forty requests once their MBIDs are known.

use super::feed::Release;
use crate::library::genres::musicbrainz::MusicBrainz;
use chrono::NaiveDate;
use serde::Deserialize;
use std::collections::HashMap;
use std::future::Future;

/// Artists per release-group query. Keeps the URL around 1.5 KB.
pub const BATCH: usize = 25;
/// MusicBrainz's maximum page size.
pub const PAGE: u32 = 100;
/// Pages read per batch before giving up on the rest. Twenty-five
/// artists rarely put out more than a page in a window.
pub const MAX_PAGES: u32 = 5;

/// Secondary types that are not a new record by the artist: label
/// repackages, mixes, and spoken word.
const SKIPPED_SECONDARY: &[&str] = &[
    "compilation",
    "dj-mix",
    "mixtape/street",
    "interview",
    "audiobook",
    "audio drama",
    "spokenword",
];

/// Whether `s` is an MBID: a lowercase-or-uppercase hex UUID,
/// `8-4-4-4-12`. Anything else must not reach a Lucene query.
pub fn is_mbid(s: &str) -> bool {
    s.len() == 36
        && s.char_indices().all(|(i, c)| match i {
            8 | 13 | 18 | 23 => c == '-',
            _ => c.is_ascii_hexdigit(),
        })
}

pub fn musicbrainz_url(release_group_id: &str) -> String {
    format!("https://musicbrainz.org/release-group/{release_group_id}")
}

pub fn cover_art_url(release_group_id: &str) -> String {
    format!("https://coverartarchive.org/release-group/{release_group_id}/front-250")
}

/// The Lucene query for release groups by any of `mbids` first released
/// between `since` and `until` inclusive. Albums, EPs and singles only;
/// "Other" and "Broadcast" are mostly bootlegs and radio sessions.
///
/// The range starts at the month of `since`: a month-only date
/// ("2026-08") sorts before "2026-08-15" as a string and would be left
/// out, although `Feed::visible` reads it as the end of its month.
/// (Year-only dates come back from MusicBrainz's range as they are; an
/// explicit `firstreleasedate:2026` would match every date in the year.)
/// `visible` trims the extra days.
pub fn release_group_query(mbids: &[String], since: NaiveDate, until: NaiveDate) -> String {
    let artists = mbids
        .iter()
        .map(|id| format!("arid:{id}"))
        .collect::<Vec<_>>()
        .join(" OR ");
    format!(
        "({artists}) AND firstreleasedate:[{} TO {}] \
         AND (primarytype:album OR primarytype:ep OR primarytype:single)",
        since.format("%Y-%m"),
        until.format("%Y-%m-%d"),
    )
}

pub trait ReleaseSearch {
    /// The MBID of the artist called `name`, or `None` when MusicBrainz
    /// has no confident match.
    fn find_mbid(&self, name: &str) -> impl Future<Output = anyhow::Result<Option<String>>> + Send;

    /// One page of a release-group search.
    fn release_groups(
        &self,
        query: &str,
        offset: u32,
    ) -> impl Future<Output = anyhow::Result<serde_json::Value>> + Send;
}

impl ReleaseSearch for MusicBrainz {
    async fn find_mbid(&self, name: &str) -> anyhow::Result<Option<String>> {
        Ok(self.find_artist(name).await?.map(|hit| hit.mbid))
    }

    async fn release_groups(&self, query: &str, offset: u32) -> anyhow::Result<serde_json::Value> {
        let limit = PAGE.to_string();
        let offset = offset.to_string();
        self.get(
            "release-group/",
            &[
                ("query", query),
                ("fmt", "json"),
                ("limit", limit.as_str()),
                ("offset", offset.as_str()),
            ],
            "release groups",
        )
        .await
    }
}

#[derive(Debug, Deserialize)]
struct SearchBody {
    #[serde(default)]
    count: u32,
    /// Parsed one by one, so a single odd record costs only itself.
    #[serde(rename = "release-groups")]
    release_groups: Vec<serde_json::Value>,
}

#[derive(Debug, Deserialize)]
struct ReleaseGroup {
    id: String,
    title: String,
    #[serde(default, rename = "primary-type")]
    primary_type: Option<String>,
    #[serde(default, rename = "secondary-types")]
    secondary_types: Vec<String>,
    #[serde(default, rename = "first-release-date")]
    first_release_date: Option<String>,
    #[serde(default, rename = "artist-credit")]
    artist_credit: Vec<Credit>,
}

#[derive(Debug, Deserialize)]
struct Credit {
    name: String,
    #[serde(default)]
    joinphrase: String,
    #[serde(default)]
    artist: Option<CreditArtist>,
}

#[derive(Debug, Deserialize)]
struct CreditArtist {
    id: String,
}

/// One page of release groups, parsed.
#[derive(Debug, Default)]
pub struct Page {
    /// Total hits for the query across all pages.
    pub count: u32,
    pub releases: Vec<Release>,
}

/// Turn a search body into feed releases. `artist_keys` maps each MBID
/// the query asked about to its library artist key; a release group is
/// filed under the first credited artist found there. Groups with no
/// date, a skipped secondary type, or no credited library artist (the
/// search matched an alias) are dropped, as is a record that does not
/// parse (with a warning). A body that is not a release-group page at
/// all is an error: reading it as "no releases" would silently wipe the
/// batch's artists from the feed. `in_library` and `qobuz` are left for
/// the caller.
pub fn parse_page(
    body: &serde_json::Value,
    artist_keys: &HashMap<String, String>,
) -> anyhow::Result<Page> {
    let parsed = serde_json::from_value::<SearchBody>(body.clone())
        .map_err(|e| anyhow::anyhow!("unexpected MusicBrainz release-group response: {e}"))?;
    let releases = parsed
        .release_groups
        .into_iter()
        .filter_map(|raw| match serde_json::from_value::<ReleaseGroup>(raw) {
            Ok(rg) => Some(rg),
            Err(e) => {
                log::warn!("musicbrainz: skipping an unreadable release group: {e}");
                None
            }
        })
        .filter_map(|rg| {
            let date = rg.first_release_date.filter(|d| !d.is_empty())?;
            if rg
                .secondary_types
                .iter()
                .any(|t| SKIPPED_SECONDARY.contains(&t.to_lowercase().as_str()))
            {
                return None;
            }
            let artist_key = rg
                .artist_credit
                .iter()
                .find_map(|c| artist_keys.get(&c.artist.as_ref()?.id))?
                .clone();
            let artist = rg
                .artist_credit
                .iter()
                .map(|c| format!("{}{}", c.name, c.joinphrase))
                .collect::<String>();
            Some(Release {
                id: rg.id,
                title: rg.title,
                artist,
                artist_key,
                primary_type: rg.primary_type,
                secondary_types: rg.secondary_types,
                release_date: date,
                in_library: false,
                qobuz: None,
                qobuz_checked: None,
            })
        })
        .collect();
    Ok(Page {
        count: parsed.count,
        releases,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn d(y: i32, m: u32, day: u32) -> NaiveDate {
        NaiveDate::from_ymd_opt(y, m, day).unwrap()
    }

    #[test]
    fn recognises_mbids_only() {
        assert!(is_mbid("65f4f0c5-ef9e-490c-aee3-909e7ae6b2ab"));
        assert!(is_mbid("65F4F0C5-EF9E-490C-AEE3-909E7AE6B2AB"));
        assert!(!is_mbid("65f4f0c5-ef9e-490c-aee3-909e7ae6b2a"));
        assert!(!is_mbid("65f4f0c5-ef9e-490c-aee3-909e7ae6b2a)"));
        assert!(!is_mbid("https://musicbrainz.org/artist/65f4f0c5"));
    }

    #[test]
    fn query_ors_the_artists_and_bounds_the_dates() {
        let q = release_group_query(&["a1".into(), "b2".into()], d(2026, 8, 15), d(2026, 10, 30));
        assert_eq!(
            q,
            "(arid:a1 OR arid:b2) AND firstreleasedate:[2026-08 TO 2026-10-30] \
             AND (primarytype:album OR primarytype:ep OR primarytype:single)"
        );
    }

    #[test]
    fn parses_credits_and_drops_compilations_and_undated_groups() {
        let body = json!({
            "count": 4,
            "release-groups": [
                {
                    "id": "rg1", "title": "New Album", "primary-type": "Album",
                    "first-release-date": "2026-10-01",
                    "artist-credit": [
                        {"name": "Guest", "joinphrase": " & ", "artist": {"id": "other"}},
                        {"name": "Radiohead", "joinphrase": "", "artist": {"id": "rh"}}
                    ]
                },
                {
                    "id": "rg2", "title": "Hits", "primary-type": "Album",
                    "secondary-types": ["Compilation"], "first-release-date": "2026-09-01",
                    "artist-credit": [{"name": "Radiohead", "artist": {"id": "rh"}}]
                },
                {
                    "id": "rg3", "title": "Someday", "primary-type": "Single",
                    "first-release-date": "",
                    "artist-credit": [{"name": "Radiohead", "artist": {"id": "rh"}}]
                },
                {
                    "id": "rg4", "title": "Alias Hit", "primary-type": "Single",
                    "first-release-date": "2026-09-02",
                    "artist-credit": [{"name": "Someone", "artist": {"id": "nobody"}}]
                }
            ]
        });
        let keys = HashMap::from([("rh".to_string(), "Radiohead".to_string())]);
        let page = parse_page(&body, &keys).unwrap();
        assert_eq!(page.count, 4);
        assert_eq!(page.releases.len(), 1);
        let r = &page.releases[0];
        assert_eq!(r.id, "rg1");
        assert_eq!(r.artist, "Guest & Radiohead");
        assert_eq!(r.artist_key, "Radiohead");
        assert_eq!(r.primary_type.as_deref(), Some("Album"));
        assert_eq!(r.release_date, "2026-10-01");
    }

    #[test]
    fn a_body_that_is_not_a_page_is_an_error() {
        assert!(parse_page(&json!({"error": "busy"}), &HashMap::new()).is_err());
    }

    #[test]
    fn one_unreadable_record_costs_only_itself() {
        let body = json!({"count": 3, "release-groups": [
            {"id": "no-title"},
            {
                "id": "rg1", "title": "Kept", "first-release-date": "2026-10-01",
                "artist-credit": [
                    {"name": "Unlinked", "joinphrase": " & "},
                    {"name": "Radiohead", "artist": {"id": "rh"}}
                ]
            }
        ]});
        let keys = HashMap::from([("rh".to_string(), "Radiohead".to_string())]);
        let page = parse_page(&body, &keys).unwrap();
        let ids: Vec<&str> = page.releases.iter().map(|r| r.id.as_str()).collect();
        assert_eq!(ids, ["rg1"]);
        assert_eq!(page.releases[0].artist, "Unlinked & Radiohead");
    }
}
