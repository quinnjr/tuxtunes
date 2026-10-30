//! Qobuz links. Without an app_id a card opens a Qobuz search for the
//! artist and title; with one, `album/search` finds the album itself
//! for an exact link and Qobuz's own cover.

use super::feed::{title_folds, QobuzAlbum};
use crate::library::genres::musicbrainz::fold;
use crate::library::pacer::Pacer;
use percent_encoding::{utf8_percent_encode, NON_ALPHANUMERIC};
use std::future::Future;
use std::time::Duration;

pub const API_ROOT: &str = "https://www.qobuz.com/api.json/0.2";
/// Qobuz publishes no rate limit; stay well clear of looking like a
/// scraper.
pub const MIN_SPACING: Duration = Duration::from_millis(300);

/// A fingerprint of an app_id to store beside Qobuz results, so the
/// feed can tell which app_id they came from without keeping the id
/// itself. FNV-1a: stable across builds, unlike `DefaultHasher`.
pub fn app_id_key(app_id: &str) -> String {
    let h = app_id.bytes().fold(0xcbf2_9ce4_8422_2325u64, |h, b| {
        (h ^ u64::from(b)).wrapping_mul(0x0100_0000_01b3)
    });
    format!("{h:016x}")
}

pub fn album_url(id: &str) -> String {
    format!(
        "https://open.qobuz.com/album/{}",
        utf8_percent_encode(id, NON_ALPHANUMERIC)
    )
}

pub fn search_url(artist: &str, title: &str) -> String {
    format!(
        "https://play.qobuz.com/search?q={}",
        utf8_percent_encode(&format!("{artist} {title}"), NON_ALPHANUMERIC)
    )
}

pub trait AlbumLookup {
    /// The Qobuz album for `title` by `artist`, or `None` when the
    /// catalog has nothing that matches closely enough.
    fn find_album(
        &self,
        artist: &str,
        title: &str,
    ) -> impl Future<Output = anyhow::Result<Option<QobuzAlbum>>> + Send;
}

/// Pick the album an `album/search` body names. The artist must match
/// after folding; the title must match exactly after folding, or else
/// match the Qobuz title with its bracketed edition suffix removed
/// ("Album" for "Album (Deluxe)", never "Moon" for "Moon Safari"). An
/// exact title wins over an edition anywhere in the results.
pub fn pick_album(body: &serde_json::Value, artist: &str, title: &str) -> Option<QobuzAlbum> {
    let items = body.pointer("/albums/items")?.as_array()?;
    let want_artist = fold(artist);
    let want_title = fold(title);
    if want_title.is_empty() {
        return None;
    }
    let candidates: Vec<(&serde_json::Value, Vec<String>)> = items
        .iter()
        .filter(|item| {
            item.pointer("/artist/name")
                .and_then(|v| v.as_str())
                .is_some_and(|name| fold(name) == want_artist)
        })
        .filter_map(|item| {
            let t = item.get("title").and_then(|v| v.as_str())?;
            Some((item, title_folds(t).collect()))
        })
        .collect();
    let item = candidates
        .iter()
        .find(|(_, folds)| folds[0] == want_title)
        .or_else(|| {
            candidates
                .iter()
                .find(|(_, folds)| folds.contains(&want_title))
        })
        .map(|(item, _)| *item)?;
    // Album ids are strings ("0886448..."), but older entries are bare
    // numbers.
    let id = match item.get("id")? {
        serde_json::Value::String(s) => s.clone(),
        serde_json::Value::Number(n) => n.to_string(),
        _ => return None,
    };
    let image = ["/image/large", "/image/small"]
        .iter()
        .find_map(|p| item.pointer(p).and_then(|v| v.as_str()))
        .map(str::to_string);
    Some(QobuzAlbum { id, image })
}

/// Qobuz refused the app_id. Unlike a timeout or a 5xx this will not
/// get better within a run, so matching stops; see `refresh`.
#[derive(Debug)]
pub struct AppIdRejected(pub String);

