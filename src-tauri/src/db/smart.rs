//! Smart-playlist rule model + SQL compiler.
//!
//! Surfaces field parity for fields represented in the Track schema:
//! nested condition groups (AND/OR), per-field operators, limits with
//! selection mode. The compiler walks
//! the rule tree once and produces a parameterized `WHERE` clause +
//! `ORDER BY` + `LIMIT` against the `tracks` table — no string
//! concatenation of user input ever reaches SQL.

use prax_query::filter::FilterValue as FV;
use prax_sqlite::raw::SqliteRawEngine;
use serde::{Deserialize, Serialize};

#[derive(Debug, thiserror::Error)]
pub enum SmartError {
    #[error("query failed: {0}")]
    Query(#[source] anyhow::Error),

    #[error("unsupported field: {0}")]
    UnsupportedField(String),

    #[error("operator {op} is not valid for field {field}")]
    InvalidOperator { field: String, op: String },

    #[error("malformed rule: {0}")]
    Malformed(String),
}

// ----- Rule shape ---------------------------------------------------------

/// Top-level smart-playlist rule. `match_all` is authoritative for the
/// ROOT group's joiner (AND when true, OR when false) — it overrides
/// `root.match_all` for that one group. Nested groups under `root` keep
/// using their own `match_all` as always.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct SmartRule {
    #[serde(default = "default_true")]
    pub match_all: bool,
    #[serde(default = "default_true")]
    pub live_updating: bool,
    #[serde(default)]
    pub limit: Option<Limit>,
    pub root: ConditionGroup,
}

