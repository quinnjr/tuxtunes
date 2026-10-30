//! MusicBrainz artist lookup: one search request per artist, genres
//! taken from the matched artist's tags filtered through MusicBrainz's
//! own genre list. Behind [`GenreLookup`] so `resolve` is testable
//! without a network.

use crate::library::pacer::Pacer;
use serde::Deserialize;
use std::collections::{HashMap, HashSet};
use std::future::Future;
use std::path::Path;
use std::time::Duration;

pub const API_ROOT: &str = "https://musicbrainz.org/ws/2";
/// MusicBrainz asks for at most one request per second per client.
pub const MIN_SPACING: Duration = Duration::from_millis(1100);
/// A search hit below this score is a different artist.
pub const MIN_SCORE: u32 = 90;
const USER_AGENT: &str = concat!(
    "TuxTunes/",
    env!("CARGO_PKG_VERSION"),
    " (https://github.com/quinnjr/tuxtunes)"
);

/// One matched artist with its genre tags, highest vote count first.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MbHit {
    pub mbid: String,
    pub name: String,
    pub genres: Vec<(String, u32)>,
}

pub trait GenreLookup {
    /// `Ok(None)` when MusicBrainz has no artist by that name.
    fn lookup(&self, artist: &str) -> impl Future<Output = anyhow::Result<Option<MbHit>>> + Send;
}

#[derive(Debug, Deserialize)]
struct SearchBody {
    #[serde(default)]
    artists: Vec<SearchArtist>,
}

#[derive(Debug, Deserialize)]
struct SearchArtist {
    id: String,
    name: String,
    #[serde(default)]
    score: u32,
    #[serde(default)]
    tags: Vec<Tag>,
}

#[derive(Debug, Deserialize)]
struct Tag {
    name: String,
    #[serde(default)]
    count: i64,
}

/// Pick the artist a search body names, or `None`. The first result
/// scoring at least [`MIN_SCORE`] whose name matches the query after
/// folding wins; its tags are kept only when they are MusicBrainz
/// genres, ordered by vote count.
pub fn pick_hit(query: &str, body: &serde_json::Value, genres: &HashSet<String>) -> Option<MbHit> {
    let parsed: SearchBody = serde_json::from_value(body.clone()).ok()?;
    let want = fold(query);
    let hit = parsed
        .artists
        .into_iter()
        .find(|a| a.score >= MIN_SCORE && fold(&a.name) == want)?;
    let mut tags: HashMap<String, u32> = HashMap::new();
    for t in hit.tags {
        let name = t.name.trim().to_lowercase();
        if t.count > 0 && genres.contains(&name) {
            *tags.entry(name).or_default() += t.count as u32;
        }
    }
    let mut genres: Vec<(String, u32)> = tags.into_iter().collect();
    genres.sort_by(|a, b| b.1.cmp(&a.1).then_with(|| a.0.cmp(&b.0)));
    Some(MbHit {
        mbid: hit.id,
        name: hit.name,
        genres,
    })
}

/// Case-, diacritic- and punctuation-insensitive artist name key.
/// "Mötley Crüe" and "motley crue" fold to the same string, as do
/// "Simon & Garfunkel" and "Simon and Garfunkel".
pub fn fold(name: &str) -> String {
    let mut out = String::with_capacity(name.len());
    let lower = name.trim().to_lowercase().replace('&', " and ");
    for c in lower.chars() {
        match strip_diacritic(c) {
            Some(s) => out.push_str(s),
            None if c.is_alphanumeric() => out.push(c),
            None => {}
        }
    }
    out
}

pub fn names_match(a: &str, b: &str) -> bool {
    fold(a) == fold(b)
}

fn strip_diacritic(c: char) -> Option<&'static str> {
    Some(match c {
        'à' | 'á' | 'â' | 'ã' | 'ä' | 'å' | 'ā' => "a",
        'æ' => "ae",
        'ç' | 'č' | 'ć' => "c",
        'è' | 'é' | 'ê' | 'ë' | 'ē' | 'ę' => "e",
        'ì' | 'í' | 'î' | 'ï' | 'ī' => "i",
        'ñ' | 'ń' => "n",
        'ò' | 'ó' | 'ô' | 'õ' | 'ö' | 'ø' | 'ō' => "o",
        'œ' => "oe",
        'ù' | 'ú' | 'û' | 'ü' | 'ū' => "u",
        'ý' | 'ÿ' => "y",
        'š' | 'ś' => "s",
        'ž' | 'ź' | 'ż' => "z",
        'ł' => "l",
        'ß' => "ss",
        'đ' => "d",
        _ => return None,
    })
}

