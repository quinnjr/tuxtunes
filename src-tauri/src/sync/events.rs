//! Typed payloads for every Tauri event the sync engine emits.

use serde::Serialize;

pub const PROGRESS: &str = "sync:progress";
pub const WARNING: &str = "sync:warning";
pub const COMPLETE: &str = "sync:complete";
pub const FAILED: &str = "sync:failed";
pub const LOG: &str = "sync:log";

#[derive(Debug, Clone, Copy, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum SyncPhase {
    Decoding,
    PathRemapping,
    Diffing,
    ApplyingTracks,
    ApplyingPlaylists,
    Finalizing,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct SyncProgress {
    pub source_id: i64,
    pub phase: SyncPhase,
    pub current: u64,
    pub total: u64,
    pub message: String,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum WarningKind {
    MissingSourceFile,
    UnmappablePath,
    SmartRuleDecodeFailed,
    /// The criteria decoded, but rules TuxTunes cannot evaluate (media
    /// kind, playlist membership, …) were dropped; the imported rule may
    /// match a superset of iTunes' list.
    SmartRulePartial,
    ConflictResolved,
    UnknownField,
    /// `auto_copy_files` is set on a source but file ingest is GUI-only
    /// (the CLI runs reconcile with no FsCoordinator), so the copy was
    /// skipped. Metadata still reconciled.
    IngestSkipped,
    /// A source entry duplicated one already imported this run — either
    /// the same persistent_id, or the same resolved file path under a
    /// different id (the same physical file imported twice in iTunes).
    /// The repeat was skipped. Real iTunes libraries contain such dupes.
    DuplicateTrack,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct SyncWarning {
    pub source_id: i64,
    pub kind: WarningKind,
    pub detail: String,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct SyncComplete {
    pub source_id: i64,
    pub inserted_tracks: u64,
    pub updated_tracks: u64,
    pub deleted_tracks: u64,
    pub inserted_playlists: u64,
    pub updated_playlists: u64,
    pub deleted_playlists: u64,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct SyncFailed {
    pub source_id: i64,
    pub error: String,
}

/// One narrative line streamed from the import log file to the UI.
#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct LogLine {
    pub source_id: i64,
    pub seq: u64,
    pub line: String,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn progress_serializes_with_snake_case_phase() {
        let p = SyncProgress {
            source_id: 1,
            phase: SyncPhase::ApplyingTracks,
            current: 500,
            total: 40_000,
            message: "batch 5/400".into(),
        };
        let json = serde_json::to_string(&p).unwrap();
        assert!(json.contains(r#""phase":"applying_tracks""#));
        assert!(json.contains(r#""current":500"#));
    }

    #[test]
    fn channel_names_stable() {
        assert_eq!(PROGRESS, "sync:progress");
        assert_eq!(WARNING, "sync:warning");
        assert_eq!(COMPLETE, "sync:complete");
        assert_eq!(FAILED, "sync:failed");
        assert_eq!(LOG, "sync:log");
    }

    #[test]
    fn warning_kind_serializes_snake() {
        let w = SyncWarning {
            source_id: 1,
            kind: WarningKind::MissingSourceFile,
            detail: "x".into(),
        };
        let json = serde_json::to_string(&w).unwrap();
        assert!(json.contains(r#""kind":"missing_source_file""#));
    }

    #[test]
    fn complete_serializes_all_counts() {
        let c = SyncComplete {
            source_id: 3,
            inserted_tracks: 1,
            updated_tracks: 2,
            deleted_tracks: 3,
            inserted_playlists: 4,
            updated_playlists: 5,
            deleted_playlists: 6,
        };
        let json = serde_json::to_string(&c).unwrap();
        for key in [
            "inserted_tracks",
            "updated_tracks",
            "deleted_tracks",
            "inserted_playlists",
            "updated_playlists",
            "deleted_playlists",
        ] {
            assert!(json.contains(key), "missing {key}: {json}");
        }
    }

    #[test]
    fn log_line_serializes_snake_case() {
        let l = LogLine {
            source_id: 7,
            seq: 3,
            line: "applying tracks: 100".into(),
        };
        let json = serde_json::to_string(&l).unwrap();
        assert!(json.contains(r#""source_id":7"#));
        assert!(json.contains(r#""seq":3"#));
        assert!(json.contains(r#""line":"applying tracks: 100""#));
    }
}