fn default_true() -> bool {
    true
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Limit {
    pub value: u32,
    pub unit: LimitUnit,
    #[serde(default)]
    pub selected_by: Option<SelectionMode>,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum LimitUnit {
    Songs,
    Minutes,
    Hours,
    Mb,
    Gb,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum SelectionMode {
    Random,
    SongName,
    Album,
    Artist,
    Genre,
    MostRecentlyAdded,
    MostOftenPlayed,
    MostRecentlyPlayed,
    HighestRating,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(untagged)]
pub enum Condition {
    Group(ConditionGroup),
    Leaf(LeafCondition),
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ConditionGroup {
    #[serde(default = "default_true")]
    pub match_all: bool,
    pub children: Vec<Condition>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct LeafCondition {
    pub field: String,
    pub op: Op,
    pub value: Value,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum Op {
    Is,
    IsNot,
    Contains,
    NotContains,
    StartsWith,
    EndsWith,
    Greater,
    Less,
    InRange,
    InTheLast,
    NotInTheLast,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(untagged)]
pub enum Value {
    Text(String),
    Int(i64),
    Bool(bool),
    Range { from: i64, to: i64 },
    Relative { n: i64, unit: TimeUnit },
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum TimeUnit {
    Days,
    Weeks,
    Months,
}

impl TimeUnit {
    fn modifier(self, n: i64) -> String {
        match self {
            Self::Days => format!("-{n} days"),
            Self::Weeks => format!("-{n} days", n = n * 7),
            Self::Months => format!("-{n} months"),
        }
    }
}

// ----- Field metadata -----------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum FieldKind {
    Text,
    Int,
    Bool,
    Date,
}

struct FieldMeta {
    column: &'static str,
    kind: FieldKind,
}

fn resolve_field(name: &str) -> Option<FieldMeta> {
    let (column, kind) = match name {
        "title" => ("title", FieldKind::Text),
        "artist" => ("artist", FieldKind::Text),
        "album_artist" => ("album_artist", FieldKind::Text),
        "album" => ("album", FieldKind::Text),
        "composer" => ("composer", FieldKind::Text),
        "genre" => ("genre", FieldKind::Text),
        "kind" => ("kind", FieldKind::Text),
        "comment" => ("comment", FieldKind::Text),
        "grouping" => ("grouping", FieldKind::Text),
        "year" => ("year", FieldKind::Int),
        "track_number" => ("track_number", FieldKind::Int),
        "disc_number" => ("disc_number", FieldKind::Int),
        "bpm" => ("bpm", FieldKind::Int),
        "duration_ms" => ("duration_ms", FieldKind::Int),
        "size_bytes" => ("size_bytes", FieldKind::Int),
        "bit_rate" => ("bit_rate", FieldKind::Int),
        "sample_rate" => ("sample_rate", FieldKind::Int),
        "rating" => ("rating", FieldKind::Int),
        "play_count" => ("play_count", FieldKind::Int),
        "skip_count" => ("skip_count", FieldKind::Int),
        "loved" => ("loved", FieldKind::Bool),
        "compilation" => ("compilation", FieldKind::Bool),
        "purchased" => ("purchased", FieldKind::Bool),
        "date_added" => ("date_added", FieldKind::Date),
        "last_played" => ("last_played", FieldKind::Date),
        "last_skipped" => ("last_skipped", FieldKind::Date),
        _ => return None,
    };
    Some(FieldMeta { column, kind })
}

// ----- Compiler -----------------------------------------------------------

/// Compiled SQL fragment + bound parameters.
#[derive(Debug, Default)]
pub struct CompiledQuery {
    pub sql: String,
    pub params: Vec<FV>,
}

/// Compile a smart-playlist rule into `SELECT … FROM tracks WHERE … LIMIT …`
/// against the `tracks` table. Order is determined by `selected_by`.
pub fn compile(rule: &SmartRule, columns: &str) -> Result<CompiledQuery, SmartError> {
    let mut params: Vec<FV> = Vec::new();
    // `rule.match_all` is authoritative for the root group's joiner,
    // overriding `root.match_all` for this one call only.
    let where_sql = compile_group(&rule.root, &mut params, Some(rule.match_all))?;

    let order = rule
        .limit
        .as_ref()
        .and_then(|l| l.selected_by.map(order_for))
        .unwrap_or_else(|| "date_added DESC".to_string());

    let limit_clause = match &rule.limit {
        Some(Limit {
            value,
            unit: LimitUnit::Songs,
            ..
        }) => format!("LIMIT {value}"),
        // Minutes/Hours/MB/GB caps are application-side: the compiler
        // returns an unbounded query (preserving ORDER BY) and
        // `evaluate()` truncates the result set with `truncate_by_cap`
        // after accumulating durations or sizes over the ordered rows.
        _ => String::new(),
    };

    let sql =
        format!("SELECT {columns} FROM tracks WHERE {where_sql} ORDER BY {order} {limit_clause}");

    Ok(CompiledQuery { sql, params })
}

/// Compile one condition group. `match_all_override`, when `Some`, wins
/// over `group.match_all` for this call only — used once, by `compile`,
/// to make the top-level `SmartRule::match_all` authoritative for the
/// root group. Nested groups always pass `None` and use their own
/// `match_all`.
fn compile_group(
    group: &ConditionGroup,
    params: &mut Vec<FV>,
    match_all_override: Option<bool>,
) -> Result<String, SmartError> {
    if group.children.is_empty() {
        return Ok("1=1".to_string());
    }
    let match_all = match_all_override.unwrap_or(group.match_all);
    let joiner = if match_all { " AND " } else { " OR " };
    let parts: Vec<String> = group
        .children
        .iter()
        .map(|child| match child {
            Condition::Group(g) => Ok(format!("({})", compile_group(g, params, None)?)),
            Condition::Leaf(l) => compile_leaf(l, params),
        })
        .collect::<Result<Vec<_>, SmartError>>()?;
    Ok(parts.join(joiner))
}

fn compile_leaf(leaf: &LeafCondition, params: &mut Vec<FV>) -> Result<String, SmartError> {
    let meta = resolve_field(&leaf.field)
        .ok_or_else(|| SmartError::UnsupportedField(leaf.field.clone()))?;

    match (meta.kind, leaf.op) {
        // --- Text ---
        (FieldKind::Text, Op::Is) => text_op(meta.column, "=", &leaf.value, params, &leaf.field),
        (FieldKind::Text, Op::IsNot) => {
            text_op(meta.column, "<>", &leaf.value, params, &leaf.field)
        }
        (FieldKind::Text, Op::Contains) => {
            like_op(meta.column, "%", "%", &leaf.value, params, &leaf.field)
        }
        (FieldKind::Text, Op::NotContains) => {
            like_not(meta.column, "%", "%", &leaf.value, params, &leaf.field)
        }
        (FieldKind::Text, Op::StartsWith) => {
            like_op(meta.column, "", "%", &leaf.value, params, &leaf.field)
        }
        (FieldKind::Text, Op::EndsWith) => {
            like_op(meta.column, "%", "", &leaf.value, params, &leaf.field)
        }

        // --- Int ---
        (FieldKind::Int, Op::Is) => int_op(meta.column, "=", &leaf.value, params, &leaf.field),
        (FieldKind::Int, Op::IsNot) => int_op(meta.column, "<>", &leaf.value, params, &leaf.field),
        (FieldKind::Int, Op::Greater) => int_op(meta.column, ">", &leaf.value, params, &leaf.field),
        (FieldKind::Int, Op::Less) => int_op(meta.column, "<", &leaf.value, params, &leaf.field),
        (FieldKind::Int, Op::InRange) => int_range(meta.column, &leaf.value, params, &leaf.field),

        // --- Bool ---
        (FieldKind::Bool, Op::Is) => bool_op(meta.column, "=", &leaf.value, params, &leaf.field),
        (FieldKind::Bool, Op::IsNot) => {
            bool_op(meta.column, "<>", &leaf.value, params, &leaf.field)
        }

        // --- Date ---
        (FieldKind::Date, Op::InTheLast) => {
            date_relative(meta.column, true, &leaf.value, &leaf.field)
        }
        (FieldKind::Date, Op::NotInTheLast) => {
            date_relative(meta.column, false, &leaf.value, &leaf.field)
        }
        (FieldKind::Date, Op::Greater) => {
            date_op(meta.column, ">", &leaf.value, params, &leaf.field)
        }
        (FieldKind::Date, Op::Less) => date_op(meta.column, "<", &leaf.value, params, &leaf.field),
        (FieldKind::Date, Op::InRange) => date_range(meta.column, &leaf.value, params, &leaf.field),

        _ => Err(SmartError::InvalidOperator {
            field: leaf.field.clone(),
            op: format!("{:?}", leaf.op),
        }),
    }
}

fn text_value<'v>(value: &'v Value, field: &str) -> Result<&'v str, SmartError> {
    match value {
        Value::Text(s) => Ok(s.as_str()),
        _ => Err(SmartError::Malformed(format!(
            "{field}: expected text value"
        ))),
    }
}

fn int_value(value: &Value, field: &str) -> Result<i64, SmartError> {
    match value {
        Value::Int(n) => Ok(*n),
        _ => Err(SmartError::Malformed(format!(
            "{field}: expected integer value"
        ))),
    }
}

fn bool_value(value: &Value, field: &str) -> Result<bool, SmartError> {
    match value {
        Value::Bool(b) => Ok(*b),
        _ => Err(SmartError::Malformed(format!(
            "{field}: expected boolean value"
        ))),
    }
}

fn text_op(
    column: &str,
    op: &str,
    value: &Value,
    params: &mut Vec<FV>,
    field: &str,
) -> Result<String, SmartError> {
    params.push(FV::String(text_value(value, field)?.to_string()));
    Ok(format!("{column} {op} ?"))
}

fn like_op(
    column: &str,
    pre: &str,
    suf: &str,
    value: &Value,
    params: &mut Vec<FV>,
    field: &str,
) -> Result<String, SmartError> {
    let raw = text_value(value, field)?;
    let pattern = format!("{pre}{}{suf}", escape_like(raw));
    params.push(FV::String(pattern));
    Ok(format!("{column} LIKE ? ESCAPE '\\'"))
}

fn like_not(
    column: &str,
    pre: &str,
    suf: &str,
    value: &Value,
    params: &mut Vec<FV>,
    field: &str,
) -> Result<String, SmartError> {
    let raw = text_value(value, field)?;
    let pattern = format!("{pre}{}{suf}", escape_like(raw));
    params.push(FV::String(pattern));
    // NULL columns shouldn't match `not contains` either way; coerce
    // them to the empty string so the LIKE compares predictably.
    Ok(format!("COALESCE({column}, '') NOT LIKE ? ESCAPE '\\'"))
}

fn int_op(
    column: &str,
    op: &str,
    value: &Value,
    params: &mut Vec<FV>,
    field: &str,
) -> Result<String, SmartError> {
    params.push(FV::Int(int_value(value, field)?));
    Ok(format!("{column} {op} ?"))
}

fn int_range(
    column: &str,
    value: &Value,
    params: &mut Vec<FV>,
    field: &str,
) -> Result<String, SmartError> {
    let (from, to) = match value {
        Value::Range { from, to } => (*from, *to),
        _ => {
            return Err(SmartError::Malformed(format!(
                "{field}: in_range expects {{from, to}}"
            )));
        }
    };
    params.push(FV::Int(from));
    params.push(FV::Int(to));
    Ok(format!("{column} BETWEEN ? AND ?"))
}

fn bool_op(
    column: &str,
    op: &str,
    value: &Value,
    params: &mut Vec<FV>,
    field: &str,
) -> Result<String, SmartError> {
    let b = bool_value(value, field)?;
    params.push(FV::Int(if b { 1 } else { 0 }));
    Ok(format!("{column} {op} ?"))
}

/// Date comparison against `column ({op}) ?`. `date_added` /
/// `last_played` / `last_skipped` are TEXT `'YYYY-MM-DD HH:MM:SS'`, so a
/// unix-seconds `Value::Int` is converted to the same text form via
/// `datetime(?, 'unixepoch')` before comparing — comparing the raw
/// integer parameter against the TEXT column would compare
/// lexicographically and silently match every/no row. A `Value::Text`
/// value is compared as-is: a literal `'YYYY-MM-DD HH:MM:SS'` string
/// already sorts correctly against the column's own text form.
fn date_op(
    column: &str,
    op: &str,
    value: &Value,
    params: &mut Vec<FV>,
    field: &str,
) -> Result<String, SmartError> {
    match value {
        Value::Int(n) => {
            params.push(FV::Int(*n));
            Ok(format!("{column} {op} datetime(?, 'unixepoch')"))
        }
        Value::Text(s) => {
            params.push(FV::String(s.clone()));
            Ok(format!("{column} {op} ?"))
        }
        _ => Err(SmartError::Malformed(format!(
            "{field}: expected an integer (unix seconds) or text date value"
        ))),
    }
}

/// `column BETWEEN datetime(?, 'unixepoch') AND datetime(?, 'unixepoch')`
/// from a `Value::Range` of unix-seconds bounds — see `date_op` for why
/// the conversion is necessary.
fn date_range(
    column: &str,
    value: &Value,
    params: &mut Vec<FV>,
    field: &str,
) -> Result<String, SmartError> {
    let (from, to) = match value {
        Value::Range { from, to } => (*from, *to),
        _ => {
            return Err(SmartError::Malformed(format!(
                "{field}: in_range expects {{from, to}}"
            )));
        }
    };
    params.push(FV::Int(from));
    params.push(FV::Int(to));
    Ok(format!(
        "{column} BETWEEN datetime(?, 'unixepoch') AND datetime(?, 'unixepoch')"
    ))
}

fn date_relative(
    column: &str,
    inside: bool,
    value: &Value,
    field: &str,
) -> Result<String, SmartError> {
    let (n, unit) = match value {
        Value::Relative { n, unit } => (*n, *unit),
        _ => {
            return Err(SmartError::Malformed(format!(
                "{field}: in_the_last expects {{n, unit}}"
            )));
        }
    };
    let modifier = unit.modifier(n);
    let cmp = if inside { ">=" } else { "<" };
    // datetime('now', '-N days') is parameterless from the caller's
    // perspective — the modifier is built from a typed enum variant
    // and a checked integer, so no untrusted text reaches SQL.
    Ok(format!("{column} {cmp} datetime('now', '{modifier}')"))
}

fn order_for(mode: SelectionMode) -> String {
    match mode {
        SelectionMode::Random => "RANDOM()".to_string(),
        SelectionMode::SongName => "title COLLATE NOCASE ASC".to_string(),
        SelectionMode::Album => "album COLLATE NOCASE ASC".to_string(),
        SelectionMode::Artist => "artist COLLATE NOCASE ASC".to_string(),
        SelectionMode::Genre => "genre COLLATE NOCASE ASC".to_string(),
        SelectionMode::MostRecentlyAdded => "date_added DESC".to_string(),
        SelectionMode::MostOftenPlayed => "play_count DESC".to_string(),
        SelectionMode::MostRecentlyPlayed => "last_played DESC NULLS LAST".to_string(),
        SelectionMode::HighestRating => "rating DESC".to_string(),
    }
}

/// Escape SQL LIKE wildcards so user input doesn't expand. Pairs with
/// the `ESCAPE '\\'` clause emitted by like_op / like_not.
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

// ----- Public command-layer helpers --------------------------------------

/// `TrackRow`'s columns plus `size_bytes`, which `TrackRow` doesn't
/// carry but `truncate_by_cap` needs for Mb/Gb caps (see `evaluate`).
fn track_list_columns() -> String {
    format!("{}, size_bytes", crate::db::tracks::TRACK_ROW_COLUMNS)
}

/// Evaluate a smart rule and return matching tracks.
pub async fn evaluate(
    engine: &SqliteRawEngine,
    rule: &SmartRule,
) -> Result<Vec<crate::db::tracks::TrackRow>, SmartError> {
    let q = compile(rule, &track_list_columns())?;
    let raw_rows = engine
        .raw_sql_query(&q.sql, &q.params)
        .await
        .map_err(|e| SmartError::Query(anyhow::Error::from(e)))?;

    // Keep the raw JSON alongside the typed rows: `TrackRow` doesn't
    // carry `size_bytes` (it's not part of the track-list view), but
    // `truncate_by_cap` needs it for Mb/Gb caps, and `size_bytes` is an
    // extra field `TrackRow`'s deserializer simply ignores.
    let jsons: Vec<serde_json::Value> = raw_rows.into_iter().map(|r| r.into_json()).collect();
    let rows: Vec<crate::db::tracks::TrackRow> = jsons
        .iter()
        .cloned()
        .map(serde_json::from_value)
        .collect::<Result<Vec<_>, _>>()
        .map_err(|e| SmartError::Query(anyhow::Error::from(e)))?;

    Ok(match rule.limit.as_ref() {
        Some(limit) if limit.unit != LimitUnit::Songs => truncate_by_cap(rows, &jsons, limit),
        _ => rows,
    })
}

/// Truncate an already-ordered set of rows so the running total of the
/// limit unit's underlying measure — duration for Minutes/Hours, size
/// for Mb/Gb — stays within the cap. `Songs`-unit limits are applied as
/// a SQL `LIMIT` by `compile()` and never reach this function.
///
/// A row is kept if adding it keeps the running total `<= cap`; the
/// first row that would push the total past the cap — and every row
/// after it, since `rows` preserves the compiler's `ORDER BY` — is
/// dropped. This matches iTunes' "stop before exceeding" behavior. As a
/// deliberate simplification, if the very first row alone exceeds the
/// cap, the result is empty rather than always keeping at least one row.
fn truncate_by_cap(
    rows: Vec<crate::db::tracks::TrackRow>,
    jsons: &[serde_json::Value],
    limit: &Limit,
) -> Vec<crate::db::tracks::TrackRow> {
    let cap: i64 = match limit.unit {
        LimitUnit::Minutes => i64::from(limit.value) * 60_000,
        LimitUnit::Hours => i64::from(limit.value) * 3_600_000,
        LimitUnit::Mb => i64::from(limit.value) * 1_000_000,
        LimitUnit::Gb => i64::from(limit.value) * 1_000_000_000,
        LimitUnit::Songs => return rows,
    };
    let field = match limit.unit {
        LimitUnit::Minutes | LimitUnit::Hours => "duration_ms",
        LimitUnit::Mb | LimitUnit::Gb => "size_bytes",
        LimitUnit::Songs => unreachable!(),
    };

    let mut total: i64 = 0;
    let mut out = Vec::with_capacity(rows.len());
    for (row, json) in rows.into_iter().zip(jsons.iter()) {
        let measure = json.get(field).and_then(|v| v.as_i64()).unwrap_or_else(|| {
            log::warn!("smart playlist cap: missing {field} on row, treating as 0");
            0
        });
        let next = total.saturating_add(measure);
        if next > cap {
            break;
        }
        total = next;
        out.push(row);
    }
    out
}

/// Lightweight count for the editor's "✓ N tracks match" preview.
pub async fn preview_count(engine: &SqliteRawEngine, rule: &SmartRule) -> Result<i64, SmartError> {
    // Strip the limit for the count — an iTunes-style preview should
    // show the full match count even if a Songs cap is set.
    let no_limit = SmartRule {
        limit: None,
        ..rule.clone()
    };
    let q = compile(&no_limit, "1")?;
    let count_sql = format!("SELECT COUNT(*) FROM ({})", q.sql);
    engine
        .raw_sql_scalar::<i64>(&count_sql, &q.params)
        .await
        .map_err(|e| SmartError::Query(anyhow::Error::from(e)))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::Db;

    async fn tmp_db() -> Db {
        let tmp = tempfile::NamedTempFile::new().unwrap();
        Db::open(tmp.path()).await.unwrap()
    }

    async fn seed(engine: &SqliteRawEngine, rows: &[(&str, &str, i64, i64)]) {
        for (i, (title, genre, year, plays)) in rows.iter().enumerate() {
            let sql = "INSERT INTO tracks (title, genre, year, play_count, duration_ms, \
                       size_bytes, file_path, playlist_ids, date_added) \
                       VALUES (?, ?, ?, ?, 1000, 0, ?, '[]', \
                       datetime('now', ?))";
            let path = format!("/tmp/{i}.flac");
            // Spread date_added over the past few weeks so date filters
            // can exercise live boundaries.
            let modifier = format!("-{} days", i);
            let params = vec![
                FV::String((*title).to_string()),
                FV::String((*genre).to_string()),
                FV::Int(*year),
                FV::Int(*plays),
                FV::String(path),
                FV::String(modifier),
            ];
            engine.raw_sql_execute(sql, &params).await.unwrap();
        }
    }

    fn leaf(field: &str, op: Op, value: Value) -> Condition {
        Condition::Leaf(LeafCondition {
            field: field.to_string(),
            op,
            value,
        })
    }

    fn rule(match_all: bool, children: Vec<Condition>) -> SmartRule {
        SmartRule {
            match_all,
            live_updating: true,
            limit: None,
            root: ConditionGroup {
                match_all,
                children,
            },
        }
    }

    #[tokio::test]
    async fn evaluate_text_contains() {
        let db = tmp_db().await;
        seed(
            &db.engine,
            &[
                ("Rock Star", "Rock", 2000, 5),
                ("Jazz Cat", "Jazz", 2010, 1),
                ("Rocky Road", "Pop", 2020, 12),
            ],
        )
        .await;
        let r = rule(
            true,
            vec![leaf("title", Op::Contains, Value::Text("rock".into()))],
        );
        let rows = evaluate(&db.engine, &r).await.unwrap();
        assert_eq!(rows.len(), 2);
        assert!(rows.iter().any(|r| r.title == "Rock Star"));
        assert!(rows.iter().any(|r| r.title == "Rocky Road"));
    }

    #[tokio::test]
    async fn evaluate_int_greater() {
        let db = tmp_db().await;
        seed(
            &db.engine,
            &[("a", "x", 0, 1), ("b", "x", 0, 5), ("c", "x", 0, 10)],
        )
        .await;
        let r = rule(true, vec![leaf("play_count", Op::Greater, Value::Int(3))]);
        let rows = evaluate(&db.engine, &r).await.unwrap();
        assert_eq!(rows.len(), 2);
    }

    #[tokio::test]
    async fn evaluate_combined_and_or() {
        let db = tmp_db().await;
        seed(
            &db.engine,
            &[
                ("a", "Rock", 2000, 0),
                ("b", "Rock", 2020, 0),
                ("c", "Jazz", 2020, 0),
            ],
        )
        .await;
        // (genre=Rock AND year>=2010)  ←→ match_all=true with one leaf and a
        // nested Group expressing year>=2010 via Greater(2009).
        let inner = ConditionGroup {
            match_all: true,
            children: vec![
                leaf("genre", Op::Is, Value::Text("Rock".into())),
                leaf("year", Op::Greater, Value::Int(2009)),
            ],
        };
        let r = SmartRule {
            match_all: true,
            live_updating: true,
            limit: None,
            root: inner,
        };
        let rows = evaluate(&db.engine, &r).await.unwrap();
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].title, "b");
    }

    #[tokio::test]
    async fn evaluate_relative_date_in_the_last_week() {
        let db = tmp_db().await;
        seed(
            &db.engine,
            &[
                ("recent", "x", 0, 0),    // date_added now -0 days
                ("yesterday", "x", 0, 0), // -1 day
                ("month_ago", "x", 0, 0), // -2 days … fixture spreads by index
            ],
        )
        .await;
        let r = rule(
            true,
            vec![leaf(
                "date_added",
                Op::InTheLast,
                Value::Relative {
                    n: 1,
                    unit: TimeUnit::Days,
                },
            )],
        );
        let rows = evaluate(&db.engine, &r).await.unwrap();
        assert!(!rows.is_empty(), "should match recent rows");
        // Every returned row should be the "recent" or "yesterday" entry.
        for row in &rows {
            assert!(matches!(row.title.as_str(), "recent" | "yesterday"));
        }
    }

    #[tokio::test]
    async fn unsupported_field_errors() {
        let db = tmp_db().await;
        let r = rule(
            true,
            vec![leaf("does_not_exist", Op::Is, Value::Text("x".into()))],
        );
        let err = evaluate(&db.engine, &r).await.unwrap_err();
        assert!(matches!(err, SmartError::UnsupportedField(_)));
    }

    #[tokio::test]
    async fn invalid_operator_for_field_errors() {
        let db = tmp_db().await;
        // contains doesn't apply to ints.
        let r = rule(
            true,
            vec![leaf("play_count", Op::Contains, Value::Text("3".into()))],
        );
        let err = evaluate(&db.engine, &r).await.unwrap_err();
        assert!(matches!(err, SmartError::InvalidOperator { .. }));
    }

    #[tokio::test]
    async fn limit_songs_caps_results() {
        let db = tmp_db().await;
        seed(
            &db.engine,
            &[("a", "x", 0, 0), ("b", "x", 0, 0), ("c", "x", 0, 0)],
        )
        .await;
        let r = SmartRule {
            match_all: true,
            live_updating: true,
            limit: Some(Limit {
                value: 2,
                unit: LimitUnit::Songs,
                selected_by: Some(SelectionMode::SongName),
            }),
            root: ConditionGroup {
                match_all: true,
                children: vec![],
            },
        };
        let rows = evaluate(&db.engine, &r).await.unwrap();
        assert_eq!(rows.len(), 2);
    }

    #[tokio::test]
    async fn preview_count_ignores_song_limit() {
        let db = tmp_db().await;
        seed(
            &db.engine,
            &[("a", "x", 0, 0), ("b", "x", 0, 0), ("c", "x", 0, 0)],
        )
        .await;
        let r = SmartRule {
            match_all: true,
            live_updating: true,
            limit: Some(Limit {
                value: 1,
                unit: LimitUnit::Songs,
                selected_by: Some(SelectionMode::SongName),
            }),
            root: ConditionGroup {
                match_all: true,
                children: vec![],
            },
        };
        let count = preview_count(&db.engine, &r).await.unwrap();
        assert_eq!(count, 3);
    }

    #[test]
    fn like_escape_neutralizes_user_wildcards() {
        // `100%` typed by the user must not match every row.
        assert_eq!(escape_like("100%"), "100\\%");
        assert_eq!(escape_like("a_b"), "a\\_b");
    }

    #[tokio::test]
    async fn evaluate_text_is_not() {
        let db = tmp_db().await;
        seed(&db.engine, &[("a", "Rock", 0, 0), ("b", "Jazz", 0, 0)]).await;
        let r = rule(
            true,
            vec![leaf("genre", Op::IsNot, Value::Text("Rock".into()))],
        );
        let rows = evaluate(&db.engine, &r).await.unwrap();
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].title, "b");
    }

    #[tokio::test]
    async fn evaluate_text_not_contains_handles_nulls() {
        let db = tmp_db().await;
        seed(
            &db.engine,
            &[("hello world", "x", 0, 0), ("goodbye", "x", 0, 0)],
        )
        .await;
        let r = rule(
            true,
            vec![leaf("title", Op::NotContains, Value::Text("hello".into()))],
        );
        let rows = evaluate(&db.engine, &r).await.unwrap();
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].title, "goodbye");
    }

    #[tokio::test]
    async fn evaluate_text_starts_and_ends_with() {
        let db = tmp_db().await;
        seed(
            &db.engine,
            &[
                ("Pre-fix", "x", 0, 0),
                ("middle", "x", 0, 0),
                ("post-Pre", "x", 0, 0),
            ],
        )
        .await;
        let starts = rule(
            true,
            vec![leaf("title", Op::StartsWith, Value::Text("Pre".into()))],
        );
        assert_eq!(evaluate(&db.engine, &starts).await.unwrap().len(), 1);
        let ends = rule(
            true,
            vec![leaf("title", Op::EndsWith, Value::Text("Pre".into()))],
        );
        let ends_rows = evaluate(&db.engine, &ends).await.unwrap();
        assert_eq!(ends_rows.len(), 1);
        assert_eq!(ends_rows[0].title, "post-Pre");
    }

    #[tokio::test]
    async fn evaluate_int_is_isnot_less_inrange() {
        let db = tmp_db().await;
        seed(
            &db.engine,
            &[("a", "x", 0, 1), ("b", "x", 0, 5), ("c", "x", 0, 10)],
        )
        .await;
        let is = rule(true, vec![leaf("play_count", Op::Is, Value::Int(5))]);
        assert_eq!(evaluate(&db.engine, &is).await.unwrap().len(), 1);
        let isnot = rule(true, vec![leaf("play_count", Op::IsNot, Value::Int(5))]);
        assert_eq!(evaluate(&db.engine, &isnot).await.unwrap().len(), 2);
        let less = rule(true, vec![leaf("play_count", Op::Less, Value::Int(5))]);
        assert_eq!(evaluate(&db.engine, &less).await.unwrap().len(), 1);
        let in_range = rule(
            true,
            vec![leaf(
                "play_count",
                Op::InRange,
                Value::Range { from: 2, to: 9 },
            )],
        );
        let rows = evaluate(&db.engine, &in_range).await.unwrap();
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].title, "b");
    }

    #[tokio::test]
    async fn evaluate_bool_is_and_isnot() {
        let db = tmp_db().await;
        db.engine
            .raw_sql_execute(
                "INSERT INTO tracks (title, duration_ms, size_bytes, file_path, \
                 playlist_ids, loved) \
                 VALUES ('loved', 1000, 0, '/tmp/l.flac', '[]', 1), \
                        ('plain', 1000, 0, '/tmp/p.flac', '[]', 0)",
                &[],
            )
            .await
            .unwrap();
        let is_loved = rule(true, vec![leaf("loved", Op::Is, Value::Bool(true))]);
        let rows = evaluate(&db.engine, &is_loved).await.unwrap();
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].title, "loved");
        let isnot_loved = rule(true, vec![leaf("loved", Op::IsNot, Value::Bool(true))]);
        let rows = evaluate(&db.engine, &isnot_loved).await.unwrap();
        assert_eq!(rows[0].title, "plain");
    }

    #[tokio::test]
    async fn evaluate_date_not_in_the_last() {
        let db = tmp_db().await;
        seed(&db.engine, &[("recent", "x", 0, 0), ("old", "x", 0, 0)]).await;
        db.engine
            .raw_sql_execute(
                "UPDATE tracks SET date_added = datetime('now', '-30 days') WHERE title = 'old'",
                &[],
            )
            .await
            .unwrap();
        let r = rule(
            true,
            vec![leaf(
                "date_added",
                Op::NotInTheLast,
                Value::Relative {
                    n: 7,
                    unit: TimeUnit::Days,
                },
            )],
        );
        let rows = evaluate(&db.engine, &r).await.unwrap();
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].title, "old");
    }

    /// Pin `date_added` on each seeded row to a known unix-second
    /// timestamp so `greater` / `less` / `in_range` can be checked
    /// against exact boundaries rather than "now"-relative fixtures.
    async fn set_date_added(engine: &SqliteRawEngine, title: &str, unix_secs: i64) {
        engine
            .raw_sql_execute(
                "UPDATE tracks SET date_added = datetime(?, 'unixepoch') WHERE title = ?",
                &[FV::Int(unix_secs), FV::String(title.to_string())],
            )
            .await
            .unwrap();
    }

    #[tokio::test]
    async fn evaluate_date_greater_and_less_unix_seconds() {
        let db = tmp_db().await;
        seed(&db.engine, &[("before", "x", 0, 0), ("after", "x", 0, 0)]).await;
        // 2019-01-01 and 2021-01-01, straddling the 2020-01-01 boundary
        // used below (1577836800).
        set_date_added(&db.engine, "before", 1_546_300_800).await;
        set_date_added(&db.engine, "after", 1_609_459_200).await;

        let boundary = 1_577_836_800; // 2020-01-01T00:00:00Z
        let greater = rule(
            true,
            vec![leaf("date_added", Op::Greater, Value::Int(boundary))],
        );
        let rows = evaluate(&db.engine, &greater).await.unwrap();
        // Old (buggy) behavior compared TEXT column vs INTEGER param
        // lexicographically and matched every row; this must match only
        // the row actually after the boundary.
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].title, "after");

        let less = rule(
            true,
            vec![leaf("date_added", Op::Less, Value::Int(boundary))],
        );
        let rows = evaluate(&db.engine, &less).await.unwrap();
        // Old (buggy) behavior matched no rows at all for `less`.
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].title, "before");
    }

    #[tokio::test]
    async fn evaluate_date_in_range_unix_seconds() {
        let db = tmp_db().await;
        seed(
            &db.engine,
            &[
                ("too_early", "x", 0, 0),
                ("middle_one", "x", 0, 0),
                ("middle_two", "x", 0, 0),
                ("too_late", "x", 0, 0),
            ],
        )
        .await;
        set_date_added(&db.engine, "too_early", 1_500_000_000).await;
        set_date_added(&db.engine, "middle_one", 1_580_000_000).await;
        set_date_added(&db.engine, "middle_two", 1_590_000_000).await;
        set_date_added(&db.engine, "too_late", 1_650_000_000).await;

        let r = rule(
            true,
            vec![leaf(
                "date_added",
                Op::InRange,
                Value::Range {
                    from: 1_577_836_800, // 2020-01-01
                    to: 1_609_459_200,   // 2021-01-01
                },
            )],
        );
        let rows = evaluate(&db.engine, &r).await.unwrap();
        assert_eq!(rows.len(), 2);
        assert!(rows.iter().any(|r| r.title == "middle_one"));
        assert!(rows.iter().any(|r| r.title == "middle_two"));
    }

    #[tokio::test]
    async fn evaluate_date_greater_text_literal() {
        let db = tmp_db().await;
        seed(&db.engine, &[("before", "x", 0, 0), ("after", "x", 0, 0)]).await;
        set_date_added(&db.engine, "before", 1_546_300_800).await;
        set_date_added(&db.engine, "after", 1_609_459_200).await;

        let r = rule(
            true,
            vec![leaf(
                "date_added",
                Op::Greater,
                Value::Text("2020-01-01 00:00:00".to_string()),
            )],
        );
        let rows = evaluate(&db.engine, &r).await.unwrap();
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].title, "after");
    }

    #[tokio::test]
    async fn evaluate_nested_or_group() {
        let db = tmp_db().await;
        seed(
            &db.engine,
            &[("a", "Rock", 0, 0), ("b", "Jazz", 0, 0), ("c", "Pop", 0, 0)],
        )
        .await;
        let nested = ConditionGroup {
            match_all: false,
            children: vec![
                leaf("genre", Op::Is, Value::Text("Rock".into())),
                leaf("genre", Op::Is, Value::Text("Pop".into())),
            ],
        };
        let r = SmartRule {
            match_all: true,
            live_updating: true,
            limit: None,
            root: ConditionGroup {
                match_all: true,
                children: vec![Condition::Group(nested)],
            },
        };
        let rows = evaluate(&db.engine, &r).await.unwrap();
        assert_eq!(rows.len(), 2);
    }

    #[tokio::test]
    async fn malformed_text_value_for_text_field_errors() {
        let db = tmp_db().await;
        let r = rule(true, vec![leaf("title", Op::Is, Value::Int(5))]);
        assert!(matches!(
            evaluate(&db.engine, &r).await.unwrap_err(),
            SmartError::Malformed(_)
        ));
    }

    #[tokio::test]
    async fn malformed_int_value_for_int_field_errors() {
        let db = tmp_db().await;
        let r = rule(
            true,
            vec![leaf("play_count", Op::Is, Value::Text("five".into()))],
        );
        assert!(matches!(
            evaluate(&db.engine, &r).await.unwrap_err(),
            SmartError::Malformed(_)
        ));
    }

    #[tokio::test]
    async fn malformed_bool_value_for_bool_field_errors() {
        let db = tmp_db().await;
        let r = rule(true, vec![leaf("loved", Op::Is, Value::Int(1))]);
        assert!(matches!(
            evaluate(&db.engine, &r).await.unwrap_err(),
            SmartError::Malformed(_)
        ));
    }

    #[tokio::test]
    async fn malformed_range_value_errors() {
        let db = tmp_db().await;
        let r = rule(true, vec![leaf("play_count", Op::InRange, Value::Int(5))]);
        assert!(matches!(
            evaluate(&db.engine, &r).await.unwrap_err(),
            SmartError::Malformed(_)
        ));
    }

    #[tokio::test]
    async fn malformed_relative_value_errors() {
        let db = tmp_db().await;
        let r = rule(true, vec![leaf("date_added", Op::InTheLast, Value::Int(5))]);
        assert!(matches!(
            evaluate(&db.engine, &r).await.unwrap_err(),
            SmartError::Malformed(_)
        ));
    }

    #[test]
    fn time_unit_modifier_handles_weeks_and_months() {
        assert_eq!(TimeUnit::Days.modifier(3), "-3 days");
        assert_eq!(TimeUnit::Weeks.modifier(2), "-14 days");
        assert_eq!(TimeUnit::Months.modifier(6), "-6 months");
    }

    #[tokio::test]
    async fn empty_group_compiles_to_truthy() {
        let db = tmp_db().await;
        seed(&db.engine, &[("a", "x", 0, 0), ("b", "x", 0, 0)]).await;
        let r = rule(true, vec![]);
        let rows = evaluate(&db.engine, &r).await.unwrap();
        assert_eq!(rows.len(), 2);
    }

    #[tokio::test]
    async fn order_for_every_selection_mode_compiles() {
        let db = tmp_db().await;
        seed(&db.engine, &[("a", "x", 0, 0), ("b", "x", 0, 0)]).await;
        for mode in [
            SelectionMode::Random,
            SelectionMode::SongName,
            SelectionMode::Album,
            SelectionMode::Artist,
            SelectionMode::Genre,
            SelectionMode::MostRecentlyAdded,
            SelectionMode::MostOftenPlayed,
            SelectionMode::MostRecentlyPlayed,
            SelectionMode::HighestRating,
        ] {
            let r = SmartRule {
                match_all: true,
                live_updating: true,
                limit: Some(Limit {
                    value: 10,
                    unit: LimitUnit::Songs,
                    selected_by: Some(mode),
                }),
                root: ConditionGroup {
                    match_all: true,
                    children: vec![],
                },
            };
            assert_eq!(evaluate(&db.engine, &r).await.unwrap().len(), 2);
        }
    }

    #[tokio::test]
    async fn limit_with_non_song_unit_yields_unbounded_query() {
        let db = tmp_db().await;
        seed(
            &db.engine,
            &[("a", "x", 0, 0), ("b", "x", 0, 0), ("c", "x", 0, 0)],
        )
        .await;
        let r = SmartRule {
            match_all: true,
            live_updating: true,
            limit: Some(Limit {
                value: 1,
                unit: LimitUnit::Minutes,
                selected_by: None,
            }),
            root: ConditionGroup {
                match_all: true,
                children: vec![],
            },
        };
        assert_eq!(evaluate(&db.engine, &r).await.unwrap().len(), 3);
    }

    #[tokio::test]
    async fn limit_minutes_truncates_by_running_duration() {
        let db = tmp_db().await;
        // Three tracks at 20 minutes (1_200_000 ms) each, ordered by title.
        db.engine
            .raw_sql_execute(
                "INSERT INTO tracks (title, duration_ms, size_bytes, file_path, \
                 playlist_ids, date_added) VALUES \
                 ('a', 1200000, 0, '/tmp/a.flac', '[]', datetime('now', '-2 days')), \
                 ('b', 1200000, 0, '/tmp/b.flac', '[]', datetime('now', '-1 days')), \
                 ('c', 1200000, 0, '/tmp/c.flac', '[]', datetime('now'))",
                &[],
            )
            .await
            .unwrap();
        // Cap at 45 minutes: a (20) + b (20) = 40 fits, + c (20) = 60 exceeds.
        let r = SmartRule {
            match_all: true,
            live_updating: true,
            limit: Some(Limit {
                value: 45,
                unit: LimitUnit::Minutes,
                selected_by: Some(SelectionMode::SongName),
            }),
            root: ConditionGroup {
                match_all: true,
                children: vec![],
            },
        };
        let rows = evaluate(&db.engine, &r).await.unwrap();
        assert_eq!(rows.len(), 2);
        assert_eq!(rows[0].title, "a");
        assert_eq!(rows[1].title, "b");
    }

    #[tokio::test]
    async fn limit_minutes_zero_cap_keeps_only_zero_duration_rows() {
        let db = tmp_db().await;
        // One zero-duration track and two with positive duration.
        db.engine
            .raw_sql_execute(
                "INSERT INTO tracks (title, duration_ms, size_bytes, file_path, \
                 playlist_ids, date_added) VALUES \
                 ('a', 0, 0, '/tmp/a.flac', '[]', datetime('now', '-3 days')), \
                 ('b', 1200000, 0, '/tmp/b.flac', '[]', datetime('now', '-2 days')), \
                 ('c', 0, 0, '/tmp/c.flac', '[]', datetime('now', '-1 days'))",
                &[],
            )
            .await
            .unwrap();
        // Cap at 0 minutes: zero-duration rows keep the running total at
        // 0 (`0 > 0` is false), while any positive-duration row pushes
        // the total past the cap and is dropped, along with everything
        // after it in ORDER BY order.
        let r = SmartRule {
            match_all: true,
            live_updating: true,
            limit: Some(Limit {
                value: 0,
                unit: LimitUnit::Minutes,
                selected_by: Some(SelectionMode::SongName),
            }),
            root: ConditionGroup {
                match_all: true,
                children: vec![],
            },
        };
        let rows = evaluate(&db.engine, &r).await.unwrap();
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].title, "a");
    }

    #[tokio::test]
    async fn limit_hours_truncates_by_running_duration() {
        let db = tmp_db().await;
        // Two tracks at 1 hour (3_600_000 ms) each.
        db.engine
            .raw_sql_execute(
                "INSERT INTO tracks (title, duration_ms, size_bytes, file_path, \
                 playlist_ids) VALUES \
                 ('a', 3600000, 0, '/tmp/a.flac', '[]'), \
                 ('b', 3600000, 0, '/tmp/b.flac', '[]')",
                &[],
            )
            .await
            .unwrap();
        // Cap at 1 hour: only the first track fits.
        let r = SmartRule {
            match_all: true,
            live_updating: true,
            limit: Some(Limit {
                value: 1,
                unit: LimitUnit::Hours,
                selected_by: Some(SelectionMode::SongName),
            }),
            root: ConditionGroup {
                match_all: true,
                children: vec![],
            },
        };
        let rows = evaluate(&db.engine, &r).await.unwrap();
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].title, "a");
    }

    #[tokio::test]
    async fn limit_mb_truncates_by_running_size() {
        let db = tmp_db().await;
        // Three tracks at 4 MB (4_000_000 bytes) each.
        db.engine
            .raw_sql_execute(
                "INSERT INTO tracks (title, duration_ms, size_bytes, file_path, \
                 playlist_ids) VALUES \
                 ('a', 1000, 4000000, '/tmp/a.flac', '[]'), \
                 ('b', 1000, 4000000, '/tmp/b.flac', '[]'), \
                 ('c', 1000, 4000000, '/tmp/c.flac', '[]')",
                &[],
            )
            .await
            .unwrap();
        // Cap at 10 MB: a (4) + b (4) = 8 fits, + c (4) = 12 exceeds.
        let r = SmartRule {
            match_all: true,
            live_updating: true,
            limit: Some(Limit {
                value: 10,
                unit: LimitUnit::Mb,
                selected_by: Some(SelectionMode::SongName),
            }),
            root: ConditionGroup {
                match_all: true,
                children: vec![],
            },
        };
        let rows = evaluate(&db.engine, &r).await.unwrap();
        assert_eq!(rows.len(), 2);
    }

    #[tokio::test]
    async fn limit_gb_truncates_by_running_size() {
        let db = tmp_db().await;
        // Two tracks at 1 GB (1_000_000_000 bytes) each.
        db.engine
            .raw_sql_execute(
                "INSERT INTO tracks (title, duration_ms, size_bytes, file_path, \
                 playlist_ids) VALUES \
                 ('a', 1000, 1000000000, '/tmp/a.flac', '[]'), \
                 ('b', 1000, 1000000000, '/tmp/b.flac', '[]')",
                &[],
            )
            .await
            .unwrap();
        // Cap at 1 GB: only the first track fits.
        let r = SmartRule {
            match_all: true,
            live_updating: true,
            limit: Some(Limit {
                value: 1,
                unit: LimitUnit::Gb,
                selected_by: Some(SelectionMode::SongName),
            }),
            root: ConditionGroup {
                match_all: true,
                children: vec![],
            },
        };
        let rows = evaluate(&db.engine, &r).await.unwrap();
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].title, "a");
    }

    #[tokio::test]
    async fn limit_below_first_row_yields_zero_rows() {
        let db = tmp_db().await;
        db.engine
            .raw_sql_execute(
                "INSERT INTO tracks (title, duration_ms, size_bytes, file_path, \
                 playlist_ids) VALUES ('a', 3600000, 0, '/tmp/a.flac', '[]')",
                &[],
            )
            .await
            .unwrap();
        let r = SmartRule {
            match_all: true,
            live_updating: true,
            limit: Some(Limit {
                value: 1,
                unit: LimitUnit::Minutes,
                selected_by: Some(SelectionMode::SongName),
            }),
            root: ConditionGroup {
                match_all: true,
                children: vec![],
            },
        };
        let rows = evaluate(&db.engine, &r).await.unwrap();
        assert!(rows.is_empty());
    }

    #[test]
    fn smart_error_display_covers_every_variant() {
        let q = SmartError::Query(anyhow::anyhow!("boom"));
        assert!(q.to_string().contains("boom"));
        let u = SmartError::UnsupportedField("xyz".into());
        assert!(u.to_string().contains("xyz"));
        let i = SmartError::InvalidOperator {
            field: "f".into(),
            op: "Op".into(),
        };
        let s = i.to_string();
        assert!(s.contains("f") && s.contains("Op"));
        let m = SmartError::Malformed("nope".into());
        assert!(m.to_string().contains("nope"));
    }

    #[tokio::test]
    async fn top_level_match_all_overrides_root_group_joiner() {
        let db = tmp_db().await;
        seed(
            &db.engine,
            &[
                ("a", "Rock", 2000, 0),
                ("b", "Jazz", 2020, 0),
                ("c", "Pop", 2020, 0),
            ],
        )
        .await;
        // rule.match_all=false (OR) must win over root.match_all=true
        // (AND): no row is both Rock and Jazz, so AND would match
        // nothing, while OR matches "a" (Rock) and "b" (Jazz).
        let r = SmartRule {
            match_all: false,
            live_updating: true,
            limit: None,
            root: ConditionGroup {
                match_all: true,
                children: vec![
                    leaf("genre", Op::Is, Value::Text("Rock".into())),
                    leaf("genre", Op::Is, Value::Text("Jazz".into())),
                ],
            },
        };
        // Under AND (root.match_all as-is) this would match nothing —
        // no row is both Rock and Jazz. Under OR (rule.match_all,
        // authoritative) it matches "a" and "b".
        let rows = evaluate(&db.engine, &r).await.unwrap();
        assert_eq!(rows.len(), 2);
        assert!(rows.iter().any(|r| r.title == "a"));
        assert!(rows.iter().any(|r| r.title == "b"));
    }

    #[test]
    fn rule_roundtrips_through_serde() {
        let r = SmartRule {
            match_all: false,
            live_updating: true,
            limit: Some(Limit {
                value: 25,
                unit: LimitUnit::Songs,
                selected_by: Some(SelectionMode::Random),
            }),
            root: ConditionGroup {
                match_all: false,
                children: vec![
                    leaf("title", Op::Contains, Value::Text("love".into())),
                    leaf("rating", Op::Greater, Value::Int(60)),
                ],
            },
        };
        let json = serde_json::to_string(&r).unwrap();
        let back: SmartRule = serde_json::from_str(&json).unwrap();
        assert_eq!(r, back);
    }
}
