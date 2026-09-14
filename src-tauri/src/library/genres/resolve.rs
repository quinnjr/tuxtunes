//! Step one: decide a genre for every artist in the library and record
//! it in the [`GenreMap`].
//!
//! Artists are grouped by a *fold* of their name — album artist when
//! present, else artist, trimmed and ASCII-lowercased — so "ABBA",
//! "abba" and "ABBA " are one artist. The map is keyed by the most
//! common trimmed spelling, and `rebuild` matches every raw spelling
//! that folds to it.

use super::map::{ArtistGenre, GenreMap, Source};
use super::musicbrainz::GenreLookup;
use super::taxonomy::{canonical_genre, normalize_tag, umbrella_for, Umbrella};
use prax_sqlite::raw::SqliteRawEngine;
use std::collections::BTreeMap;

/// Artist keys that are not one artist and must never be looked up.
pub const NON_ARTIST_KEYS: &[&str] = &["", "various artists", "various", "va", "unknown artist"];

/// The trimmed display spelling of an artist key: album artist, falling
/// back to artist.
pub const ARTIST_KEY_SQL: &str = "TRIM(COALESCE(NULLIF(TRIM(album_artist), ''), artist, ''))";

/// The grouping fold of an artist key. SQLite's `LOWER` is ASCII-only;
/// [`fold_key`] mirrors that exactly so both sides agree.
pub const ARTIST_FOLD_SQL: &str =
    "LOWER(TRIM(COALESCE(NULLIF(TRIM(album_artist), ''), artist, '')))";

/// Rust twin of [`ARTIST_FOLD_SQL`].
pub fn fold_key(s: &str) -> String {
    s.trim().to_ascii_lowercase()
}

/// Everything `resolve` needs to know about one artist.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ArtistStats {
    /// Most common trimmed spelling; the map key.
    pub key: String,
    pub fold: String,
    pub track_count: u64,
    /// Raw existing genre tags with per-tag track counts.
    pub tags: Vec<(String, u64)>,
}

/// One GROUP BY over the whole library, folded per artist.
pub async fn artist_stats(engine: &SqliteRawEngine) -> anyhow::Result<Vec<ArtistStats>> {
    let sql = format!(
        "SELECT {ARTIST_FOLD_SQL} AS f, {ARTIST_KEY_SQL} AS k, COALESCE(genre, '') AS g, \
         COUNT(*) AS c FROM tracks GROUP BY f, k, g ORDER BY f, c DESC"
    );
    let rows = engine.raw_sql_query(&sql, &[]).await?;
    struct Acc {
        spellings: BTreeMap<String, u64>,
        track_count: u64,
        tags: BTreeMap<String, u64>,
    }
    let mut by_fold: BTreeMap<String, Acc> = BTreeMap::new();
    for r in rows {
        let j = r.into_json();
        let fold = j
            .get("f")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string();
        let key = j
            .get("k")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string();
        let tag = j
            .get("g")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string();
        let count = j.get("c").and_then(|v| v.as_u64()).unwrap_or(0);
        let acc = by_fold.entry(fold).or_insert_with(|| Acc {
            spellings: BTreeMap::new(),
            track_count: 0,
            tags: BTreeMap::new(),
        });
        *acc.spellings.entry(key).or_default() += count;
        acc.track_count += count;
        if !tag.is_empty() {
            *acc.tags.entry(tag).or_default() += count;
        }
    }
    Ok(by_fold
        .into_iter()
        .map(|(fold, acc)| {
            let key = acc
                .spellings
                .iter()
                .max_by(|a, b| a.1.cmp(b.1).then_with(|| b.0.cmp(a.0)))
                .map(|(k, _)| k.clone())
                .unwrap_or_default();
            let mut tags: Vec<(String, u64)> = acc.tags.into_iter().collect();
            tags.sort_by(|a, b| b.1.cmp(&a.1).then_with(|| a.0.cmp(&b.0)));
            ArtistStats {
                key,
                fold,
                track_count: acc.track_count,
                tags,
            }
        })
        .collect())
}

/// The most common normalized tag, junk excluded. Ties break toward
/// the alphabetically first name so the result is stable.
pub fn dominant_tag(tags: &[(String, u64)]) -> Option<String> {
    let mut counts: BTreeMap<String, u64> = BTreeMap::new();
    for (raw, n) in tags {
        if let Some(canon) = normalize_tag(raw) {
            *counts.entry(canon).or_default() += n;
        }
    }
    counts
        .into_iter()
        .max_by(|a, b| a.1.cmp(&b.1).then_with(|| b.0.cmp(&a.0)))
        .map(|(g, _)| g)
}

