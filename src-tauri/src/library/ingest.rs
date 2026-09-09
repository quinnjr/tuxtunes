//! Probe an audio file with `lofty` and insert a minimal `Track` row.
//!
//! The row's `file_path` starts out pointing at the user-picked source
//! file. Callers that want the file living under the managed library
//! root hand the new id to [`crate::fs::coordinator::FsCoordinator::copy_for_track`],
//! which copies it into place and rewrites `file_path`.

use lofty::file::{AudioFile, TaggedFileExt};
use lofty::probe::Probe;
use lofty::tag::{Accessor, ItemKey};
use prax_query::filter::FilterValue;
use prax_sqlite::raw::SqliteRawEngine;
use std::path::Path;

#[derive(Debug, thiserror::Error)]
pub enum IngestError {
    #[error("failed to probe {path}: {source}")]
    Probe {
        path: String,
        #[source]
        source: lofty::error::LoftyError,
    },

    #[error("path has no file name or stem: {0}")]
    NoFileName(String),

    #[error("db error: {0}")]
    Db(#[source] anyhow::Error),
}

struct ProbeResult {
    title: Option<String>,
    artist: Option<String>,
    album: Option<String>,
    album_artist: Option<String>,
    genre: Option<String>,
    year: Option<i64>,
    track_number: Option<i64>,
    disc_number: Option<i64>,
    duration_ms: i64,
    sample_rate: Option<i64>,
    bit_depth: Option<i64>,
    channels: Option<i64>,
    bit_rate: Option<i64>,
    size_bytes: i64,
}

fn probe_blocking(path: &Path) -> Result<ProbeResult, IngestError> {
    let tagged = Probe::open(path)
        .map_err(|e| IngestError::Probe {
            path: path.display().to_string(),
            source: e,
        })?
        .read()
        .map_err(|e| IngestError::Probe {
            path: path.display().to_string(),
            source: e,
        })?;

    let props = tagged.properties();
    let primary_tag = tagged.primary_tag().or_else(|| tagged.first_tag());

    // Empty strings become NULL so an album with a blank album-artist
    // tag groups and sorts with its untagged siblings rather than
    // forming its own "" artist.
    let text = |s: &str| {
        let t = s.trim();
        (!t.is_empty()).then(|| t.to_string())
    };

    Ok(ProbeResult {
        title: primary_tag.and_then(|t| t.title().and_then(|s| text(&s))),
        artist: primary_tag.and_then(|t| t.artist().and_then(|s| text(&s))),
        album: primary_tag.and_then(|t| t.album().and_then(|s| text(&s))),
        album_artist: primary_tag.and_then(|t| t.get_string(&ItemKey::AlbumArtist).and_then(text)),
        genre: primary_tag.and_then(|t| t.genre().and_then(|s| text(&s))),
        year: primary_tag.and_then(|t| t.year().map(|y| y as i64)),
        track_number: primary_tag.and_then(|t| t.track().map(|n| n as i64)),
        disc_number: primary_tag.and_then(|t| t.disk().map(|n| n as i64)),
        duration_ms: props.duration().as_millis() as i64,
        sample_rate: props.sample_rate().map(|r| r as i64),
        bit_depth: props.bit_depth().map(|b| b as i64),
        channels: props.channels().map(|c| c as i64),
        bit_rate: props.audio_bitrate().map(|b| b as i64),
        size_bytes: std::fs::metadata(path).map(|m| m.len() as i64).unwrap_or(0),
    })
}

pub async fn probe_and_add(engine: &SqliteRawEngine, path: &Path) -> Result<i64, IngestError> {
    let owned_path = path.to_path_buf();
    let probed = tokio::task::spawn_blocking(move || probe_blocking(&owned_path))
        .await
        .map_err(|e| IngestError::Db(anyhow::Error::from(e)))??;

    let title = probed.title.clone().or_else(|| {
        path.file_stem()
            .and_then(|s| s.to_str())
            .map(|s| s.to_string())
    });
    let title = title.ok_or_else(|| IngestError::NoFileName(path.display().to_string()))?;

    let opt_str = |v: Option<String>| v.map(FilterValue::String).unwrap_or(FilterValue::Null);
    let opt_int = |v: Option<i64>| v.map(FilterValue::Int).unwrap_or(FilterValue::Null);

    let sql = "INSERT INTO tracks (title, artist, album, album_artist, genre, year, \
               track_number, disc_number, duration_ms, size_bytes, \
               sample_rate, bit_depth, channels, bit_rate, file_path, playlist_ids) \
               VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, '[]') RETURNING id";

    let params: Vec<FilterValue> = vec![
        FilterValue::String(title),
        opt_str(probed.artist),
        opt_str(probed.album),
        opt_str(probed.album_artist),
        opt_str(probed.genre),
        opt_int(probed.year),
        opt_int(probed.track_number),
        opt_int(probed.disc_number),
        FilterValue::Int(probed.duration_ms),
        FilterValue::Int(probed.size_bytes),
        opt_int(probed.sample_rate),
        opt_int(probed.bit_depth),
        opt_int(probed.channels),
        opt_int(probed.bit_rate),
        FilterValue::String(path.display().to_string()),
    ];

    let json_row = engine
        .raw_sql_first(sql, &params)
        .await
        .map_err(|e| IngestError::Db(anyhow::Error::from(e)))?;

    let value: serde_json::Value = json_row.into_json();
    Ok(value.get("id").and_then(|v| v.as_i64()).unwrap_or(-1))
}

/// Extensions considered audio when walking a folder. Mirrors the
/// picker filter in `pick_and_add_track`, plus the iTunes containers
/// a consolidated library carries.
pub const AUDIO_EXTENSIONS: &[&str] = &[
    "flac", "mp3", "m4a", "m4p", "m4b", "wav", "ogg", "opus", "aiff", "aif", "dsf", "dff", "wma",
    "aac",
];

fn is_audio(path: &Path) -> bool {
    path.extension()
        .and_then(|e| e.to_str())
        .is_some_and(|e| AUDIO_EXTENSIONS.iter().any(|a| a.eq_ignore_ascii_case(e)))
}

/// Every audio file under `dir`, depth-first, sorted for a stable
/// insertion order. Symlinked directories are not followed. Unreadable
/// directories are skipped rather than aborting the walk.
pub fn collect_audio_files(dir: &Path) -> Vec<std::path::PathBuf> {
    let mut out = Vec::new();
    let mut stack = vec![dir.to_path_buf()];
    while let Some(d) = stack.pop() {
        let Ok(entries) = std::fs::read_dir(&d) else {
            continue;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            let Ok(ft) = entry.file_type() else {
                continue;
            };
            if ft.is_dir() {
                stack.push(path);
            } else if ft.is_file() && is_audio(&path) {
                out.push(path);
            }
        }
    }
    out.sort();
    out
}

/// Outcome of [`add_folder`]; serialized for the UI.
#[derive(Debug, Default, Clone, PartialEq, Eq, serde::Serialize)]
pub struct AddFolderSummary {
    pub added: u64,
    /// Already in the library (same path) — left untouched.
    pub skipped: u64,
    /// Files lofty could not read; listed so the user can see which.
    pub failed: Vec<String>,
    /// Row ids of the tracks just inserted, paired with the source path
    /// each came from, so the caller can queue them for copy-on-add.
    /// Not part of the UI payload.
    #[serde(skip)]
    pub added_tracks: Vec<(i64, std::path::PathBuf)>,
}

/// Every source path under `dir` the library already knows about,
/// loaded once so `add_folder` can check membership in memory instead
/// of issuing a SELECT per file.
///
/// Both `file_path` and `original_path` count: copy-on-add rewrites
/// `file_path` to the file's home under the managed root and records
/// where it came from in `original_path`, so matching on `file_path`
/// alone would see a re-scan of the same source folder as entirely new
/// and import every file a second time.
async fn known_paths(
    engine: &SqliteRawEngine,
    dir: &Path,
) -> Result<std::collections::HashSet<String>, IngestError> {
    let prefix = format!("{}%", dir.display());
    let rows = engine
        .raw_sql_query(
            "SELECT file_path, original_path FROM tracks \
             WHERE file_path LIKE ?1 OR original_path LIKE ?1",
            &[FilterValue::String(prefix)],
        )
        .await
        .map_err(|e| IngestError::Db(anyhow::Error::from(e)))?;
    Ok(rows
        .into_iter()
        .flat_map(|row| {
            let json = row.into_json();
            ["file_path", "original_path"]
                .into_iter()
                .filter_map(|col| json.get(col).and_then(|v| v.as_str()).map(str::to_owned))
                .collect::<Vec<_>>()
        })
        .collect())
}

/// The track that already owns `path`, whether as its current location
/// or as the source it was copied from. `file_path` is UNIQUE, so
/// before copy-on-add a re-add of the same file simply failed that
/// constraint; now that the column moves to the managed root, nothing
/// but this check stops a second add from duplicating both the row and
/// the file on disk.
/// Re-read the technical columns of an existing row whose file was just
/// rewritten in place (a re-export over a previous conversion). Title
/// and the other user-editable tags are left alone.
pub async fn probe_and_update(
    engine: &SqliteRawEngine,
    track_id: i64,
    path: &Path,
) -> Result<(), IngestError> {
    let owned_path = path.to_path_buf();
    let probed = tokio::task::spawn_blocking(move || probe_blocking(&owned_path))
        .await
        .map_err(|e| IngestError::Db(anyhow::Error::from(e)))??;
    let opt_int = |v: Option<i64>| v.map(FilterValue::Int).unwrap_or(FilterValue::Null);
    let sql = "UPDATE tracks SET duration_ms = ?, size_bytes = ?, sample_rate = ?, \
               bit_depth = ?, channels = ?, bit_rate = ?, file_hash = NULL WHERE id = ?";
    let params: Vec<FilterValue> = vec![
        FilterValue::Int(probed.duration_ms),
        FilterValue::Int(probed.size_bytes),
        opt_int(probed.sample_rate),
        opt_int(probed.bit_depth),
        opt_int(probed.channels),
        opt_int(probed.bit_rate),
        FilterValue::Int(track_id),
    ];
    engine
        .raw_sql_execute(sql, &params)
        .await
        .map_err(|e| IngestError::Db(anyhow::Error::from(e)))?;
    Ok(())
}

pub async fn track_id_for_path(
    engine: &SqliteRawEngine,
    path: &Path,
) -> Result<Option<i64>, IngestError> {
    let p = FilterValue::String(path.display().to_string());
    let row = engine
        .raw_sql_optional(
            "SELECT id FROM tracks WHERE file_path = ?1 OR original_path = ?1 LIMIT 1",
            &[p],
        )
        .await
        .map_err(|e| IngestError::Db(anyhow::Error::from(e)))?;
    Ok(row.and_then(|r| r.into_json().get("id").and_then(|v| v.as_i64())))
}

/// Add every audio file under `dir` that the library doesn't already
/// reference. Per-file probe failures are recorded, not fatal.
pub async fn add_folder(
    engine: &SqliteRawEngine,
    dir: &Path,
) -> Result<AddFolderSummary, IngestError> {
    let files = tokio::task::spawn_blocking({
        let d = dir.to_path_buf();
        move || collect_audio_files(&d)
    })
    .await
    .map_err(|e| IngestError::Db(anyhow::Error::from(e)))?;

    let known = known_paths(engine, dir).await?;

    let mut summary = AddFolderSummary::default();
    for path in files {
        let path_str = path.to_string_lossy().into_owned();
        if known.contains(&path_str) {
            summary.skipped += 1;
            continue;
        }
        match probe_and_add(engine, &path).await {
            Ok(id) => {
                summary.added += 1;
                summary.added_tracks.push((id, path));
            }
            Err(IngestError::Probe { path, source }) => {
                log::warn!("add_folder: skipping {path}: {source}");
                summary.failed.push(path);
            }
            // A database error is not per-file — the next insert would
            // almost certainly hit it too. Stop the walk, but return
            // what was added rather than `?`-ing the summary away: the
            // caller still has to queue those rows for copy, and rows
            // left behind here are invisible to a later re-run because
            // `known_paths` now counts them as known.
            Err(e) => {
                log::warn!("add_folder: stopping after {} files: {e}", summary.added);
                summary.failed.push(path_str);
                break;
            }
        }
    }
    Ok(summary)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::Db;

    /// Build a tiny synthetic WAV that lofty will happily parse.
    fn write_minimal_wav(path: &Path) {
        // 44-byte WAV header for a 1-sample, 1-channel, 8-bit, 8000 Hz file.
        let header: &[u8] = &[
            b'R', b'I', b'F', b'F', 0x25, 0x00, 0x00, 0x00, // chunk size 37
            b'W', b'A', b'V', b'E', b'f', b'm', b't', b' ', 0x10, 0x00, 0x00,
            0x00, // subchunk1 size 16
            0x01, 0x00, // PCM
            0x01, 0x00, // mono
            0x40, 0x1f, 0x00, 0x00, // 8000 Hz
            0x40, 0x1f, 0x00, 0x00, // byte rate
            0x01, 0x00, // block align
            0x08, 0x00, // bits/sample
            b'd', b'a', b't', b'a', 0x01, 0x00, 0x00, 0x00, // data size 1
            0x80, // one silent sample
        ];
        std::fs::write(path, header).unwrap();
    }

    #[tokio::test]
    async fn probe_and_add_inserts_track_from_wav() {
        let dir = tempfile::tempdir().unwrap();
        let wav = dir.path().join("probe_test.wav");
        write_minimal_wav(&wav);

        let tmp_db = tempfile::NamedTempFile::new().unwrap();
        let db = Db::open(tmp_db.path()).await.unwrap();

        let id = probe_and_add(&db.engine, &wav)
            .await
            .expect("ingest succeeds");
        assert!(id > 0);

        let row = crate::db::tracks::get(&db.engine, id).await.unwrap();
        assert_eq!(row.title, "probe_test");
        assert_eq!(row.file_path, wav.display().to_string());
    }

    /// Like `write_minimal_wav` but with an even-length data chunk: RIFF
    /// requires odd chunks to be padded, and `write_minimal_wav`'s
    /// one-byte chunk has no pad, so a tag appended after it is not
    /// found on re-read. Fine for the untagged tests above; not here.
    fn write_taggable_wav(path: &Path) {
        let bytes: &[u8] = &[
            b'R', b'I', b'F', b'F', 0x26, 0, 0, 0, b'W', b'A', b'V', b'E', b'f', b'm', b't', b' ',
            0x10, 0, 0, 0, 0x01, 0, 0x01, 0, 0x40, 0x1f, 0, 0, 0x40, 0x1f, 0, 0, 0x01, 0, 0x08, 0,
            b'd', b'a', b't', b'a', 0x02, 0, 0, 0, 0x80, 0x80,
        ];
        std::fs::write(path, bytes).unwrap();
    }

    #[tokio::test]
    async fn probe_and_add_stores_album_artist_disc_track_genre_and_year() {
        let dir = tempfile::tempdir().unwrap();
        let wav = dir.path().join("tagged.wav");
        write_taggable_wav(&wav);
        crate::fs::tags::write_metadata(
            &wav,
            &crate::db::tracks::MetadataEdit {
                title: "Anthem, Pt. 2",
                artist: Some("blink-182"),
                album: Some("Take Off Your Pants and Jacket"),
                album_artist: Some("blink-182"),
                genre: Some("Punk"),
                year: Some(2001),
                track_number: Some(1),
                disc_number: Some(2),
            },
        )
        .unwrap();

        let tmp_db = tempfile::NamedTempFile::new().unwrap();
        let db = Db::open(tmp_db.path()).await.unwrap();
        let id = probe_and_add(&db.engine, &wav).await.unwrap();
        let row = crate::db::tracks::get(&db.engine, id).await.unwrap();

        assert_eq!(row.title, "Anthem, Pt. 2");
        assert_eq!(row.album_artist.as_deref(), Some("blink-182"));
        assert_eq!(row.genre.as_deref(), Some("Punk"));
        assert_eq!(row.year, Some(2001));
        assert_eq!(row.track_number, Some(1));
        assert_eq!(row.disc_number, Some(2));
    }

    #[tokio::test]
    async fn probe_and_add_stores_blank_text_tags_as_null() {
        let dir = tempfile::tempdir().unwrap();
        let wav = dir.path().join("blank.wav");
        write_taggable_wav(&wav);
        crate::fs::tags::write_metadata(
            &wav,
            &crate::db::tracks::MetadataEdit {
                title: "x",
                artist: Some("  "),
                album: None,
                album_artist: None,
                genre: None,
                year: None,
                track_number: None,
                disc_number: None,
            },
        )
        .unwrap();

        let tmp_db = tempfile::NamedTempFile::new().unwrap();
        let db = Db::open(tmp_db.path()).await.unwrap();
        let id = probe_and_add(&db.engine, &wav).await.unwrap();
        let row = crate::db::tracks::get(&db.engine, id).await.unwrap();
        assert_eq!(row.artist, None);
        assert_eq!(row.album_artist, None);
        assert_eq!(row.track_number, None);
    }

    #[test]
    fn ingest_error_variants_display() {
        // Exercise IngestError so its variants are non-dead in non-test
        // builds.
        let e = IngestError::NoFileName("/missing".into());
        assert!(e.to_string().contains("/missing"));

        let e2 = IngestError::Db(anyhow::anyhow!("underlying"));
        assert!(e2.to_string().contains("underlying"));
    }

    #[tokio::test]
    async fn add_folder_walks_recursively_skips_known_paths_and_records_failures() {
        let tmp = tempfile::tempdir().unwrap();
        let db = Db::open(&tmp.path().join("t.db")).await.unwrap();
        let root = tmp.path().join("music");
        std::fs::create_dir_all(root.join("Artist/Album")).unwrap();
        write_minimal_wav(&root.join("top.wav"));
        write_minimal_wav(&root.join("Artist/Album/01.WAV"));
        write_minimal_wav(&root.join("Artist/Album/02.wav"));
        std::fs::write(root.join("Artist/Album/cover.jpg"), b"jpg").unwrap();
        std::fs::write(root.join("Artist/broken.flac"), b"not audio").unwrap();

        let files = collect_audio_files(&root);
        assert_eq!(files.len(), 4, "{files:?}");

        // Pre-register one file so it counts as skipped.
        probe_and_add(&db.engine, &root.join("top.wav"))
            .await
            .unwrap();

        let summary = add_folder(&db.engine, &root).await.unwrap();
        assert_eq!(summary.added, 2);
        assert_eq!(summary.skipped, 1);
        assert_eq!(summary.failed.len(), 1);
        assert!(summary.failed[0].ends_with("broken.flac"));

        // The ids + source paths the caller hands to copy-on-add: one
        // per newly added file, never the skipped or failed ones.
        assert_eq!(summary.added_tracks.len(), 2);
        let mut sources: Vec<_> = summary
            .added_tracks
            .iter()
            .map(|(_, p)| p.file_name().unwrap().to_string_lossy().into_owned())
            .collect();
        sources.sort();
        assert_eq!(sources, vec!["01.WAV", "02.wav"]);
        assert!(summary.added_tracks.iter().all(|(id, _)| *id > 0));

        // `added_tracks` is internal plumbing, not part of the payload
        // the UI receives.
        let json = serde_json::to_value(&summary).unwrap();
        assert!(json.get("added_tracks").is_none(), "{json}");

        let n: i64 = db
            .engine
            .raw_sql_scalar("SELECT COUNT(*) FROM tracks", &[])
            .await
            .unwrap();
        assert_eq!(n, 3);

        // Re-running is a no-op.
        let again = add_folder(&db.engine, &root).await.unwrap();
        assert_eq!(again.added, 0);
        assert_eq!(again.skipped, 3);
    }

    #[tokio::test]
    async fn add_folder_skips_files_copy_on_add_has_already_relocated() {
        let tmp = tempfile::tempdir().unwrap();
        let db = Db::open(&tmp.path().join("t.db")).await.unwrap();
        let root = tmp.path().join("music");
        std::fs::create_dir_all(&root).unwrap();
        write_minimal_wav(&root.join("a.wav"));

        let first = add_folder(&db.engine, &root).await.unwrap();
        assert_eq!(first.added, 1);

        // Stand in for the ingest worker: file_path moves under the
        // managed root and the source is recorded as original_path.
        let (id, source) = first.added_tracks[0].clone();
        db.engine
            .raw_sql_execute(
                "UPDATE tracks SET file_path = ?, original_path = ? WHERE id = ?",
                &[
                    FilterValue::String("/managed/Artist/Album/a.wav".into()),
                    FilterValue::String(source.display().to_string()),
                    FilterValue::Int(id),
                ],
            )
            .await
            .unwrap();

        let again = add_folder(&db.engine, &root).await.unwrap();
        assert_eq!(again.added, 0, "re-added a file that was already copied in");
        assert_eq!(again.skipped, 1);

        let n: i64 = db
            .engine
            .raw_sql_scalar("SELECT COUNT(*) FROM tracks", &[])
            .await
            .unwrap();
        assert_eq!(n, 1);
    }

    #[tokio::test]
    async fn track_id_for_path_matches_both_current_and_original_locations() {
        let tmp = tempfile::tempdir().unwrap();
        let db = Db::open(&tmp.path().join("t.db")).await.unwrap();
        let src = tmp.path().join("a.wav");
        write_minimal_wav(&src);

        assert_eq!(track_id_for_path(&db.engine, &src).await.unwrap(), None);

        let id = probe_and_add(&db.engine, &src).await.unwrap();
        assert_eq!(
            track_id_for_path(&db.engine, &src).await.unwrap(),
            Some(id),
            "should match while file_path is still the source"
        );

        db.engine
            .raw_sql_execute(
                "UPDATE tracks SET file_path = '/managed/a.wav', original_path = ? WHERE id = ?",
                &[
                    FilterValue::String(src.display().to_string()),
                    FilterValue::Int(id),
                ],
            )
            .await
            .unwrap();
        assert_eq!(
            track_id_for_path(&db.engine, &src).await.unwrap(),
            Some(id),
            "should still match once the copy moved file_path"
        );
    }

    #[tokio::test]
    async fn probe_and_add_errors_on_non_audio_file() {
        let dir = tempfile::tempdir().unwrap();
        let bogus = dir.path().join("not-audio.flac");
        std::fs::write(&bogus, b"not actually audio").unwrap();

        let tmp_db = tempfile::NamedTempFile::new().unwrap();
        let db = Db::open(tmp_db.path()).await.unwrap();

        let err = probe_and_add(&db.engine, &bogus).await.unwrap_err();
        assert!(matches!(err, IngestError::Probe { .. }));
    }
}
