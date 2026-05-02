//! Aggregations of `tracks` rows into per-album / per-artist summaries
//! for the album-grid and artist-split views.

use prax_query::filter::FilterValue as FV;
use prax_sqlite::raw::SqliteRawEngine;
use serde::{Deserialize, Serialize};

#[derive(Debug, thiserror::Error)]
pub enum AlbumsError {
    #[error("query failed: {0}")]
    Query(#[source] anyhow::Error),
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct AlbumSummary {
    /// Display name. Empty albums get the placeholder "Unknown Album".
    pub album: String,
    /// `album_artist` if set, otherwise the most-common `artist` for
    /// the album. Empty falls back to "Unknown Artist".
    pub album_artist: String,
    pub year: Option<i64>,
    pub track_count: i64,
    pub total_duration_ms: i64,
    /// First non-null `artwork_path` discovered for the album, if any.
    pub artwork_path: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ArtistSummary {
    pub artist: String,
    pub album_count: i64,
    pub track_count: i64,
}

/// Group tracks by `(album_artist, album)`. Tracks missing both fields
/// fold under "Unknown Artist / Unknown Album". Sorted by album_artist
/// then album.
///
/// `GROUP BY` uses the full coalesced expressions, not the aliases:
/// SQLite resolves bare identifiers in `GROUP BY` against table
/// columns first, so `GROUP BY album_artist, album` would split rows
/// with NULL vs '' into separate groups even though both project the
/// same "Unknown" placeholder in the SELECT.
pub async fn list_albums(engine: &SqliteRawEngine) -> Result<Vec<AlbumSummary>, AlbumsError> {
    let sql = "SELECT \
        COALESCE(NULLIF(album, ''), 'Unknown Album') AS album, \
        COALESCE(NULLIF(album_artist, ''), NULLIF(artist, ''), 'Unknown Artist') AS album_artist, \
        MIN(year) AS year, \
        COUNT(*) AS track_count, \
        COALESCE(SUM(duration_ms), 0) AS total_duration_ms, \
        MIN(artwork_path) AS artwork_path \
        FROM tracks \
        GROUP BY \
            COALESCE(NULLIF(album_artist, ''), NULLIF(artist, ''), 'Unknown Artist'), \
            COALESCE(NULLIF(album, ''), 'Unknown Album') \
        ORDER BY album_artist COLLATE NOCASE, album COLLATE NOCASE";
    let rows = engine
        .raw_sql_query(sql, &[])
        .await
        .map_err(|e| AlbumsError::Query(anyhow::Error::from(e)))?;
    rows.into_iter()
        .map(|r| serde_json::from_value(r.into_json()))
        .collect::<Result<Vec<_>, _>>()
        .map_err(|e| AlbumsError::Query(anyhow::Error::from(e)))
}

/// Group tracks by their effective artist (album_artist preferred).
///
/// `GROUP BY` uses the full expression rather than the `artist` alias —
/// SQLite resolves bare identifiers in `GROUP BY` against the table's
/// own columns first, so `GROUP BY artist` here would group by the raw
/// `tracks.artist` column and ignore the album_artist preference.
pub async fn list_artists(engine: &SqliteRawEngine) -> Result<Vec<ArtistSummary>, AlbumsError> {
    let sql = "SELECT \
        COALESCE(NULLIF(album_artist, ''), NULLIF(artist, ''), 'Unknown Artist') AS artist, \
        COUNT(DISTINCT COALESCE(NULLIF(album, ''), '__no_album__')) AS album_count, \
        COUNT(*) AS track_count \
        FROM tracks \
        GROUP BY COALESCE(NULLIF(album_artist, ''), NULLIF(artist, ''), 'Unknown Artist') \
        ORDER BY artist COLLATE NOCASE";
    let rows = engine
        .raw_sql_query(sql, &[])
        .await
        .map_err(|e| AlbumsError::Query(anyhow::Error::from(e)))?;
    rows.into_iter()
        .map(|r| serde_json::from_value(r.into_json()))
        .collect::<Result<Vec<_>, _>>()
        .map_err(|e| AlbumsError::Query(anyhow::Error::from(e)))
}

/// All tracks for the given `(album_artist, album)` pair, ordered by
/// disc/track number then title. The placeholder strings used by
/// `list_albums` ("Unknown Album", "Unknown Artist") are treated as
/// matches against NULL or empty source values so a click in the grid
/// always returns the same set the grid summarized.
pub async fn tracks_for_album(
    engine: &SqliteRawEngine,
    album_artist: &str,
    album: &str,
) -> Result<Vec<crate::db::tracks::TrackRow>, AlbumsError> {
    let artist_clause = if album_artist == "Unknown Artist" {
        "(album_artist IS NULL OR album_artist = '') \
         AND (artist IS NULL OR artist = '')"
    } else {
        "(COALESCE(NULLIF(album_artist, ''), NULLIF(artist, '')) = ?)"
    };
    let album_clause = if album == "Unknown Album" {
        "(album IS NULL OR album = '')"
    } else {
        "album = ?"
    };
    let sql = format!(
        "SELECT id, title, artist, album, duration_ms, file_path, file_hash, \
         sample_rate, bit_depth, kind, play_count, skip_count \
         FROM tracks \
         WHERE {artist_clause} AND {album_clause} \
         ORDER BY disc_number ASC NULLS LAST, track_number ASC NULLS LAST, \
                  title COLLATE NOCASE"
    );
    let mut params: Vec<FV> = Vec::new();
    if album_artist != "Unknown Artist" {
        params.push(FV::String(album_artist.to_string()));
    }
    if album != "Unknown Album" {
        params.push(FV::String(album.to_string()));
    }

    let rows = engine
        .raw_sql_query(&sql, &params)
        .await
        .map_err(|e| AlbumsError::Query(anyhow::Error::from(e)))?;
    rows.into_iter()
        .map(|r| serde_json::from_value(r.into_json()))
        .collect::<Result<Vec<_>, _>>()
        .map_err(|e| AlbumsError::Query(anyhow::Error::from(e)))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::Db;

    async fn tmp_db() -> Db {
        let tmp = tempfile::NamedTempFile::new().unwrap();
        Db::open(tmp.path()).await.unwrap()
    }

    async fn seed(engine: &SqliteRawEngine, fixtures: &[(&str, Option<&str>, Option<&str>)]) {
        for (i, (title, artist, album)) in fixtures.iter().enumerate() {
            let sql = "INSERT INTO tracks (title, album_artist, album, duration_ms, \
                       size_bytes, file_path, playlist_ids) VALUES (?, ?, ?, 1000, 0, ?, '[]')";
            let path = format!("/tmp/{i}.flac");
            let params = vec![
                FV::String((*title).to_string()),
                match artist {
                    Some(a) => FV::String((*a).to_string()),
                    None => FV::String(String::new()),
                },
                match album {
                    Some(a) => FV::String((*a).to_string()),
                    None => FV::String(String::new()),
                },
                FV::String(path),
            ];
            engine.raw_sql_execute(sql, &params).await.unwrap();
        }
    }

    #[tokio::test]
    async fn list_albums_groups_and_orders() {
        let db = tmp_db().await;
        seed(
            &db.engine,
            &[
                ("Track A1", Some("Artist B"), Some("Album One")),
                ("Track A2", Some("Artist B"), Some("Album One")),
                ("Track B1", Some("Artist A"), Some("Album Two")),
            ],
        )
        .await;

        let albums = list_albums(&db.engine).await.unwrap();
        assert_eq!(albums.len(), 2);
        // Sorted by album_artist NOCASE
        assert_eq!(albums[0].album_artist, "Artist A");
        assert_eq!(albums[0].album, "Album Two");
        assert_eq!(albums[0].track_count, 1);
        assert_eq!(albums[1].album_artist, "Artist B");
        assert_eq!(albums[1].track_count, 2);
        assert_eq!(albums[1].total_duration_ms, 2000);
    }

    #[tokio::test]
    async fn list_albums_uses_placeholders_for_missing_metadata() {
        let db = tmp_db().await;
        seed(&db.engine, &[("Untitled", None, None)]).await;
        let albums = list_albums(&db.engine).await.unwrap();
        assert_eq!(albums.len(), 1);
        assert_eq!(albums[0].album, "Unknown Album");
        assert_eq!(albums[0].album_artist, "Unknown Artist");
    }

    #[tokio::test]
    async fn list_artists_counts_distinct_albums() {
        let db = tmp_db().await;
        seed(
            &db.engine,
            &[
                ("Track 1", Some("Artist A"), Some("Album One")),
                ("Track 2", Some("Artist A"), Some("Album One")),
                ("Track 3", Some("Artist A"), Some("Album Two")),
                ("Track 4", Some("Artist B"), Some("Album Three")),
            ],
        )
        .await;

        let artists = list_artists(&db.engine).await.unwrap();
        assert_eq!(artists.len(), 2);
        let a = artists.iter().find(|x| x.artist == "Artist A").unwrap();
        assert_eq!(a.album_count, 2);
        assert_eq!(a.track_count, 3);
    }

    #[tokio::test]
    async fn tracks_for_album_returns_only_matching_tracks() {
        let db = tmp_db().await;
        seed(
            &db.engine,
            &[
                ("On Album", Some("Artist A"), Some("Album One")),
                ("On Album Too", Some("Artist A"), Some("Album One")),
                ("Other Album", Some("Artist A"), Some("Album Two")),
            ],
        )
        .await;

        let rows = tracks_for_album(&db.engine, "Artist A", "Album One")
            .await
            .unwrap();
        assert_eq!(rows.len(), 2);
        assert!(rows.iter().all(|r| r.album.as_deref() == Some("Album One")));
    }

    #[tokio::test]
    async fn tracks_for_album_handles_unknown_placeholders() {
        let db = tmp_db().await;
        seed(&db.engine, &[("orphan", None, None)]).await;
        let rows = tracks_for_album(&db.engine, "Unknown Artist", "Unknown Album")
            .await
            .unwrap();
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].title, "orphan");
    }
}