impl std::fmt::Display for AppIdRejected {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for AppIdRejected {}

/// Whether a 400/401 reply is about the app_id rather than the query.
fn blames_app_id(status: reqwest::StatusCode, message: &str) -> bool {
    status == reqwest::StatusCode::UNAUTHORIZED || message.to_lowercase().contains("app_id")
}

pub struct Qobuz {
    http: reqwest::Client,
    app_id: String,
    pacer: Pacer,
}

impl Qobuz {
    pub fn new(app_id: String) -> anyhow::Result<Self> {
        let http = reqwest::Client::builder()
            .timeout(Duration::from_secs(20))
            .build()?;
        Ok(Self {
            http,
            app_id,
            pacer: Pacer::new(MIN_SPACING),
        })
    }
}

impl AlbumLookup for Qobuz {
    async fn find_album(&self, artist: &str, title: &str) -> anyhow::Result<Option<QobuzAlbum>> {
        self.pacer.wait().await;
        let query = format!("{artist} {title}");
        let resp = self
            .http
            .get(format!("{API_ROOT}/album/search"))
            .header("X-App-Id", &self.app_id)
            .query(&[
                ("query", query.as_str()),
                ("limit", "10"),
                ("app_id", self.app_id.as_str()),
            ])
            .send()
            .await?;
        let status = resp.status();
        // Qobuz answers a bad or revoked app_id with 400 ("Invalid or
        // missing app_id parameter") or 401. A 400 about anything else is
        // this one query's problem, not the app_id's.
        if status == reqwest::StatusCode::BAD_REQUEST || status == reqwest::StatusCode::UNAUTHORIZED
        {
            let body: serde_json::Value = resp.json().await.unwrap_or_default();
            let msg = body
                .get("message")
                .and_then(|m| m.as_str())
                .unwrap_or("rejected the request")
                .to_string();
            if blames_app_id(status, &msg) {
                return Err(AppIdRejected(format!("Qobuz rejected the app_id: {msg}")).into());
            }
            anyhow::bail!("Qobuz rejected the search for {title:?}: {msg}");
        }
        let body: serde_json::Value = resp.error_for_status()?.json().await?;
        Ok(pick_album(&body, artist, title))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn urls_encode_their_parts() {
        assert_eq!(album_url("0886448"), "https://open.qobuz.com/album/0886448");
        assert_eq!(
            search_url("AC/DC", "Power Up"),
            "https://play.qobuz.com/search?q=AC%2FDC%20Power%20Up"
        );
    }

    #[test]
    fn picks_the_exact_title_over_a_deluxe_prefix() {
        let body = json!({"albums": {"items": [
            {"id": "deluxe", "title": "Moon (Deluxe)", "artist": {"name": "Björk"},
             "image": {"large": "L1", "small": "S1"}},
            {"id": "tribute", "title": "Moon", "artist": {"name": "Bjork Tribute Band"}},
            {"id": "plain", "title": "MOON", "artist": {"name": "bjork"},
             "image": {"small": "S2"}}
        ]}});
        let hit = pick_album(&body, "Björk", "Moon").unwrap();
        assert_eq!(hit.id, "plain");
        assert_eq!(hit.image.as_deref(), Some("S2"));
    }

    #[test]
    fn falls_back_to_a_prefix_and_reads_numeric_ids() {
        let body = json!({"albums": {"items": [
            {"id": 12345, "title": "Moon (Deluxe Edition)", "artist": {"name": "Björk"},
             "image": {"large": "L1"}}
        ]}});
        let hit = pick_album(&body, "Bjork", "Moon").unwrap();
        assert_eq!(hit.id, "12345");
        assert_eq!(hit.image.as_deref(), Some("L1"));
    }

    #[test]
    fn only_an_app_id_complaint_stops_matching() {
        use reqwest::StatusCode as S;
        assert!(blames_app_id(
            S::BAD_REQUEST,
            "Invalid or missing app_id parameter"
        ));
        assert!(blames_app_id(S::UNAUTHORIZED, "whatever"));
        assert!(!blames_app_id(S::BAD_REQUEST, "Invalid argument: query"));
    }

    #[test]
    fn a_longer_title_is_not_an_edition() {
        let body = json!({"albums": {"items": [
            {"id": "safari", "title": "Moon Safari", "artist": {"name": "Air"}}
        ]}});
        assert_eq!(pick_album(&body, "Air", "Moon"), None);
    }

    #[test]
    fn no_match_for_another_artist_or_title() {
        let body = json!({"albums": {"items": [
            {"id": "x", "title": "Sun", "artist": {"name": "Björk"}},
            {"id": "y", "title": "Moon", "artist": {"name": "Someone"}}
        ]}});
        assert_eq!(pick_album(&body, "Björk", "Moon"), None);
        assert_eq!(pick_album(&json!({}), "Björk", "Moon"), None);
    }
}
