//! Library-scoped Tauri commands.

use crate::runtime::AppState;
use serde::Serialize;

#[derive(Debug, Serialize, Clone, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub struct LibraryStats {
    pub track_count: i64,
    pub total_duration_ms: i64,
    pub total_size_bytes: i64,
}

#[tauri::command]
pub async fn get_library_stats(state: tauri::State<'_, AppState>) -> Result<LibraryStats, String> {
    let engine = &state.db.engine;

    let track_count: i64 = engine
        .raw_sql_scalar("SELECT COUNT(*) FROM tracks", &[])
        .await
        .map_err(|e| e.to_string())?;

    let total_duration_ms: i64 = engine
        .raw_sql_scalar("SELECT COALESCE(SUM(duration_ms), 0) FROM tracks", &[])
        .await
        .map_err(|e| e.to_string())?;

    let total_size_bytes: i64 = engine
        .raw_sql_scalar("SELECT COALESCE(SUM(size_bytes), 0) FROM tracks", &[])
        .await
        .map_err(|e| e.to_string())?;

    Ok(LibraryStats {
        track_count,
        total_duration_ms,
        total_size_bytes,
    })
}

#[cfg(test)]
mod tests {
    use crate::db::Db;

    #[tokio::test]
    async fn library_stats_zero_on_fresh_db() {
        let tmp = tempfile::NamedTempFile::new().unwrap();
        let db = Db::open(tmp.path()).await.unwrap();
        let engine = &db.engine;

        let track_count: i64 = engine
            .raw_sql_scalar("SELECT COUNT(*) FROM tracks", &[])
            .await
            .unwrap();
        let total_duration_ms: i64 = engine
            .raw_sql_scalar("SELECT COALESCE(SUM(duration_ms), 0) FROM tracks", &[])
            .await
            .unwrap();
        let total_size_bytes: i64 = engine
            .raw_sql_scalar("SELECT COALESCE(SUM(size_bytes), 0) FROM tracks", &[])
            .await
            .unwrap();

        assert_eq!(
            (track_count, total_duration_ms, total_size_bytes),
            (0, 0, 0),
        );
    }
}
