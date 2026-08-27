//! Probe an audio file with `lofty` and insert a minimal `Track` row.
//!
//! Files are not copied into a managed library root; `file_path` points at
//! the user-picked source file.

use lofty::file::{AudioFile, TaggedFileExt};
use lofty::probe::Probe;
use lofty::tag::Accessor;
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

    Ok(ProbeResult {
        title: primary_tag.and_then(|t| t.title().map(|s| s.to_string())),
        artist: primary_tag.and_then(|t| t.artist().map(|s| s.to_string())),
        album: primary_tag.and_then(|t| t.album().map(|s| s.to_string())),
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

    let artist = probed.artist;
    let album = probed.album;
    let duration_ms = probed.duration_ms;
    let sample_rate = probed.sample_rate;
    let bit_depth = probed.bit_depth;
    let channels = probed.channels;
    let bit_rate = probed.bit_rate;
    let size_bytes = probed.size_bytes;

    let sql = "INSERT INTO tracks (title, artist, album, duration_ms, size_bytes, \
               sample_rate, bit_depth, channels, bit_rate, file_path, playlist_ids) \
               VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, '[]') RETURNING id";

    let params: Vec<FilterValue> = vec![
        FilterValue::String(title),
        artist.map(FilterValue::String).unwrap_or(FilterValue::Null),
        album.map(FilterValue::String).unwrap_or(FilterValue::Null),
        FilterValue::Int(duration_ms),
        FilterValue::Int(size_bytes),
        sample_rate
            .map(FilterValue::Int)
            .unwrap_or(FilterValue::Null),
        bit_depth.map(FilterValue::Int).unwrap_or(FilterValue::Null),
        channels.map(FilterValue::Int).unwrap_or(FilterValue::Null),
        bit_rate.map(FilterValue::Int).unwrap_or(FilterValue::Null),
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
}

/// Every `file_path` already registered under `dir`, loaded once so
/// `add_folder` can check membership in memory instead of issuing a
/// `path_in_use` SELECT per file.
async fn known_paths(
    engine: &SqliteRawEngine,
    dir: &Path,
) -> Result<std::collections::HashSet<String>, IngestError> {
    let prefix = format!("{}%", dir.display());
    let rows = engine
        .raw_sql_query(
            "SELECT file_path FROM tracks WHERE file_path LIKE ?",
            &[FilterValue::String(prefix)],
        )
        .await
        .map_err(|e| IngestError::Db(anyhow::Error::from(e)))?;
    Ok(rows
        .into_iter()
        .filter_map(|row| {
            row.into_json()
                .get("file_path")
                .and_then(|v| v.as_str())
                .map(str::to_owned)
        })
        .collect())
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
            Ok(_) => summary.added += 1,
            Err(IngestError::Probe { path, source }) => {
                log::warn!("add_folder: skipping {path}: {source}");
                summary.failed.push(path);
            }
            Err(e) => return Err(e),
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