/// Escape a value for a Lucene phrase query: backslash the characters
/// Lucene treats as syntax, then wrap in double quotes.
pub fn lucene_phrase(s: &str) -> String {
    let mut out = String::with_capacity(s.len() + 2);
    out.push('"');
    for c in s.chars() {
        if matches!(
            c,
            '+' | '-'
                | '!'
                | '('
                | ')'
                | '{'
                | '}'
                | '['
                | ']'
                | '^'
                | '"'
                | '~'
                | '*'
                | '?'
                | ':'
                | '\\'
                | '/'
                | '&'
                | '|'
        ) {
            out.push('\\');
        }
        out.push(c);
    }
    out.push('"');
    out
}

/// Live client. Serialises requests through a mutex-guarded timestamp
/// so concurrent callers still respect [`MIN_SPACING`].
pub struct MusicBrainz {
    http: reqwest::Client,
    genres: HashSet<String>,
    pacer: Pacer,
}

impl MusicBrainz {
    pub fn new(genres: HashSet<String>) -> anyhow::Result<Self> {
        let http = reqwest::Client::builder()
            .user_agent(USER_AGENT)
            .timeout(Duration::from_secs(30))
            .build()?;
        Ok(Self {
            http,
            genres,
            pacer: Pacer::new(MIN_SPACING),
        })
    }

    /// The MusicBrainz genre list (lowercase names), read from
    /// `cache_path` when present and fetched (then cached) otherwise.
    pub async fn genre_list(cache_path: &Path) -> anyhow::Result<HashSet<String>> {
        if let Ok(text) = std::fs::read_to_string(cache_path) {
            let set = parse_genre_list(&text);
            if !set.is_empty() {
                return Ok(set);
            }
        }
        let http = reqwest::Client::builder().user_agent(USER_AGENT).build()?;
        let text = http
            .get(format!("{API_ROOT}/genre/all?fmt=txt"))
            .send()
            .await?
            .error_for_status()?
            .text()
            .await?;
        let set = parse_genre_list(&text);
        anyhow::ensure!(!set.is_empty(), "MusicBrainz genre list came back empty");
        if let Some(dir) = cache_path.parent() {
            std::fs::create_dir_all(dir)?;
        }
        std::fs::write(cache_path, &text)?;
        Ok(set)
    }

    async fn search(&self, artist: &str) -> anyhow::Result<serde_json::Value> {
        let query = format!("artist:{}", lucene_phrase(artist));
        self.get(
            "artist/",
            &[("query", query.as_str()), ("fmt", "json"), ("limit", "5")],
            artist,
        )
        .await
    }

    /// The artist search hit for `artist`, genres or not. The release
    /// feed only needs the MBID, so an artist with no genre tags still
    /// counts as found here.
    pub async fn find_artist(&self, artist: &str) -> anyhow::Result<Option<MbHit>> {
        let body = self.search(artist).await?;
        Ok(pick_hit(artist, &body, &self.genres))
    }

