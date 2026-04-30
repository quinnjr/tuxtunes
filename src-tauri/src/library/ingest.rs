//! Probe an audio file with `lofty` and insert a minimal `Track` row.
//!
//! Phase 2 intentionally does NOT copy files into a managed library root —
//! that's Phase 4. `file_path` points at the user-picked source file.

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

pub async fn probe_and_add(engine: &SqliteRawEngine, path: &Path) -> Result<i64, IngestError> {
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
    let duration_ms = props.duration().as_millis() as i64;
    let sample_rate = props.sample_rate().map(|r| r as i64);
    let bit_depth = props.bit_depth().map(|b| b as i64);
    let channels = props.channels().map(|c| c as i64);
    let bit_rate = props.audio_bitrate().map(|b| b as i64);

    let primary_tag = tagged.primary_tag().or_else(|| tagged.first_tag());

    let title = primary_tag
        .and_then(|t| t.title().map(|s| s.to_string()))
        .or_else(|| {
            path.file_stem()
                .and_then(|s| s.to_str())
                .map(|s| s.to_string())
        })
        .ok_or_else(|| IngestError::NoFileName(path.display().to_string()))?;

    let artist = primary_tag.and_then(|t| t.artist().map(|s| s.to_string()));
    let album = primary_tag.and_then(|t| t.album().map(|s| s.to_string()));

    let size_bytes = std::fs::metadata(path).map(|m| m.len() as i64).unwrap_or(0);

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
}
