//! Step two: push the resolved genres onto tracks (database rows and
//! the files' own tags).

use super::map::{ArtistGenre, GenreMap};
use super::resolve::{fold_key, is_non_artist, ARTIST_FOLD_SQL};
use super::taxonomy::normalize_tag;
use crate::db::tracks::set_genres_locked;
use crate::fs::tags::{write_genre, TagsError};
use prax_sqlite::raw::SqliteRawEngine;
use std::collections::{BTreeMap, HashMap};
use std::path::PathBuf;

#[derive(Debug, Clone, Copy)]
pub struct ApplyOpts {
    /// Also rewrite the genre tag inside each changed file.
    pub write_tags: bool,
    /// Compute and report, touch nothing.
    pub dry_run: bool,
}

impl Default for ApplyOpts {
    fn default() -> Self {
        Self {
            write_tags: true,
            dry_run: false,
        }
    }
}

#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct ApplySummary {
    /// Tracks whose target genre differs from what the row holds.
    pub planned: u64,
    pub db_updated: u64,
    pub tags_written: u64,
    pub tags_skipped_missing: u64,
    pub tags_failed: Vec<String>,
    /// Planned changes per target genre.
    pub by_genre: BTreeMap<String, u64>,
}

/// The map re-indexed by artist fold, for per-track lookups.
pub fn by_fold(map: &GenreMap) -> HashMap<String, &ArtistGenre> {
    map.artists.iter().map(|(k, v)| (fold_key(k), v)).collect()
}

/// The genre a track should carry: the artist's resolved genre when
/// there is one, otherwise its own tag normalised, otherwise `None`
/// (leave the row alone).
pub fn target_genre(
    folded: &HashMap<String, &ArtistGenre>,
    fold: &str,
    existing: Option<&str>,
) -> Option<String> {
    if !is_non_artist(fold) {
        if let Some(entry) = folded.get(fold) {
            if entry.is_resolved() {
                return Some(entry.genre.clone());
            }
        }
    }
    existing.and_then(normalize_tag)
}

#[derive(Debug)]
struct Planned {
    id: i64,
    path: PathBuf,
    genre: String,
}

