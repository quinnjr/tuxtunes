//! Typed payloads for file-management events.

use serde::Serialize;

pub const INGEST_PROGRESS: &str = "fs:ingest-progress";
pub const INGEST_COMPLETE: &str = "fs:ingest-complete";
pub const INGEST_FAILED: &str = "fs:ingest-failed";
pub const ORGANIZE_APPLIED: &str = "fs:organize-applied";

#[derive(Debug, Clone, Serialize)]
pub struct IngestProgress {
    pub track_id: i64,
    pub current: u64,
    pub total: u64,
    pub message: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct IngestComplete {
    pub track_id: i64,
    pub managed_path: String,
    pub artwork_path: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct IngestFailed {
    pub track_id: i64,
    pub source_path: String,
    pub error: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct OrganizeApplied {
    pub track_id: i64,
    pub old_path: String,
    pub new_path: String,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn channel_names_stable() {
        assert_eq!(INGEST_PROGRESS, "fs:ingest-progress");
        assert_eq!(INGEST_COMPLETE, "fs:ingest-complete");
        assert_eq!(INGEST_FAILED, "fs:ingest-failed");
        assert_eq!(ORGANIZE_APPLIED, "fs:organize-applied");
    }
}
