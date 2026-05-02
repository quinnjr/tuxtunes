//! `get_distinct` — distinct categorical values + per-value track
//! counts under an arbitrary subset of categorical filters. Backs the
//! Column Browser strip above the track view.

use prax_query::filter::FilterValue as FV;
use prax_sqlite::raw::SqliteRawEngine;
use serde::{Deserialize, Serialize};

#[derive(Debug, thiserror::Error)]
pub enum DistinctError {
    #[error("query failed: {0}")]
    Query(#[source] anyhow::Error),
    #[error("unsupported column: {0}")]
    UnsupportedColumn(String),
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct DistinctValue {
    /// Display label. Empty source values map to a per-column placeholder
    /// ("Unknown Genre", "Unknown Artist", "Unknown Album").
    pub value: String,
    pub count: i64,
}

#[derive(Debug, Default, Deserialize, Clone)]
pub struct TrackFilters {
    /// Multiple values per slot are OR'd; slots are AND'd together.
    /// Each entry is a display label (i.e. matches `DistinctValue.value`)
    /// — empty/placeholder labels round-trip to `column IS NULL OR = ''`.
    #[serde(default)]
    pub genres: Vec<String>,
    #[serde(default)]
    pub artists: Vec<String>,
    #[serde(default)]
    pub albums: Vec<String>,
    #[serde(default)]
    pub search: Option<String>,
}

impl TrackFilters {
    /// Whether any filter slot is populated. Used by callers that want
    /// to short-circuit to the unfiltered hot path.
    pub fn is_empty(&self) -> bool {
        self.genres.is_empty()
            && self.artists.is_empty()
            && self.albums.is_empty()
            && self
                .search
                .as_deref()
                .map(str::trim)
                .is_none_or(str::is_empty)
    }
}

/// Render the WHERE clause + bound params that match `filters`.
/// Returns `("", [])` when no filters apply, so callers can interpolate
/// without an `if` ladder.
pub(crate) fn build_where(filters: &TrackFilters) -> (String, Vec<FV>) {
    let mut clauses: Vec<String> = Vec::new();
    let mut params: Vec<FV> = Vec::new();

    push_in_clause(
        &mut clauses,
        &mut params,
        column_expr_for("genre"),
        "Unknown Genre",
        &filters.genres,
    );
    push_in_clause(
        &mut clauses,
        &mut params,
        column_expr_for("artist"),
        "Unknown Artist",
        &filters.artists,
    );
    push_in_clause(
        &mut clauses,
        &mut params,
        column_expr_for("album"),
        "Unknown Album",
        &filters.albums,
    );

    if let Some(q) = filters.search.as_deref().map(str::trim) {
        if !q.is_empty() {
            let pattern = format!("%{}%", escape_like(q));
            clauses.push(
                "(title LIKE ? ESCAPE '\\' OR artist LIKE ? ESCAPE '\\' \
                  OR album LIKE ? ESCAPE '\\' OR album_artist LIKE ? ESCAPE '\\')"
                    .to_string(),
            );
            for _ in 0..4 {
                params.push(FV::String(pattern.clone()));
            }
        }
    }

    if clauses.is_empty() {
        (String::new(), params)
    } else {
        (format!("WHERE {}", clauses.join(" AND ")), params)
    }
}

/// Distinct values + counts for one categorical column under `filters`.
/// `column` is one of the keys in [`column_expr_for`]'s match arm.
pub async fn get_distinct(
    engine: &SqliteRawEngine,
    column: &str,
    filters: &TrackFilters,
) -> Result<Vec<DistinctValue>, DistinctError> {
    let expr = column_expr_for(column).ok_or_else(|| DistinctError::UnsupportedColumn(column.to_string()))?;

    // Drop the same column's filter from the where clause — selecting
    // distinct values for "genre" should ignore the active genre filter
    // so the user can switch between genres without re-clicking a
    // current selection. Other columns' filters still apply.
    let scoped = filters_with_column_cleared(filters, column);
    let (where_clause, params) = build_where(&scoped);

    let placeholder = placeholder_for(column);
    let sql = format!(
        "SELECT \
            COALESCE(NULLIF({expr}, ''), '{placeholder}') AS value, \
            COUNT(*) AS count \
         FROM tracks {where_clause} \
         GROUP BY COALESCE(NULLIF({expr}, ''), '{placeholder}') \
         ORDER BY value COLLATE NOCASE"
    );
    let rows = engine
        .raw_sql_query(&sql, &params)
        .await
        .map_err(|e| DistinctError::Query(anyhow::Error::from(e)))?;
    rows.into_iter()
        .map(|r| serde_json::from_value(r.into_json()))
        .collect::<Result<Vec<_>, _>>()
        .map_err(|e| DistinctError::Query(anyhow::Error::from(e)))
}

/// Map a logical column name to the SQL expression that resolves it.
/// "artist" prefers album_artist; the rest map directly.
fn column_expr_for(column: &str) -> Option<&'static str> {
    match column {
        "genre" => Some("genre"),
        "artist" => Some("COALESCE(NULLIF(album_artist, ''), NULLIF(artist, ''))"),
        "album" => Some("album"),
        _ => None,
    }
}

fn placeholder_for(column: &str) -> &'static str {
    match column {
        "genre" => "Unknown Genre",
        "artist" => "Unknown Artist",
        "album" => "Unknown Album",
        _ => "",
    }
}

