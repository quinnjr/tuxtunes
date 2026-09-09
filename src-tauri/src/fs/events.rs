//! Typed payloads for file-management events.

use serde::Serialize;

/// Reserved for future per-track ingest progress reporting. Not
/// currently emitted — `fs/ingest.rs` intentionally omits per-track
/// emits to avoid excessive IPC volume during bulk imports. Keeping
/// the constant so consumers don't have to be rewired when it lands.
/// Remove or wire up once a batched/throttled progress design exists
/// for bulk imports.
/// Rows appeared or changed outside the UI's own actions: the DB watcher
/// fires it on any foreign commit, and workers that insert rows nudge it
/// directly so the list does not wait out the poll interval.
pub const LIBRARY_CHANGED: &str = "library:external-change";
pub const INGEST_PROGRESS: &str = "fs:ingest-progress";
pub const INGEST_COMPLETE: &str = "fs:ingest-complete";
pub const INGEST_FAILED: &str = "fs:ingest-failed";
pub const ORGANIZE_APPLIED: &str = "fs:organize-applied";
pub const ORGANIZE_FAILED: &str = "fs:organize-failed";
pub const CONSOLIDATE_PROGRESS: &str = "fs:consolidate-progress";
pub const CONSOLIDATE_COMPLETE: &str = "fs:consolidate-complete";
pub const RECLAIM_PROGRESS: &str = "fs:reclaim-progress";
pub const RECLAIM_COMPLETE: &str = "fs:reclaim-complete";
pub const VERIFY_PROGRESS: &str = "fs:verify-progress";
pub const VERIFY_COMPLETE: &str = "fs:verify-complete";
pub const VERIFY_FAILED: &str = "fs:verify-failed";
pub const CONVERT_PROGRESS: &str = "fs:convert-progress";
pub const CONVERT_COMPLETE: &str = "fs:convert-complete";
pub const CONVERT_FAILED: &str = "fs:convert-failed";

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
pub struct ConsolidateProgress {
    pub current: u64,
    pub total: u64,
}

#[derive(Debug, Clone, Serialize)]
pub struct ConsolidateComplete {
    pub total: u64,
    /// Files moved within the library root to match the scheme.
    pub moved: u64,
    /// Files copied in from outside the library root.
    pub copied: u64,
    /// Files already at the path the scheme asks for.
    pub in_place: u64,
    /// Rows whose file is not on disk at all. Counted apart from
    /// `failed` because there is nothing wrong with the pass — an
    /// import can carry in thousands of rows for files that were
    /// already gone, and reporting those as failures is alarming and
    /// useless.
    pub missing: u64,
    pub failed: u64,
}

#[derive(Debug, Clone, Serialize)]
pub struct ReclaimProgress {
    pub current: u64,
    pub total: u64,
}

#[derive(Debug, Clone, Serialize)]
pub struct ReclaimComplete {
    /// Originals sent to the trash.
    pub reclaimed: u64,
    /// Bytes those originals occupied.
    pub bytes_freed: u64,
    /// Originals left alone: the copy did not match, or one of the two
    /// files was not there.
    pub skipped: u64,
    pub failed: u64,
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
    pub relinked: u64,
}

#[derive(Debug, Clone, Serialize)]
pub struct VerifyFailed {
    pub message: String,
}

/// Emitted before each file in a convert batch starts encoding, so the
/// UI can name what it is working on. `current` is zero-based.
#[derive(Debug, Clone, Serialize)]
pub struct ConvertProgress {
    pub current: u64,
    pub total: u64,
    pub title: String,
    /// How far through this file the encoder is, or `None` when the
    /// library does not know the track's duration and there is nothing
    /// to measure against.
    pub percent: Option<u8>,
}

#[derive(Debug, Clone, Serialize)]
pub struct ConvertComplete {
    pub total: u64,
    pub converted: u64,
    pub failed: u64,
    /// Converted files that were also added to the library as tracks.
    pub added_to_library: u64,
    /// Whether the batch stopped early because the user cancelled.
    pub cancelled: bool,
    /// Target extension, so a UI showing several batches can label them.
    pub format: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct ConvertFailed {
    pub track_id: i64,
    pub title: String,
    pub error: String,
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
        assert_eq!(CONSOLIDATE_PROGRESS, "fs:consolidate-progress");
        assert_eq!(CONSOLIDATE_COMPLETE, "fs:consolidate-complete");
        assert_eq!(RECLAIM_PROGRESS, "fs:reclaim-progress");
        assert_eq!(RECLAIM_COMPLETE, "fs:reclaim-complete");
        assert_eq!(VERIFY_PROGRESS, "fs:verify-progress");
        assert_eq!(VERIFY_COMPLETE, "fs:verify-complete");
        assert_eq!(VERIFY_FAILED, "fs:verify-failed");
        assert_eq!(CONVERT_PROGRESS, "fs:convert-progress");
        assert_eq!(CONVERT_COMPLETE, "fs:convert-complete");
        assert_eq!(CONVERT_FAILED, "fs:convert-failed");
    }
}
