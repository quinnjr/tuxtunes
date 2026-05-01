//! Helpers shared by the sync-facing DB modules.

use prax_query::filter::FilterValue;
use prax_sqlite::raw::SqliteRawEngine;
use prax_sqlite::SqliteError;
use std::collections::HashMap;

/// iTunes Persistent ID as a zero-padded 16-char hex string — the
/// canonical on-disk form in the `persistent_id` TEXT column.
pub fn pid_hex(pid: u64) -> String {
    format!("{pid:016x}")
}

pub fn opt_str(v: Option<&str>) -> FilterValue {
    v.map(|s| FilterValue::String(s.to_string()))
        .unwrap_or(FilterValue::Null)
}

pub fn opt_int(v: Option<i64>) -> FilterValue {
    v.map(FilterValue::Int).unwrap_or(FilterValue::Null)
}

/// Serde helper: parse SQLite INTEGER 0/1 into `bool`. iTunes-shaped
/// schemas store booleans as INTEGER, so JSON values coming through
/// Prax arrive as numbers; plain `bool::deserialize` would reject them.
pub fn sqlite_bool<'de, D>(d: D) -> Result<bool, D::Error>
where
    D: serde::Deserializer<'de>,
{
    use serde::Deserialize;
    let v = serde_json::Value::deserialize(d)?;
    Ok(v.as_i64().map(|n| n != 0).unwrap_or(false))
}

/// Load a `persistent_id (u64) → local id (i64)` map for every row in
/// `table` matching `sync_source_id`. Used by reconcilers to avoid N+1
/// SELECTs. Parses the hex TEXT column back to u64 up front so callers
/// can use integer keys in hot loops.
pub async fn load_pid_to_local_id_map(
    engine: &SqliteRawEngine,
    table: &'static str,
    sync_source_id: i64,
) -> Result<HashMap<u64, i64>, SqliteError> {
    let sql = format!(
        "SELECT id, persistent_id FROM {table} \
         WHERE sync_source_id = ? AND persistent_id IS NOT NULL"
    );
    let rows = engine
        .raw_sql_query(&sql, &[FilterValue::Int(sync_source_id)])
        .await?;
    let mut out = HashMap::with_capacity(rows.len());
    for r in rows {
        let v = r.into_json();
        let Some(id) = v.get("id").and_then(|v| v.as_i64()) else {
            continue;
        };
        let Some(pid_str) = v.get("persistent_id").and_then(|v| v.as_str()) else {
            continue;
        };
        let Ok(pid) = u64::from_str_radix(pid_str, 16) else {
            continue;
        };
        out.insert(pid, id);
    }
    Ok(out)
}

/// Delete rows in `table` under `sync_source_id` whose `persistent_id`
/// is not in `keep`. Stages the keep set in a temp table to side-step
/// SQLite's per-statement parameter limit — important at real iTunes
/// scale (~50K tracks).
///
/// The staging inserts are wrapped in a single transaction via
/// `raw_sql_batch` so WAL is fsync'd once rather than once per batch.
pub async fn delete_by_keep_set(
    engine: &SqliteRawEngine,
    table: &'static str,
    sync_source_id: i64,
    keep: &[u64],
) -> Result<u64, SqliteError> {
    if keep.is_empty() {
        let sql = format!("DELETE FROM {table} WHERE sync_source_id = ?");
        return engine
            .raw_sql_execute(&sql, &[FilterValue::Int(sync_source_id)])
            .await;
    }

    engine
        .raw_sql_execute(
            "CREATE TEMP TABLE IF NOT EXISTS _sync_keep \
             (persistent_id TEXT PRIMARY KEY) WITHOUT ROWID",
            &[],
        )
        .await?;
    engine
        .raw_sql_execute("DELETE FROM _sync_keep", &[])
        .await?;

    // Build one SQL string: BEGIN; N multi-row INSERTs; COMMIT. Inlining
    // the hex values is safe because they come from `pid_hex`
    // (always 16 chars of [0-9a-f]); there is no user-supplied input.
    const BATCH: usize = 500;
    let mut batch_sql = String::with_capacity(keep.len() * 20 + 64);
    batch_sql.push_str("BEGIN;\n");
    for chunk in keep.chunks(BATCH) {
        batch_sql.push_str("INSERT INTO _sync_keep (persistent_id) VALUES ");
        for (i, pid) in chunk.iter().enumerate() {
            if i > 0 {
                batch_sql.push(',');
            }
            batch_sql.push_str(&format!("('{pid:016x}')"));
        }
        batch_sql.push_str(";\n");
    }
    batch_sql.push_str("COMMIT;");
    engine.raw_sql_batch(&batch_sql).await?;

    let sql = format!(
        "DELETE FROM {table} WHERE sync_source_id = ? \
         AND persistent_id NOT IN (SELECT persistent_id FROM _sync_keep)"
    );
    engine
        .raw_sql_execute(&sql, &[FilterValue::Int(sync_source_id)])
        .await
}
