//! Key/value access for the `preferences` table.
//!
//! The table stores one row per preference key, with `value` as a JSON
//! column. Callers pass/receive serde-compatible types; each key owns
//! its own shape (volume is an integer, library_root is a string, etc.).

use prax_query::filter::FilterValue;
use prax_sqlite::raw::SqliteRawEngine;
use serde::de::DeserializeOwned;
use serde::Serialize;

#[derive(Debug, thiserror::Error)]
pub enum PreferencesError {
    #[error("query failed: {0}")]
    Query(#[source] anyhow::Error),

    #[error("failed to (de)serialize pref {key:?}: {source}")]
    Serde {
        key: String,
        #[source]
        source: serde_json::Error,
    },
}

pub const KEY_VOLUME: &str = "volume";
pub const KEY_LIBRARY_ROOT: &str = "library_root";
pub const KEY_ORGANIZE_SCHEME: &str = "organize_scheme";
pub const KEY_KEEP_ORGANIZED: &str = "keep_organized";

const DEFAULT_LIBRARY_ROOT_SUFFIX: &str = "Music/TuxTunes";
pub const DEFAULT_ORGANIZE_SCHEME: &str =
    "{album_artist}/{album}/{disc:02}-{track:02} - {title}.{ext}";

/// Default managed library root: `$HOME/Music/TuxTunes`. Falls back to
/// `./Music/TuxTunes` when `$HOME` is unset (tests, unusual environments).
pub fn default_library_root() -> std::path::PathBuf {
    std::env::var_os("HOME")
        .map(std::path::PathBuf::from)
        .unwrap_or_else(|| std::path::PathBuf::from("."))
        .join(DEFAULT_LIBRARY_ROOT_SUFFIX)
}

/// Read a preference, returning `Ok(None)` if the key is absent.
pub async fn get<T: DeserializeOwned>(
    engine: &SqliteRawEngine,
    key: &str,
) -> Result<Option<T>, PreferencesError> {
    let sql = "SELECT value FROM preferences WHERE key = ?";
    let params = vec![FilterValue::String(key.to_string())];

    match engine.raw_sql_optional(sql, &params).await {
        Ok(Some(row)) => {
            let json: serde_json::Value = row.into_json();
            let raw = json.get("value").cloned().ok_or_else(|| {
                PreferencesError::Query(anyhow::anyhow!("row missing 'value' column"))
            })?;
            // SQLite stores our JSON column as TEXT; raw_sql_optional returns it
            // as a JSON string, so parse it back into its actual shape.
            let unwrapped: serde_json::Value = match raw {
                serde_json::Value::String(s) => {
                    serde_json::from_str(&s).map_err(|source| PreferencesError::Serde {
                        key: key.to_string(),
                        source,
                    })?
                }
                other => other,
            };
            let typed =
                serde_json::from_value(unwrapped).map_err(|source| PreferencesError::Serde {
                    key: key.to_string(),
                    source,
                })?;
            Ok(Some(typed))
        }
        Ok(None) => Ok(None),
        Err(e) => Err(PreferencesError::Query(anyhow::Error::from(e))),
    }
}

/// Upsert a preference.
pub async fn set<T: Serialize>(
    engine: &SqliteRawEngine,
    key: &str,
    value: &T,
) -> Result<(), PreferencesError> {
    let serialized = serde_json::to_string(value).map_err(|source| PreferencesError::Serde {
        key: key.to_string(),
        source,
    })?;
    let sql = "INSERT INTO preferences (key, value) VALUES (?, ?) \
               ON CONFLICT(key) DO UPDATE SET value = excluded.value";
    let params = vec![
        FilterValue::String(key.to_string()),
        FilterValue::String(serialized),
    ];
    engine
        .raw_sql_execute(sql, &params)
        .await
        .map(|_| ())
        .map_err(|e| PreferencesError::Query(anyhow::Error::from(e)))
}

/// Managed library root — absolute path where TuxTunes owns the file
/// layout. Returns `default_library_root()` when the key is absent.
pub async fn get_library_root(
    engine: &SqliteRawEngine,
) -> Result<std::path::PathBuf, PreferencesError> {
    let stored: Option<String> = get(engine, KEY_LIBRARY_ROOT).await?;
    Ok(stored
        .map(std::path::PathBuf::from)
        .unwrap_or_else(default_library_root))
}

pub async fn set_library_root(
    engine: &SqliteRawEngine,
    path: &std::path::Path,
) -> Result<(), PreferencesError> {
    let s = path.display().to_string();
    set(engine, KEY_LIBRARY_ROOT, &s).await
}

/// `organize_scheme` template string used by the organize + ingest
/// workers. Returns `DEFAULT_ORGANIZE_SCHEME` when the key is absent.
pub async fn get_organize_scheme(engine: &SqliteRawEngine) -> Result<String, PreferencesError> {
    let stored: Option<String> = get(engine, KEY_ORGANIZE_SCHEME).await?;
    Ok(stored.unwrap_or_else(|| DEFAULT_ORGANIZE_SCHEME.to_string()))
}

pub async fn set_organize_scheme(
    engine: &SqliteRawEngine,
    scheme: &str,
) -> Result<(), PreferencesError> {
    set(engine, KEY_ORGANIZE_SCHEME, &scheme.to_string()).await
}

/// Whether the `organize` worker runs on metadata edits. Defaults to
/// `true` when absent.
pub async fn get_keep_organized(engine: &SqliteRawEngine) -> Result<bool, PreferencesError> {
    let stored: Option<bool> = get(engine, KEY_KEEP_ORGANIZED).await?;
    Ok(stored.unwrap_or(true))
}

pub async fn set_keep_organized(
    engine: &SqliteRawEngine,
    keep: bool,
) -> Result<(), PreferencesError> {
    set(engine, KEY_KEEP_ORGANIZED, &keep).await
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::Db;

    async fn tmp_db() -> Db {
        let tmp = tempfile::NamedTempFile::new().unwrap();
        Db::open(tmp.path()).await.unwrap()
    }

    #[tokio::test]
    async fn get_returns_none_for_absent_key() {
        let db = tmp_db().await;
        let v: Option<i64> = get(&db.engine, "missing").await.unwrap();
        assert!(v.is_none());
    }

    #[tokio::test]
    async fn set_then_get_roundtrips_i64() {
        let db = tmp_db().await;
        set(&db.engine, KEY_VOLUME, &75_i64).await.unwrap();
        let v: Option<i64> = get(&db.engine, KEY_VOLUME).await.unwrap();
        assert_eq!(v, Some(75));
    }

    #[tokio::test]
    async fn set_overwrites_existing_value() {
        let db = tmp_db().await;
        set(&db.engine, KEY_VOLUME, &50_i64).await.unwrap();
        set(&db.engine, KEY_VOLUME, &90_i64).await.unwrap();
        let v: Option<i64> = get(&db.engine, KEY_VOLUME).await.unwrap();
        assert_eq!(v, Some(90));
    }

    #[tokio::test]
    async fn set_then_get_roundtrips_string() {
        let db = tmp_db().await;
        set(&db.engine, "theme", &"dark".to_string()).await.unwrap();
        let v: Option<String> = get(&db.engine, "theme").await.unwrap();
        assert_eq!(v.as_deref(), Some("dark"));
    }

    #[test]
    fn preferences_error_variants_display() {
        let e = PreferencesError::Query(anyhow::anyhow!("boom"));
        assert!(e.to_string().contains("boom"));
        let e2 = PreferencesError::Serde {
            key: "volume".into(),
            source: serde_json::from_str::<i64>("not-json").unwrap_err(),
        };
        assert!(e2.to_string().contains("volume"));
    }

    #[tokio::test]
    async fn library_root_defaults_and_roundtrips() {
        let db = tmp_db().await;
        // With no stored value, returns the default (ends with Music/TuxTunes).
        let def = get_library_root(&db.engine).await.unwrap();
        assert!(def.ends_with("Music/TuxTunes"));

        let custom = std::path::PathBuf::from("/tmp/tuxtunes-test");
        set_library_root(&db.engine, &custom).await.unwrap();
        assert_eq!(get_library_root(&db.engine).await.unwrap(), custom);
    }

    #[tokio::test]
    async fn organize_scheme_defaults_and_roundtrips() {
        let db = tmp_db().await;
        assert_eq!(
            get_organize_scheme(&db.engine).await.unwrap(),
            DEFAULT_ORGANIZE_SCHEME
        );
        set_organize_scheme(&db.engine, "{title}.{ext}")
            .await
            .unwrap();
        assert_eq!(
            get_organize_scheme(&db.engine).await.unwrap(),
            "{title}.{ext}"
        );
    }

    #[tokio::test]
    async fn keep_organized_defaults_true_and_roundtrips() {
        let db = tmp_db().await;
        assert!(get_keep_organized(&db.engine).await.unwrap());
        set_keep_organized(&db.engine, false).await.unwrap();
        assert!(!get_keep_organized(&db.engine).await.unwrap());
    }
}
