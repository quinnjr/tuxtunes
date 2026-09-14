//! The per-artist genre map, persisted as `genre_map.json` beside the
//! library database. Every decision `resolve` makes lands here with its
//! provenance, so the file is both the cache that makes re-runs cheap
//! and the place to hand-correct a wrong guess (set `"source":
//! "manual"` and the entry is never touched again).

use super::taxonomy::{umbrella_for, Umbrella};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

pub const FILE_NAME: &str = "genre_map.json";
pub const VERSION: u32 = 1;

#[derive(Debug, thiserror::Error)]
pub enum MapError {
    #[error("cannot read {path}: {source}")]
    Read {
        path: String,
        #[source]
        source: std::io::Error,
    },
    #[error("cannot write {path}: {source}")]
    Write {
        path: String,
        #[source]
        source: std::io::Error,
    },
    #[error("{path} is not a valid genre map: {source}")]
    Parse {
        path: String,
        #[source]
        source: serde_json::Error,
    },
    #[error("{path} is version {found}, this build reads version {expected}")]
    Version {
        path: String,
        found: u32,
        expected: u32,
    },
}

/// Where an artist's genre came from.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Source {
    /// MusicBrainz artist tags.
    Musicbrainz,
    /// Dominant normalized tag already on the artist's tracks.
    Tags,
    /// Hand-edited in the map file; never overwritten.
    Manual,
    /// Nothing usable found; tracks keep their own normalized tag.
    Unresolved,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ArtistGenre {
    /// Canonical specific genre, e.g. "Melodic Death Metal". Empty when
    /// unresolved.
    pub genre: String,
    /// Folder display name, e.g. "Metal". Derived from `genre` unless
    /// the entry is manual.
    pub umbrella: String,
    pub source: Source,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mbid: Option<String>,
    #[serde(default)]
    pub track_count: u64,
}

impl ArtistGenre {
    pub fn new(genre: impl Into<String>, source: Source, track_count: u64) -> Self {
        let genre = genre.into();
        let umbrella = umbrella_for(&genre).display_name().to_string();
        Self {
            genre,
            umbrella,
            source,
            mbid: None,
            track_count,
        }
    }

    pub fn unresolved(track_count: u64) -> Self {
        Self {
            genre: String::new(),
            umbrella: Umbrella::Other.display_name().to_string(),
            source: Source::Unresolved,
            mbid: None,
            track_count,
        }
    }

    /// The folder this artist files under, honouring a hand-edited
    /// umbrella on manual entries and re-deriving it otherwise.
    pub fn umbrella(&self) -> Umbrella {
        if self.source == Source::Manual {
            if let Some(u) = Umbrella::from_display_name(&self.umbrella) {
                return u;
            }
        }
        if self.genre.is_empty() {
            return Umbrella::Other;
        }
        umbrella_for(&self.genre)
    }

    pub fn is_resolved(&self) -> bool {
        self.source != Source::Unresolved && !self.genre.trim().is_empty()
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct GenreMap {
    pub version: u32,
    /// Keyed by artist key (album artist, falling back to artist).
    pub artists: BTreeMap<String, ArtistGenre>,
}

impl Default for GenreMap {
    fn default() -> Self {
        Self {
            version: VERSION,
            artists: BTreeMap::new(),
        }
    }
}

impl GenreMap {
    /// `genre_map.json` next to the database file.
    pub fn path_for(db_path: &Path) -> PathBuf {
        db_path
            .parent()
            .map(Path::to_path_buf)
            .unwrap_or_default()
            .join(FILE_NAME)
    }

    /// `Ok(None)` when the file does not exist yet.
    pub fn load(path: &Path) -> Result<Option<GenreMap>, MapError> {
        let text = match std::fs::read_to_string(path) {
            Ok(t) => t,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Err(e) => {
                return Err(MapError::Read {
                    path: path.display().to_string(),
                    source: e,
                })
            }
        };
        let map: GenreMap = serde_json::from_str(&text).map_err(|e| MapError::Parse {
            path: path.display().to_string(),
            source: e,
        })?;
        if map.version != VERSION {
            return Err(MapError::Version {
                path: path.display().to_string(),
                found: map.version,
                expected: VERSION,
            });
        }
        Ok(Some(map))
    }

    /// Write atomically: serialize to a sibling temp file, then rename
    /// over the target, so an interrupted save never leaves a torn map.
    pub fn save(&self, path: &Path) -> Result<(), MapError> {
        let write_err = |source| MapError::Write {
            path: path.display().to_string(),
            source,
        };
        let text = serde_json::to_string_pretty(self).expect("GenreMap serializes");
        let tmp = path.with_extension("json.tmp");
        std::fs::write(&tmp, text).map_err(write_err)?;
        std::fs::rename(&tmp, path).map_err(write_err)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trips_and_sorts_keys() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join(FILE_NAME);
        let mut map = GenreMap::default();
        map.artists.insert(
            "Zebra".into(),
            ArtistGenre::new("Metalcore", Source::Musicbrainz, 12),
        );
        map.artists.insert("Alpha".into(), ArtistGenre::unresolved(1));
        map.save(&path).unwrap();
        let text = std::fs::read_to_string(&path).unwrap();
        assert!(text.find("\"Alpha\"").unwrap() < text.find("\"Zebra\"").unwrap());
        assert!(text.contains("\"source\": \"musicbrainz\""));
        assert!(!text.contains("\"mbid\": null"));
        let back = GenreMap::load(&path).unwrap().unwrap();
        assert_eq!(back, map);
        assert!(!path.with_extension("json.tmp").exists());
    }

    #[test]
    fn missing_file_is_none() {
        let dir = tempfile::tempdir().unwrap();
        assert_eq!(GenreMap::load(&dir.path().join("nope.json")).unwrap(), None);
    }

    #[test]
    fn malformed_file_is_an_error_naming_the_path() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join(FILE_NAME);
        std::fs::write(&path, "{ not json").unwrap();
        let err = GenreMap::load(&path).unwrap_err().to_string();
        assert!(err.contains(FILE_NAME), "{err}");
    }

    #[test]
    fn wrong_version_is_rejected() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join(FILE_NAME);
        std::fs::write(&path, r#"{"version":99,"artists":{}}"#).unwrap();
        assert!(matches!(
            GenreMap::load(&path),
            Err(MapError::Version { found: 99, .. })
        ));
    }

    #[test]
    fn umbrella_is_derived_unless_manual() {
        let auto = ArtistGenre::new("Metalcore", Source::Musicbrainz, 1);
        assert_eq!(auto.umbrella(), Umbrella::Metal);
        assert_eq!(auto.umbrella, "Metal");
        let mut manual = ArtistGenre::new("Metalcore", Source::Manual, 1);
        manual.umbrella = "Rock".into();
        assert_eq!(manual.umbrella(), Umbrella::Rock);
        let mut bad_manual = manual.clone();
        bad_manual.umbrella = "Nonsense".into();
        assert_eq!(bad_manual.umbrella(), Umbrella::Metal);
        assert_eq!(ArtistGenre::unresolved(3).umbrella(), Umbrella::Other);
        assert!(!ArtistGenre::unresolved(3).is_resolved());
    }

    #[test]
    fn path_sits_beside_the_database() {
        let p = GenreMap::path_for(Path::new("/data/app/tuxtunes.db"));
        assert_eq!(p, PathBuf::from("/data/app/genre_map.json"));
    }
}