/// Apply the map to every track. `progress` receives `(done, total)`
/// during the tag-writing phase, which dominates the run time.
pub async fn apply(
    engine: &SqliteRawEngine,
    map: &GenreMap,
    opts: ApplyOpts,
    mut progress: impl FnMut(u64, u64),
) -> anyhow::Result<ApplySummary> {
    let folded = by_fold(map);
    let sql =
        format!("SELECT id, {ARTIST_FOLD_SQL} AS f, genre, file_path FROM tracks ORDER BY id");
    let rows = engine.raw_sql_query(&sql, &[]).await?;
    let mut summary = ApplySummary::default();
    let mut planned = Vec::new();
    for r in rows {
        let j = r.into_json();
        let id = j.get("id").and_then(|v| v.as_i64()).unwrap_or_default();
        let fold = j.get("f").and_then(|v| v.as_str()).unwrap_or("");
        let existing = j.get("genre").and_then(|v| v.as_str());
        let path = j.get("file_path").and_then(|v| v.as_str()).unwrap_or("");
        let Some(genre) = target_genre(&folded, fold, existing) else {
            continue;
        };
        // Exact comparison on purpose: a padded "Metalcore " must be
        // rewritten, or the "All <Genre>" rules (plain equality) miss it.
        if existing == Some(genre.as_str()) {
            continue;
        }
        *summary.by_genre.entry(genre.clone()).or_default() += 1;
        summary.planned += 1;
        planned.push(Planned {
            id,
            path: PathBuf::from(path),
            genre,
        });
    }
    if opts.dry_run {
        return Ok(summary);
    }

    let changes: Vec<(i64, &str)> = planned.iter().map(|p| (p.id, p.genre.as_str())).collect();
    summary.db_updated = set_genres_locked(engine, &changes).await?;

    if opts.write_tags {
        let total = planned.len() as u64;
        // Tag writes are blocking file IO; batch them onto the blocking
        // pool so progress still reaches the caller between batches.
        const BATCH: usize = 64;
        for (i, chunk) in planned.chunks(BATCH).enumerate() {
            let jobs: Vec<(PathBuf, String)> = chunk
                .iter()
                .map(|p| (p.path.clone(), p.genre.clone()))
                .collect();
            let results = tokio::task::spawn_blocking(move || {
                jobs.into_iter()
                    .map(|(path, genre)| (path.clone(), write_genre(&path, &genre)))
                    .collect::<Vec<_>>()
            })
            .await?;
            for (path, res) in results {
                match res {
                    Ok(()) => summary.tags_written += 1,
                    Err(TagsError::NotFound(_)) => summary.tags_skipped_missing += 1,
                    Err(e) => summary.tags_failed.push(format!("{}: {e}", path.display())),
                }
            }
            progress(((i + 1) * BATCH).min(planned.len()) as u64, total);
        }
    }
    Ok(summary)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::Db;
    use crate::library::genres::map::Source;
    use prax_query::filter::FilterValue as FV;

    async fn tmp_db() -> Db {
        let tmp = tempfile::NamedTempFile::new().unwrap();
        Db::open(tmp.path()).await.unwrap()
    }

    async fn insert(db: &Db, artist: &str, genre: Option<&str>, path: &str) -> i64 {
        db.engine
            .raw_sql_scalar(
                "INSERT INTO tracks (title, artist, genre, duration_ms, size_bytes, file_path, \
                 playlist_ids) VALUES ('t', ?, ?, 1000, 0, ?, '[]') RETURNING id",
                &[
                    FV::String(artist.to_string()),
                    genre.map(|s| FV::String(s.to_string())).unwrap_or(FV::Null),
                    FV::String(path.to_string()),
                ],
            )
            .await
            .unwrap()
    }

    async fn row(db: &Db, id: i64) -> (Option<String>, i64) {
        let j = db
            .engine
            .raw_sql_first(
                "SELECT genre, genre_locked FROM tracks WHERE id = ?",
                &[FV::Int(id)],
            )
            .await
            .unwrap()
            .into_json();
        (
            j["genre"].as_str().map(str::to_string),
            j["genre_locked"].as_i64().unwrap(),
        )
    }

    fn map() -> GenreMap {
        let mut m = GenreMap::default();
        m.artists.insert(
            "Band".into(),
            ArtistGenre::new("Metalcore", Source::Musicbrainz, 2),
        );
        m.artists
            .insert("Nobody".into(), ArtistGenre::unresolved(1));
        m
    }

    #[test]
    fn target_genre_prefers_artist_then_normalized_tag() {
        let m = map();
        let f = by_fold(&m);
        assert_eq!(
            target_genre(&f, "band", Some("Alternative")).as_deref(),
            Some("Metalcore")
        );
        assert_eq!(
            target_genre(&f, "nobody", Some("JPop")).as_deref(),
            Some("J-Pop")
        );
        assert_eq!(target_genre(&f, "nobody", Some("145")), None);
        assert_eq!(target_genre(&f, "nobody", None), None);
        assert_eq!(
            target_genre(&f, "various artists", Some("rock")).as_deref(),
            Some("Rock")
        );
        assert_eq!(
            target_genre(&f, "unmapped", Some("Rock")).as_deref(),
            Some("Rock")
        );
    }

    fn write_minimal_wav(path: &std::path::Path) {
        let bytes: &[u8] = &[
            b'R', b'I', b'F', b'F', 0x26, 0, 0, 0, b'W', b'A', b'V', b'E', b'f', b'm', b't', b' ',
            0x10, 0, 0, 0, 0x01, 0, 0x01, 0, 0x40, 0x1f, 0, 0, 0x40, 0x1f, 0, 0, 0x01, 0, 0x08, 0,
            b'd', b'a', b't', b'a', 0x02, 0, 0, 0, 0x80, 0x80,
        ];
        std::fs::write(path, bytes).unwrap();
    }

    #[tokio::test]
    async fn apply_updates_rows_and_tags_and_reports() {
        let db = tmp_db().await;
        let dir = tempfile::tempdir().unwrap();
        let wav = dir.path().join("a.wav");
        write_minimal_wav(&wav);
        let a = insert(&db, "Band", Some("Alternative"), wav.to_str().unwrap()).await;
        let b = insert(&db, "BAND ", Some("Metalcore"), "/tmp/already.flac").await;
        let c = insert(&db, "Nobody", Some("JPop"), "/tmp/missing.flac").await;
        let d = insert(&db, "Nobody", Some("145"), "/tmp/junk.flac").await;
        let e = insert(&db, "Nobody", Some("Trance "), "/tmp/padded.flac").await;

        let dry = apply(
            &db.engine,
            &map(),
            ApplyOpts {
                write_tags: true,
                dry_run: true,
            },
            |_, _| {},
        )
        .await
        .unwrap();
        assert_eq!(dry.planned, 3);
        assert_eq!(dry.db_updated, 0);
        assert_eq!(row(&db, a).await, (Some("Alternative".into()), 0));

        let mut ticks = 0;
        let s = apply(&db.engine, &map(), ApplyOpts::default(), |_, _| ticks += 1)
            .await
            .unwrap();
        assert_eq!(s.planned, 3);
        assert_eq!(s.db_updated, 3);
        assert_eq!(s.tags_written, 1);
        assert_eq!(s.tags_skipped_missing, 2);
        assert!(s.tags_failed.is_empty());
        assert_eq!(s.by_genre["Metalcore"], 1);
        assert_eq!(s.by_genre["J-Pop"], 1);
        assert_eq!(s.by_genre["Trance"], 1);
        assert_eq!(ticks, 1);
        assert_eq!(row(&db, a).await, (Some("Metalcore".into()), 1));
        assert_eq!(
            row(&db, b).await,
            (Some("Metalcore".into()), 0),
            "case/padding variant of the artist still maps, and an exact row is untouched"
        );
        assert_eq!(row(&db, c).await, (Some("J-Pop".into()), 1));
        assert_eq!(row(&db, d).await, (Some("145".into()), 0));
        assert_eq!(row(&db, e).await, (Some("Trance".into()), 1));
        let tag = lofty::read_from_path(&wav).unwrap();
        use lofty::file::TaggedFileExt;
        use lofty::tag::Accessor;
        assert_eq!(
            tag.primary_tag().unwrap().genre().as_deref(),
            Some("Metalcore")
        );

        // Second run is a no-op.
        let again = apply(&db.engine, &map(), ApplyOpts::default(), |_, _| {})
            .await
            .unwrap();
        assert_eq!(again.planned, 0);
    }
}
