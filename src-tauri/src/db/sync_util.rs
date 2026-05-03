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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::Db;
    use serde::Deserialize;

    async fn tmp() -> Db {
        let tmp = tempfile::NamedTempFile::new().unwrap();
        let db = Db::open(tmp.path()).await.unwrap();
        db.engine
            .raw_sql_execute(
                "INSERT INTO sync_sources (id, name, source_path, path_mappings, \
                 conflict_rules, kind) VALUES (1, 'x', '/x', '[]', '{}', 'itunes_itl')",
                &[],
            )
            .await
            .unwrap();
        db
    }

    #[test]
    fn pid_hex_zero_pads_to_sixteen_chars() {
        assert_eq!(pid_hex(0), "0000000000000000");
        assert_eq!(pid_hex(0xDEAD_BEEF), "00000000deadbeef");
        assert_eq!(pid_hex(u64::MAX), "ffffffffffffffff");
    }

    #[test]
    fn opt_str_handles_some_and_none() {
        assert!(matches!(opt_str(Some("hi")), FilterValue::String(s) if s == "hi"));
        assert!(matches!(opt_str(None), FilterValue::Null));
    }

    #[test]
    fn opt_int_handles_some_and_none() {
        assert!(matches!(opt_int(Some(42)), FilterValue::Int(42)));
        assert!(matches!(opt_int(None), FilterValue::Null));
    }

    #[test]
    fn sqlite_bool_decodes_integers_to_bool() {
        #[derive(Deserialize)]
        struct Holder {
            #[serde(deserialize_with = "sqlite_bool")]
            flag: bool,
        }
        let one: Holder = serde_json::from_str(r#"{"flag":1}"#).unwrap();
        assert!(one.flag);
        let zero: Holder = serde_json::from_str(r#"{"flag":0}"#).unwrap();
        assert!(!zero.flag);
        // Non-integer (e.g. native bool) falls through to false. The
        // helper is forgiving rather than strict — a sync source with
        // unexpected JSON shape still loads cleanly.
        let str_val: Holder = serde_json::from_str(r#"{"flag":"true"}"#).unwrap();
        assert!(!str_val.flag);
    }

    #[tokio::test]
    async fn load_pid_to_local_id_map_round_trips() {
        let db = tmp().await;
        // Two synced rows + one un-synced (NULL persistent_id) the
        // helper should ignore.
        db.engine
            .raw_sql_execute(
                "INSERT INTO playlists (sync_source_id, persistent_id, name, kind, \
                 sort_order, track_entries) \
                 VALUES (1, ?, 'a', 'regular', 0, '[]'), \
                        (1, ?, 'b', 'regular', 0, '[]'), \
                        (1, NULL, 'c', 'regular', 0, '[]')",
                &[
                    FilterValue::String(pid_hex(0xAABB)),
                    FilterValue::String(pid_hex(0xCCDD)),
                ],
            )
            .await
            .unwrap();

        let map = load_pid_to_local_id_map(&db.engine, "playlists", 1)
            .await
            .unwrap();
        assert_eq!(map.len(), 2);
        assert!(map.contains_key(&0xAABB));
        assert!(map.contains_key(&0xCCDD));
    }

    #[tokio::test]
    async fn load_pid_to_local_id_map_skips_unparseable_persistent_ids() {
        let db = tmp().await;
        // Manually-inserted rubbish persistent_id should be silently
        // dropped, not abort the load.
        db.engine
            .raw_sql_execute(
                "INSERT INTO playlists (sync_source_id, persistent_id, name, kind, \
                 sort_order, track_entries) \
                 VALUES (1, 'not-hex', 'a', 'regular', 0, '[]')",
                &[],
            )
            .await
            .unwrap();
        let map = load_pid_to_local_id_map(&db.engine, "playlists", 1)
            .await
            .unwrap();
        assert!(map.is_empty());
    }

    #[tokio::test]
    async fn delete_by_keep_set_with_empty_keep_clears_source() {
        let db = tmp().await;
        // Insert two rows under sync_source 1 and one under a fictional
        // source 99 that the delete must not touch.
        db.engine
            .raw_sql_execute(
                "INSERT INTO sync_sources (id, name, source_path, path_mappings, \
                 conflict_rules, kind) VALUES (99, 'y', '/y', '[]', '{}', 'itunes_itl')",
                &[],
            )
            .await
            .unwrap();
        db.engine
            .raw_sql_execute(
                "INSERT INTO playlists (sync_source_id, persistent_id, name, kind, \
                 sort_order, track_entries) \
                 VALUES (1, ?, 'a', 'regular', 0, '[]'), \
                        (1, ?, 'b', 'regular', 0, '[]'), \
                        (99, ?, 'c', 'regular', 0, '[]')",
                &[
                    FilterValue::String(pid_hex(1)),
                    FilterValue::String(pid_hex(2)),
                    FilterValue::String(pid_hex(3)),
                ],
            )
            .await
            .unwrap();

        let deleted = delete_by_keep_set(&db.engine, "playlists", 1, &[])
            .await
            .unwrap();
        assert_eq!(deleted, 2);
        // Source 99 untouched.
        let remaining: i64 = db
            .engine
            .raw_sql_scalar("SELECT COUNT(*) FROM playlists", &[])
            .await
            .unwrap();
        assert_eq!(remaining, 1);
    }

    #[tokio::test]
    async fn delete_by_keep_set_chunks_more_than_batch() {
        let db = tmp().await;
        // BATCH = 500 — insert 600 rows so the keep set spans two chunks
        // and exercises the multi-INSERT path.
        let mut sql = String::from(
            "INSERT INTO playlists (sync_source_id, persistent_id, name, kind, \
             sort_order, track_entries) VALUES ",
        );
        for i in 1u64..=600 {
            if i > 1 {
                sql.push(',');
            }
            sql.push_str(&format!("(1, '{:016x}', 'p', 'regular', 0, '[]')", i));
        }
        db.engine.raw_sql_execute(&sql, &[]).await.unwrap();

        // Keep the first 550, drop 50.
        let keep: Vec<u64> = (1u64..=550).collect();
        let deleted = delete_by_keep_set(&db.engine, "playlists", 1, &keep)
            .await
            .unwrap();
        assert_eq!(deleted, 50);
    }
}