    /// GET `{API_ROOT}/{path}` with `query`, paced and retried through
    /// MusicBrainz's load shedding. `what` names the request in logs.
    pub async fn get(
        &self,
        path: &str,
        query: &[(&str, &str)],
        what: &str,
    ) -> anyhow::Result<serde_json::Value> {
        let url = format!("{API_ROOT}/{path}");
        // MusicBrainz sheds load with a cheap "busy" reply that, under
        // load, hits about half of all requests regardless of pacing.
        // Retry at the normal request pace for a while (a 503 costs
        // half a second and the pacer already spaces the retry), and only
        // back off (5s, 10s, … 60s) once a run of them suggests a real
        // outage.
        const ATTEMPTS: u32 = 24;
        let backoff = |attempt: u32| {
            if attempt <= 12 {
                Duration::ZERO
            } else {
                Duration::from_secs((5u64 << (attempt - 13)).min(60))
            }
        };
        for attempt in 1..=ATTEMPTS {
            self.pacer.wait().await;
            let resp = match self.http.get(&url).query(query).send().await {
                Ok(r) => r,
                Err(e) if attempt < ATTEMPTS => {
                    // Timeouts and connection resets are as transient as
                    // a "busy" reply over a run this long.
                    log::warn!("musicbrainz: request failed for {what:?}: {e}");
                    tokio::time::sleep(backoff(attempt).max(Duration::from_secs(2))).await;
                    continue;
                }
                Err(e) => return Err(e.into()),
            };
            let status = resp.status();
            // A rejected request (a query too long, a bad parameter)
            // fails the same way every time; retrying it would only
            // spend the whole budget before failing anyway.
            if is_rejection(status) {
                let text = resp.text().await.unwrap_or_default();
                let detail = serde_json::from_str::<serde_json::Value>(&text)
                    .ok()
                    .and_then(|b| b.get("error")?.as_str().map(str::to_string))
                    .unwrap_or_else(|| text.chars().take(200).collect());
                return Err(Rejected(format!(
                    "musicbrainz rejected the request for {what:?} ({status}): {detail}"
                ))
                .into());
            }
            let body: serde_json::Value = match resp.json().await {
                Ok(v) => v,
                Err(e) if attempt < ATTEMPTS => {
                    log::warn!("musicbrainz: unreadable body for {what:?}: {e}");
                    tokio::time::sleep(backoff(attempt)).await;
                    continue;
                }
                Err(e) => return Err(e.into()),
            };
            let error = body.get("error").and_then(|e| e.as_str());
            if is_busy(status, error) {
                if attempt < ATTEMPTS {
                    tokio::time::sleep(backoff(attempt)).await;
                    continue;
                }
                anyhow::bail!(
                    "musicbrainz unavailable after {ATTEMPTS} attempts: {}",
                    error.unwrap_or(status.as_str())
                );
            }
            if let Some(error) = error {
                return Err(Rejected(format!(
                    "musicbrainz returned an error for {what:?}: {error}"
                ))
                .into());
            }
            if !status.is_success() {
                anyhow::bail!("musicbrainz returned {status} for {what:?}");
            }
            return Ok(body);
        }
        unreachable!("every attempt either returns or continues")
    }
}

/// MusicBrainz refused the request itself (a 4xx, or an error body
/// that is not load shedding). Asking again will get the same answer, so
/// callers can treat it as a settled miss instead of a transient
/// failure. Recover it with `anyhow::Error::downcast_ref`.
#[derive(Debug)]
pub struct Rejected(pub String);

