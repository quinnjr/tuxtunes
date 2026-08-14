//! Typed payloads for file-management events.

use serde::Serialize;

/// Reserved for future per-track ingest progress reporting. Not
/// currently emitted — `fs/ingest.rs` intentionally omits per-track
/// emits to avoid excessive IPC volume during bulk imports. Keeping
/// the constant so consumers don't have to be rewired when it lands.
/// Remove or wire up once a batched/throttled progress design exists
/// for bulk imports.
pub const INGEST_PROGRESS: &str = "fs:ingest-progress";
pub const INGEST_COMPLETE: &str = "fs:ingest-complete";
pub const INGEST_FAILED: &str = "fs:ingest-failed";
pub const ORGANIZE_APPLIED: &str = "fs:organize-applied";
pub const ORGANIZE_FAILED: &str = "fs:organize-failed";
pub const VERIFY_PROGRESS: &str = "fs:verify-progress";
pub const VERIFY_COMPLETE: &str = "fs:verify-complete";
pub const VERIFY_FAILED: &str = "fs:verify-failed";

/// Payload for [`INGEST_PROGRESS`]. See that constant for why this
/// event is reserved and not currently emitted.
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

#[derive(Debug, Clone, Serialize)]
pub struct OrganizeFailed {
    pub track_id: i64,
    pub error: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct VerifyProgress {
    pub current: u64,
    pub total: u64,
}

#[derive(Debug, Clone, Serialize)]
pub struct VerifyComplete {
    pub total: u64,
    pub verified: u64,
    pub missing: u64,
    pub mismatched: u64,
}

#[derive(Debug, Clone, Serialize)]
pub struct VerifyFailed {
    pub message: String,
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
        assert_eq!(ORGANIZE_FAILED, "fs:organize-failed");
        assert_eq!(VERIFY_PROGRESS, "fs:verify-progress");
        assert_eq!(VERIFY_COMPLETE, "fs:verify-complete");
        assert_eq!(VERIFY_FAILED, "fs:verify-failed");
    }
}