fn filters_with_column_cleared(filters: &TrackFilters, column: &str) -> TrackFilters {
    let mut out = filters.clone();
    match column {
        "genre" => out.genres.clear(),
        "artist" => out.artists.clear(),
        "album" => out.albums.clear(),
        _ => {}
    }
    out
}

fn push_in_clause(
    clauses: &mut Vec<String>,
    params: &mut Vec<FV>,
    column_expr: Option<&'static str>,
    placeholder: &str,
    values: &[String],
) {
    if values.is_empty() {
        return;
    }
    let Some(expr) = column_expr else { return };

    let mut alts: Vec<String> = Vec::new();
    for v in values {
        if v == placeholder {
            // The "Unknown …" placeholder represents NULL or empty source.
            alts.push(format!("({expr} IS NULL OR {expr} = '')"));
        } else {
            alts.push(format!("{expr} = ?"));
            params.push(FV::String(v.clone()));
        }
    }
    clauses.push(format!("({})", alts.join(" OR ")));
}

fn escape_like(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for ch in s.chars() {
        if matches!(ch, '\\' | '%' | '_') {
            out.push('\\');
        }
        out.push(ch);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::Db;

    async fn tmp_db() -> Db {
        let tmp = tempfile::NamedTempFile::new().unwrap();
        Db::open(tmp.path()).await.unwrap()
    }

    async fn seed(
        engine: &SqliteRawEngine,
        rows: &[(&str, Option<&str>, Option<&str>, Option<&str>)],
    ) {
        for (i, (title, artist, album, genre)) in rows.iter().enumerate() {
            let sql = "INSERT INTO tracks (title, album_artist, album, genre, \
                       duration_ms, size_bytes, file_path, playlist_ids) \
                       VALUES (?, ?, ?, ?, 1000, 0, ?, '[]')";
            let params = vec![
                FV::String((*title).to_string()),
                opt_str(*artist),
                opt_str(*album),
                opt_str(*genre),
                FV::String(format!("/tmp/{i}.flac")),
            ];
            engine.raw_sql_execute(sql, &params).await.unwrap();
        }
    }

    fn opt_str(o: Option<&str>) -> FV {
        match o {
            Some(s) => FV::String(s.to_string()),
            None => FV::String(String::new()),
        }
    }

    #[tokio::test]
    async fn distinct_genres_groups_and_counts() {
        let db = tmp_db().await;
        seed(
            &db.engine,
            &[
                ("a", Some("X"), Some("A1"), Some("Rock")),
                ("b", Some("X"), Some("A1"), Some("Rock")),
                ("c", Some("Y"), Some("A2"), Some("Jazz")),
                ("d", Some("Y"), Some("A2"), None),
            ],
        )
        .await;

        let f = TrackFilters::default();
        let g = get_distinct(&db.engine, "genre", &f).await.unwrap();
        assert_eq!(g.len(), 3);
        // Sorted NOCASE: Jazz, Rock, Unknown Genre
        assert_eq!(g[0].value, "Jazz");
        assert_eq!(g[1].value, "Rock");
        assert_eq!(g[1].count, 2);
        assert_eq!(g[2].value, "Unknown Genre");
        assert_eq!(g[2].count, 1);
    }

    #[tokio::test]
    async fn distinct_filters_by_other_column_but_not_self() {
        let db = tmp_db().await;
        seed(
            &db.engine,
            &[
                ("a", Some("X"), Some("A1"), Some("Rock")),
                ("b", Some("X"), Some("A1"), Some("Jazz")),
                ("c", Some("Y"), Some("A2"), Some("Rock")),
            ],
        )
        .await;

        // Pinning artist=X should narrow albums to A1 only…
        let f = TrackFilters {
            artists: vec!["X".into()],
            ..Default::default()
        };
        let albums = get_distinct(&db.engine, "album", &f).await.unwrap();
        assert_eq!(albums.len(), 1);
        assert_eq!(albums[0].value, "A1");

        // …but distinct artists should still show every artist (the
        // active artist filter is dropped from its own column query so
        // the user can pivot away).
        let artists = get_distinct(&db.engine, "artist", &f).await.unwrap();
        assert_eq!(artists.len(), 2);
    }

    #[tokio::test]
    async fn distinct_unknown_placeholder_round_trips() {
        let db = tmp_db().await;
        seed(&db.engine, &[("a", Some("X"), Some("A1"), None)]).await;
        // Filter by "Unknown Genre" → matches the NULL row.
        let f = TrackFilters {
            genres: vec!["Unknown Genre".into()],
            ..Default::default()
        };
        let albums = get_distinct(&db.engine, "album", &f).await.unwrap();
        assert_eq!(albums.len(), 1);
        assert_eq!(albums[0].value, "A1");
    }

    #[tokio::test]
    async fn distinct_unsupported_column_errors() {
        let db = tmp_db().await;
        let err = get_distinct(&db.engine, "bogus", &TrackFilters::default())
            .await
            .unwrap_err();
        assert!(matches!(err, DistinctError::UnsupportedColumn(_)));
    }

    #[test]
    fn track_filters_is_empty_handles_blank_search() {
        let f = TrackFilters {
            search: Some("   ".into()),
            ..Default::default()
        };
        assert!(f.is_empty());
    }
}