impl std::fmt::Display for Rejected {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for Rejected {}

/// A 4xx other than 429: the request itself is wrong, so it is not
/// retried.
fn is_rejection(status: reqwest::StatusCode) -> bool {
    status.is_client_error() && status != reqwest::StatusCode::TOO_MANY_REQUESTS
}

/// Worth retrying: MusicBrainz sheds load as `{"error": "... busy ..."}`,
/// sometimes with a 200 and sometimes with a 503 (or a 429 when rate
/// limiting), and any other 5xx is a server fault that passes. An error
/// body that reads like load shedding ("rate limit", "try again") is
/// retried whatever its status; only any other error on a 2xx is a real
/// refusal.
fn is_busy(status: reqwest::StatusCode, error: Option<&str>) -> bool {
    const SHEDDING: &[&str] = &["busy", "rate limit", "try again", "too many"];
    status.is_server_error()
        || status == reqwest::StatusCode::TOO_MANY_REQUESTS
        || error.is_some_and(|e| {
            let e = e.to_lowercase();
            SHEDDING.iter().any(|w| e.contains(w))
        })
}

pub fn parse_genre_list(text: &str) -> HashSet<String> {
    text.lines()
        .map(|l| l.trim().to_lowercase())
        .filter(|l| !l.is_empty())
        .collect()
}

impl GenreLookup for MusicBrainz {
    async fn lookup(&self, artist: &str) -> anyhow::Result<Option<MbHit>> {
        self.find_artist(artist).await
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn only_overload_replies_are_retried() {
        use reqwest::StatusCode as S;
        assert!(is_busy(S::SERVICE_UNAVAILABLE, None));
        assert!(is_busy(S::TOO_MANY_REQUESTS, None));
        assert!(is_busy(
            S::OK,
            Some("The MusicBrainz web server is currently busy.")
        ));
        assert!(!is_busy(S::OK, Some("Invalid query")));
        assert!(is_busy(
            S::OK,
            Some("Your requests are exceeding the allowable rate limit.")
        ));
        assert!(is_busy(S::OK, Some("Please try again later")));
        assert!(!is_busy(S::BAD_REQUEST, Some("Invalid query")));
        assert!(is_busy(
            S::INTERNAL_SERVER_ERROR,
            Some("Internal Server Error")
        ));
        assert!(is_busy(S::BAD_GATEWAY, None));
        assert!(is_rejection(S::BAD_REQUEST));
        assert!(is_rejection(S::NOT_FOUND));
        assert!(!is_rejection(S::TOO_MANY_REQUESTS));
        assert!(!is_rejection(S::SERVICE_UNAVAILABLE));
    }

    fn genres() -> HashSet<String> {
        ["metal", "thrash metal", "heavy metal", "rock"]
            .iter()
            .map(|s| s.to_string())
            .collect()
    }

    #[test]
    fn picks_first_high_scoring_exact_name_and_orders_genres() {
        let body = json!({"artists": [
            {"id": "x1", "name": "Metallica Tribute", "score": 100, "tags": [{"name":"metal","count":9}]},
            {"id": "65f4f0c5", "name": "Metallica", "score": 100, "tags": [
                {"name": "heavy metal", "count": 12},
                {"name": "thrash metal", "count": 20},
                {"name": "american", "count": 4},
                {"name": "rock", "count": 0},
                {"name": "Metal", "count": 20}
            ]}
        ]});
        let hit = pick_hit("metallica", &body, &genres()).unwrap();
        assert_eq!(hit.mbid, "65f4f0c5");
        assert_eq!(hit.name, "Metallica");
        assert_eq!(
            hit.genres,
            vec![
                ("metal".to_string(), 20),
                ("thrash metal".to_string(), 20),
                ("heavy metal".to_string(), 12)
            ]
        );
    }

    #[test]
    fn low_score_or_different_name_is_a_miss() {
        let body = json!({"artists": [
            {"id": "a", "name": "Nero", "score": 80, "tags": [{"name":"rock","count":1}]},
            {"id": "b", "name": "Nero Reborn", "score": 95, "tags": [{"name":"rock","count":1}]}
        ]});
        assert_eq!(pick_hit("Nero", &body, &genres()), None);
        assert_eq!(pick_hit("Nero", &json!({"artists": []}), &genres()), None);
        assert_eq!(pick_hit("Nero", &json!({}), &genres()), None);
    }

    #[test]
    fn name_folding_ignores_case_diacritics_and_punctuation() {
        assert!(names_match("Mötley Crüe", "motley crue"));
        assert!(names_match("Simon & Garfunkel", "Simon and Garfunkel"));
        assert!(names_match("blink-182", "Blink 182"));
        assert!(names_match("Sigur Rós", "sigur ros"));
        assert!(!names_match("Nero", "Nero Reborn"));
    }

    #[test]
    fn lucene_phrase_escapes_syntax() {
        assert_eq!(lucene_phrase("AC/DC"), r#""AC\/DC""#);
        assert_eq!(lucene_phrase(r#"Say "Hi""#), r#""Say \"Hi\"""#);
        assert_eq!(
            lucene_phrase("Panic! At the Disco"),
            r#""Panic\! At the Disco""#
        );
    }

    #[test]
    fn genre_list_parses_lowercase_nonempty_lines() {
        let set = parse_genre_list("Metal\n\n Thrash Metal \nrock\n");
        assert_eq!(
            set,
            genres()
                .into_iter()
                .filter(|g| g != "heavy metal")
                .collect()
        );
    }
}
