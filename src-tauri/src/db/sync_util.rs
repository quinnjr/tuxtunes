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

/// Load a `persistent_id → local id` map for every row in `table`
/// matching `sync_source_id`. Used by reconcilers to avoid N+1 SELECTs.
pub async fn load_pid_to_local_id_map(
    engine: &SqliteRawEngine,
    table: &'static str,
    sync_source_id: i64,
) -> Result<HashMap<String, i64>, SqliteError> {
    let sql = format!("SELECT id, persistent_id FROM {table} WHERE sync_source_id = ?");
    let rows = engine
        .raw_sql_query(&sql, &[FilterValue::Int(sync_source_id)])
        .await?;
    let mut out = HashMap::with_capacity(rows.len());
    for r in rows {
        let v = r.into_json();
        let (Some(id), Some(pid)) = (
            v.get("id").and_then(|v| v.as_i64()),
            v.get("persistent_id").and_then(|v| v.as_str()),
        ) else {
            continue;
        };
        out.insert(pid.to_string(), id);
    }
    Ok(out)
}

/// Delete rows in `table` under `sync_source_id` whose `persistent_id`
/// is not in `keep_hex`. Uses a temporary staging table to side-step
/// SQLite's per-statement parameter limit — important at real iTunes
/// scale (~50K tracks).
pub async fn delete_by_keep_set(
    engine: &SqliteRawEngine,
    table: &'static str,
    sync_source_id: i64,
    keep_hex: &[String],
) -> Result<u64, SqliteError> {
    if keep_hex.is_empty() {
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

    // Batch inserts — 500 rows per statement lets us populate 50K keys
    // in ~100 round-trips instead of 50K.
    const BATCH: usize = 500;
    for chunk in keep_hex.chunks(BATCH) {
        let placeholders = vec!["(?)"; chunk.len()].join(", ");
        let sql = format!("INSERT INTO _sync_keep (persistent_id) VALUES {placeholders}");
        let params: Vec<FilterValue> = chunk
            .iter()
            .map(|h| FilterValue::String(h.clone()))
            .collect();
        engine.raw_sql_execute(&sql, &params).await?;
    }

    let sql = format!(
        "DELETE FROM {table} WHERE sync_source_id = ? \
         AND persistent_id NOT IN (SELECT persistent_id FROM _sync_keep)"
    );
    engine
        .raw_sql_execute(&sql, &[FilterValue::Int(sync_source_id)])
        .await
}
