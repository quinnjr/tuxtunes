//! MusicBrainz artist lookup: one search request per artist, genres
//! taken from the matched artist's tags filtered through MusicBrainz's
//! own genre list. Behind [`GenreLookup`] so `resolve` is testable
//! without a network.

use serde::Deserialize;
use std::collections::{HashMap, HashSet};
use std::future::Future;
use std::path::Path;
use std::time::Duration;
use tokio::sync::Mutex;
use tokio::time::Instant;

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
    last_request: Mutex<Option<Instant>>,
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
            last_request: Mutex::new(None),
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

    async fn pace(&self) {
        let mut last = self.last_request.lock().await;
        if let Some(t) = *last {
            let elapsed = t.elapsed();
            if elapsed < MIN_SPACING {
                tokio::time::sleep(MIN_SPACING - elapsed).await;
            }
        }
        *last = Some(Instant::now());
    }

    async fn search(&self, artist: &str) -> anyhow::Result<serde_json::Value> {
        let query = format!("artist:{}", lucene_phrase(artist));
        let url = format!("{API_ROOT}/artist/");
        // MusicBrainz sheds load with a cheap "busy" reply that, under
        // load, hits about half of all requests regardless of pacing.
        // Retry at the normal request pace for a while (a 503 costs
        // half a second and `pace` already spaces the retry), and only
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
            self.pace().await;
            let resp = self
                .http
                .get(&url)
                .query(&[("query", query.as_str()), ("fmt", "json"), ("limit", "5")])
                .send()
                .await?;
            let status = resp.status();
            let body: serde_json::Value = match resp.json().await {
                Ok(v) => v,
                Err(e) if attempt < ATTEMPTS => {
                    log::warn!("musicbrainz: unreadable body for {artist:?}: {e}");
                    tokio::time::sleep(backoff(attempt)).await;
                    continue;
                }
                Err(e) => return Err(e.into()),
            };
            // MusicBrainz reports overload as `{"error": "... busy ..."}`,
            // sometimes with a 200 and sometimes with a 503.
            let busy = body.get("error").and_then(|e| e.as_str());
            if status == reqwest::StatusCode::SERVICE_UNAVAILABLE || busy.is_some() {
                if attempt < ATTEMPTS {
                    tokio::time::sleep(backoff(attempt)).await;
                    continue;
                }
                anyhow::bail!(
                    "musicbrainz unavailable after {ATTEMPTS} attempts: {}",
                    busy.unwrap_or(status.as_str())
                );
            }
            if !status.is_success() {
                anyhow::bail!("musicbrainz returned {status} for {artist:?}");
            }
            return Ok(body);
        }
        unreachable!("every attempt either returns or continues")
    }
}

pub fn parse_genre_list(text: &str) -> HashSet<String> {
    text.lines()
        .map(|l| l.trim().to_lowercase())
        .filter(|l| !l.is_empty())
        .collect()
}

impl GenreLookup for MusicBrainz {
    async fn lookup(&self, artist: &str) -> anyhow::Result<Option<MbHit>> {
        let body = self.search(artist).await?;
        Ok(pick_hit(artist, &body, &self.genres))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

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