pub fn is_non_artist(key: &str) -> bool {
    NON_ARTIST_KEYS.contains(&fold_key(key).as_str())
}

/// The map key whose fold matches `fold`, if any. Linear over the map,
/// which is a couple of thousand entries at most.
pub fn key_for_fold(map: &GenreMap, fold: &str) -> Option<String> {
    map.artists.keys().find(|k| fold_key(k) == fold).cloned()
}

#[derive(Debug, Clone, Copy, Default)]
pub struct ResolveOpts {
    /// Re-query artists already resolved (from MusicBrainz or tags)
    /// and artists that previously came up unresolved. Manual entries
    /// are never touched.
    pub refresh: bool,
}

#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct ResolveSummary {
    pub looked_up: u64,
    pub musicbrainz: u64,
    pub tags: u64,
    pub unresolved: u64,
    pub skipped: u64,
}

/// How often the map is flushed mid-run so an interrupted resolve
/// resumes where it stopped.
pub const SAVE_EVERY: u64 = 10;

/// Fill `map` for every artist. `save` is called every [`SAVE_EVERY`]
/// lookups and once at the end; `progress` gets `(key, done, total)`
/// before each artist is handled.
pub async fn resolve<L: GenreLookup>(
    engine: &SqliteRawEngine,
    lookup: &L,
    map: &mut GenreMap,
    opts: ResolveOpts,
    mut save: impl FnMut(&GenreMap) -> anyhow::Result<()>,
    mut progress: impl FnMut(&str, usize, usize),
) -> anyhow::Result<ResolveSummary> {
    let stats = artist_stats(engine).await?;
    let total = stats.len();
    let mut summary = ResolveSummary::default();
    let mut since_save = 0u64;
    for (i, a) in stats.iter().enumerate() {
        progress(&a.key, i, total);
        // An entry from an earlier run may sit under another spelling
        // of the same artist; adopt it under the current key.
        if let Some(old_key) = key_for_fold(map, &a.fold) {
            let mut existing = map.artists.remove(&old_key).expect("key came from the map");
            existing.track_count = a.track_count;
            let keep = match existing.source {
                Source::Manual => true,
                Source::Musicbrainz | Source::Tags | Source::Unresolved => !opts.refresh,
            };
            if keep {
                map.artists.insert(a.key.clone(), existing);
                summary.skipped += 1;
                continue;
            }
        }
        let fallback = dominant_tag(&a.tags);
        let entry = if is_non_artist(&a.key) {
            ArtistGenre::unresolved(a.track_count)
        } else if fallback
            .as_deref()
            .is_some_and(|g| umbrella_for(g) == Umbrella::Soundtrack)
        {
            // Soundtrack "artists" are album titles; MusicBrainz would
            // either miss or match the wrong thing.
            ArtistGenre::new(fallback.clone().unwrap(), Source::Tags, a.track_count)
        } else {
            summary.looked_up += 1;
            since_save += 1;
            match lookup.lookup(&a.key).await? {
                Some(hit) if !hit.genres.is_empty() => {
                    let mut e = ArtistGenre::new(
                        canonical_genre(&hit.genres[0].0),
                        Source::Musicbrainz,
                        a.track_count,
                    );
                    e.mbid = Some(hit.mbid);
                    e
                }
                _ => match &fallback {
                    Some(g) => ArtistGenre::new(g.clone(), Source::Tags, a.track_count),
                    None => ArtistGenre::unresolved(a.track_count),
                },
            }
        };
        match entry.source {
            Source::Musicbrainz => summary.musicbrainz += 1,
            Source::Tags => summary.tags += 1,
            Source::Unresolved => summary.unresolved += 1,
            Source::Manual => {}
        }
        map.artists.insert(a.key.clone(), entry);
        if since_save >= SAVE_EVERY {
            save(map)?;
            since_save = 0;
        }
    }
    save(map)?;
    Ok(summary)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::Db;
    use crate::library::genres::musicbrainz::MbHit;
    use prax_query::filter::FilterValue as FV;
    use std::collections::HashMap;
    use std::sync::Mutex;

    async fn tmp_db() -> Db {
        let tmp = tempfile::NamedTempFile::new().unwrap();
        Db::open(tmp.path()).await.unwrap()
    }

    async fn insert(db: &Db, artist: &str, album_artist: Option<&str>, genre: Option<&str>) {
        static N: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
        let n = N.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        db.engine
            .raw_sql_execute(
                "INSERT INTO tracks (title, artist, album_artist, genre, duration_ms, size_bytes, \
                 file_path, playlist_ids) VALUES (?, ?, ?, ?, 1000, 0, ?, '[]')",
                &[
                    FV::String(format!("t{n}")),
                    FV::String(artist.to_string()),
                    album_artist
                        .map(|s| FV::String(s.to_string()))
                        .unwrap_or(FV::Null),
                    genre.map(|s| FV::String(s.to_string())).unwrap_or(FV::Null),
                    FV::String(format!("/tmp/t{n}.flac")),
                ],
            )
            .await
            .unwrap();
    }

    struct Fake {
        hits: HashMap<String, MbHit>,
        calls: Mutex<Vec<String>>,
    }

    impl Fake {
        fn new(hits: Vec<(&str, MbHit)>) -> Self {
            Self {
                hits: hits.into_iter().map(|(k, v)| (k.to_string(), v)).collect(),
                calls: Mutex::new(Vec::new()),
            }
        }
    }

    impl GenreLookup for Fake {
        async fn lookup(&self, artist: &str) -> anyhow::Result<Option<MbHit>> {
            self.calls.lock().unwrap().push(artist.to_string());
            Ok(self.hits.get(artist).cloned())
        }
    }

    fn hit(genres: &[(&str, u32)]) -> MbHit {
        MbHit {
            mbid: "mbid".into(),
            name: "x".into(),
            genres: genres.iter().map(|(g, c)| (g.to_string(), *c)).collect(),
        }
    }

    #[tokio::test]
    async fn artist_stats_folds_case_and_padding_and_prefers_album_artist() {
        let db = tmp_db().await;
        insert(&db, "Skream feat. X", Some("Skream"), Some("Dubstep")).await;
        insert(&db, "skream", None, Some("Dubstep")).await;
        insert(&db, "Skream ", None, Some("Electronic")).await;
        insert(&db, "SKREAM", Some(" "), Some("Electronic")).await;
        insert(&db, "  ", Some(""), None).await;
        let stats = artist_stats(&db.engine).await.unwrap();
        assert_eq!(stats.len(), 2);
        assert_eq!(stats[0].key, "");
        assert_eq!(stats[0].track_count, 1);
        assert_eq!(stats[1].key, "Skream", "most common trimmed spelling wins");
        assert_eq!(stats[1].fold, "skream");
        assert_eq!(stats[1].track_count, 4);
        assert_eq!(
            stats[1].tags,
            vec![("Dubstep".to_string(), 2), ("Electronic".to_string(), 2)]
        );
    }

    #[test]
    fn dominant_tag_normalizes_and_ignores_junk() {
        let tags = vec![
            ("JPop".to_string(), 3),
            ("J-Pop".to_string(), 3),
            ("Rock".to_string(), 5),
            ("atrilli.net".to_string(), 100),
        ];
        assert_eq!(dominant_tag(&tags).as_deref(), Some("J-Pop"));
        assert_eq!(dominant_tag(&[("145".to_string(), 9)]), None);
    }

    #[test]
    fn fold_is_ascii_only_like_sqlite_lower() {
        assert_eq!(fold_key("  ABBA "), "abba");
        assert_eq!(fold_key("Röyksopp"), "röyksopp");
        assert!(is_non_artist(" VARIOUS ARTISTS "));
    }

    #[tokio::test]
    async fn resolve_covers_hit_miss_soundtrack_manual_and_resume() {
        let db = tmp_db().await;
        insert(&db, "Asking Alexandria", None, Some("Alternative")).await;
        insert(&db, "asking alexandria", None, Some("Alternative")).await;
        insert(&db, "Obscure Band", None, Some("Death Metal/Black Metal")).await;
        insert(&db, "Nothing Known", None, Some("145")).await;
        insert(&db, "Armored Core V", None, Some("Game")).await;
        insert(
            &db,
            "Various Artists",
            Some("Various Artists"),
            Some("Rock"),
        )
        .await;
        insert(&db, "Hand Fixed", None, Some("Pop")).await;
        insert(&db, "Already Done", None, Some("Pop")).await;
        insert(&db, "ALREADY DONE", None, Some("Pop")).await;
        insert(&db, "ALREADY DONE", None, Some("Pop")).await;

        let fake = Fake::new(vec![
            ("Asking Alexandria", hit(&[("metalcore", 10), ("rock", 5)])),
            ("asking alexandria", hit(&[("metalcore", 10), ("rock", 5)])),
            ("Hand Fixed", hit(&[("pop", 1)])),
            ("ALREADY DONE", hit(&[("pop", 1)])),
        ]);
        let mut map = GenreMap::default();
        let mut manual = ArtistGenre::new("Trance", Source::Manual, 0);
        manual.umbrella = "Electronic".into();
        map.artists.insert("Hand Fixed".into(), manual);
        map.artists.insert(
            "Already Done".into(),
            ArtistGenre::new("Old Genre", Source::Musicbrainz, 0),
        );

        let saves = std::cell::Cell::new(0);
        let summary = resolve(
            &db.engine,
            &fake,
            &mut map,
            ResolveOpts::default(),
            |_| {
                saves.set(saves.get() + 1);
                Ok(())
            },
            |_, _, _| {},
        )
        .await
        .unwrap();

        let g = |k: &str| map.artists.get(k).unwrap().clone();
        let aa = map
            .artists
            .keys()
            .filter(|k| fold_key(k) == "asking alexandria")
            .count();
        assert_eq!(aa, 1, "case variants fold into one entry");
        let aa_key = key_for_fold(&map, "asking alexandria").unwrap();
        assert_eq!(g(&aa_key).genre, "Metalcore");
        assert_eq!(g(&aa_key).source, Source::Musicbrainz);
        assert_eq!(g(&aa_key).mbid.as_deref(), Some("mbid"));
        assert_eq!(g(&aa_key).track_count, 2);
        assert_eq!(g(&aa_key).umbrella, "Metal");
        assert_eq!(g("Obscure Band").genre, "Death Metal");
        assert_eq!(g("Obscure Band").source, Source::Tags);
        assert_eq!(g("Nothing Known").source, Source::Unresolved);
        assert_eq!(g("Armored Core V").genre, "Video Game Music");
        assert_eq!(g("Armored Core V").source, Source::Tags);
        assert_eq!(g("Various Artists").source, Source::Unresolved);
        assert_eq!(g("Hand Fixed").genre, "Trance");
        assert_eq!(g("Hand Fixed").source, Source::Manual);
        assert_eq!(
            g("Hand Fixed").track_count,
            1,
            "counts refresh even on kept entries"
        );
        // The old entry was adopted under the now-dominant spelling.
        assert!(!map.artists.contains_key("Already Done"));
        assert_eq!(g("ALREADY DONE").genre, "Old Genre");
        assert_eq!(g("ALREADY DONE").track_count, 3);

        let mut calls = fake.calls.lock().unwrap().clone();
        calls.sort();
        assert_eq!(calls, [aa_key.as_str(), "Nothing Known", "Obscure Band"]);
        assert_eq!(saves.get(), 1);
        assert_eq!(
            summary,
            ResolveSummary {
                looked_up: 3,
                musicbrainz: 1,
                tags: 2,
                unresolved: 2,
                skipped: 2,
            }
        );

        // Without --refresh an unresolved artist stays unresolved and
        // is not queried again.
        fake.calls.lock().unwrap().clear();
        resolve(
            &db.engine,
            &fake,
            &mut map,
            ResolveOpts::default(),
            |_| Ok(()),
            |_, _, _| {},
        )
        .await
        .unwrap();
        assert!(fake.calls.lock().unwrap().is_empty());

        // --refresh re-queries everything except manual entries.
        let summary = resolve(
            &db.engine,
            &fake,
            &mut map,
            ResolveOpts { refresh: true },
            |_| Ok(()),
            |_, _, _| {},
        )
        .await
        .unwrap();
        assert_eq!(map.artists["ALREADY DONE"].genre, "Pop");
        assert_eq!(map.artists["Hand Fixed"].genre, "Trance");
        assert_eq!(summary.skipped, 1);
    }

    #[tokio::test]
    async fn lookup_errors_stop_the_run_and_keep_earlier_results() {
        struct Boom;
        impl GenreLookup for Boom {
            async fn lookup(&self, artist: &str) -> anyhow::Result<Option<MbHit>> {
                if artist == "B" {
                    anyhow::bail!("network down");
                }
                Ok(Some(hit(&[("rock", 1)])))
            }
        }
        let db = tmp_db().await;
        insert(&db, "A", None, None).await;
        insert(&db, "B", None, None).await;
        let mut map = GenreMap::default();
        let err = resolve(
            &db.engine,
            &Boom,
            &mut map,
            ResolveOpts::default(),
            |_| Ok(()),
            |_, _, _| {},
        )
        .await
        .unwrap_err();
        assert!(err.to_string().contains("network down"));
        assert_eq!(map.artists["A"].genre, "Rock");
        assert!(!map.artists.contains_key("B"));
    }
}
