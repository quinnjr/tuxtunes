//! Typed payloads for every Tauri event the sync engine emits.

use serde::Serialize;

pub const PROGRESS: &str = "sync:progress";
pub const WARNING: &str = "sync:warning";
pub const COMPLETE: &str = "sync:complete";
pub const FAILED: &str = "sync:failed";

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
    ConflictResolved,
    UnknownField,
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
}
