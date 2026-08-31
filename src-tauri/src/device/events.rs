//! Typed payloads for every Tauri event the device sync emits.
//!
//! Mirrors [`crate::sync::events`] so the frontend's established
//! progress/warning/terminal handling carries straight over.

use serde::Serialize;

pub const ATTACHED: &str = "device:attached";
pub const DETACHED: &str = "device:detached";
pub const PROGRESS: &str = "device:progress";
pub const WARNING: &str = "device:warning";
pub const COMPLETE: &str = "device:complete";
pub const FAILED: &str = "device:failed";
pub const LOG: &str = "device:log";

#[derive(Debug, Clone, Copy, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum DevicePhase {
    Enumerating,
    Planning,
    /// Reserved for the transcoding phase; never emitted yet.
    Transcoding,
    Uploading,
    Playlists,
    /// Reserved for the stats pull-back phase; never emitted yet.
    PullingStats,
    Pruning,
    Finalizing,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct DeviceProgress {
    pub device_id: i64,
    pub phase: DevicePhase,
    pub current: u64,
    pub total: u64,
    pub message: String,
}

#[derive(Debug, Clone, Copy, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum DeviceWarningKind {
    /// The device cannot decode this codec and no lossless fallback
    /// applies, so the track was left behind.
    UnsupportedCodec,
    /// The track's file is gone from the library root.
    MissingSourceFile,
    /// A name had to be shortened to fit the device's limit.
    PathTruncated,
    /// Two tracks rendered to the same device path; the second was
    /// suffixed.
    NameCollision,
    /// The `.m3u8` was written but the native playlist object was not.
    /// Not fatal: the file is the guarantee.
    PlaylistObjectFailed,
    UploadFailed,
    DeleteFailed,
    OutOfSpace,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct DeviceWarning {
    pub device_id: i64,
    pub kind: DeviceWarningKind,
    pub detail: String,
}

#[derive(Debug, Clone, Copy, Default, Serialize, PartialEq, Eq)]
pub struct DeviceComplete {
    pub device_id: i64,
    pub added: u64,
    pub replaced: u64,
    pub unchanged: u64,
    pub deleted: u64,
    pub playlists_written: u64,
    pub skipped: u64,
    pub bytes_written: u64,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct DeviceFailed {
    pub device_id: i64,
    pub error: String,
}

/// A device appearing or disappearing.
#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct DevicePresence {
    pub device_id: i64,
    pub name: String,
}

/// One narrative line streamed from the run's log file to the UI.
#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct DeviceLogLine {
    pub device_id: i64,
    pub seq: u64,
    pub line: String,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn channel_names_are_stable() {
        assert_eq!(ATTACHED, "device:attached");
        assert_eq!(DETACHED, "device:detached");
        assert_eq!(PROGRESS, "device:progress");
        assert_eq!(WARNING, "device:warning");
        assert_eq!(COMPLETE, "device:complete");
        assert_eq!(FAILED, "device:failed");
        assert_eq!(LOG, "device:log");
    }

    #[test]
    fn phase_serialises_snake_case() {
        assert_eq!(
            serde_json::to_string(&DevicePhase::PullingStats).unwrap(),
            r#""pulling_stats""#
        );
        assert_eq!(
            serde_json::to_string(&DevicePhase::Uploading).unwrap(),
            r#""uploading""#
        );
    }

    #[test]
    fn warning_kind_serialises_snake_case() {
        let w = DeviceWarning {
            device_id: 1,
            kind: DeviceWarningKind::PlaylistObjectFailed,
            detail: "x".into(),
        };
        let json = serde_json::to_string(&w).unwrap();
        assert!(json.contains(r#""kind":"playlist_object_failed""#), "{json}");
    }

    #[test]
    fn complete_serialises_every_count() {
        let c = DeviceComplete {
            device_id: 3,
            added: 1,
            replaced: 2,
            unchanged: 3,
            deleted: 4,
            playlists_written: 5,
            skipped: 6,
            bytes_written: 7,
        };
        let json = serde_json::to_string(&c).unwrap();
        for key in [
            "added",
            "replaced",
            "unchanged",
            "deleted",
            "playlists_written",
            "skipped",
            "bytes_written",
        ] {
            assert!(json.contains(key), "missing {key}: {json}");
        }
    }
}
