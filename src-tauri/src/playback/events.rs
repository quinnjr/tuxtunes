//! Typed payloads for every Tauri event the playback engine emits.
//!
//! These structs are serialized to JSON and listened to from Angular via
//! `TauriService.listen(...)`. Keep field names snake_case in Rust — the
//! Angular-side service maps to camelCase.

use serde::Serialize;

pub const TRACK_CHANGED: &str = "playback:track-changed";
pub const POSITION_UPDATE: &str = "playback:position-update";
pub const STATE_CHANGED: &str = "playback:state-changed";
pub const DEVICE_CHANGED: &str = "playback:device-changed";
pub const WARNING: &str = "playback:warning";

#[derive(Debug, Clone, Copy, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum PlaybackState {
    Playing,
    Paused,
    Stopped,
    Loading,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct TrackChanged {
    pub track_id: Option<i64>,
    pub prev_track_id: Option<i64>,
}

#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct PositionUpdate {
    pub position_ms: i64,
    pub duration_ms: i64,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct StateChanged {
    pub state: PlaybackState,
}

#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct DeviceChanged {
    pub device_id: Option<String>,
    pub sample_rate: Option<u32>,
    pub bit_depth: Option<u8>,
    pub exclusive: bool,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum WarningKind {
    DsdDowngraded,
    ExclusiveModeFailed,
    SampleRateMismatch,
    LoadFailed,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct Warning {
    pub kind: WarningKind,
    pub detail: String,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn position_update_serializes_to_snake_case() {
        let p = PositionUpdate {
            position_ms: 150,
            duration_ms: 200_000,
        };
        let json = serde_json::to_string(&p).unwrap();
        assert_eq!(json, r#"{"position_ms":150,"duration_ms":200000}"#);
    }

    #[test]
    fn playback_state_serializes_as_lowercase_variant() {
        let s = StateChanged {
            state: PlaybackState::Playing,
        };
        let json = serde_json::to_string(&s).unwrap();
        assert_eq!(json, r#"{"state":"playing"}"#);
    }

    #[test]
    fn warning_kind_serializes_snake_case() {
        let w = Warning {
            kind: WarningKind::DsdDowngraded,
            detail: "no native DSD; using DoP".into(),
        };
        let json = serde_json::to_string(&w).unwrap();
        assert!(json.contains(r#""kind":"dsd_downgraded""#), "got: {json}");
    }

    #[test]
    fn track_changed_serializes_with_null_ids() {
        let t = TrackChanged {
            track_id: None,
            prev_track_id: None,
        };
        let json = serde_json::to_string(&t).unwrap();
        assert_eq!(json, r#"{"track_id":null,"prev_track_id":null}"#);
    }

    #[test]
    fn device_changed_serializes_with_all_fields() {
        let d = DeviceChanged {
            device_id: Some("alsa/hw:0,0".into()),
            sample_rate: Some(96_000),
            bit_depth: Some(24),
            exclusive: true,
        };
        let json = serde_json::to_string(&d).unwrap();
        assert!(json.contains(r#""device_id":"alsa/hw:0,0""#));
        assert!(json.contains(r#""sample_rate":96000"#));
        assert!(json.contains(r#""bit_depth":24"#));
        assert!(json.contains(r#""exclusive":true"#));
    }

    #[test]
    fn event_channel_names_are_stable() {
        // Cross-checks the Tauri event-channel contract the Angular
        // PlaybackService listens on. Changing any of these is a
        // protocol break.
        assert_eq!(TRACK_CHANGED, "playback:track-changed");
        assert_eq!(POSITION_UPDATE, "playback:position-update");
        assert_eq!(STATE_CHANGED, "playback:state-changed");
        assert_eq!(DEVICE_CHANGED, "playback:device-changed");
        assert_eq!(WARNING, "playback:warning");
    }

    #[test]
    fn all_playback_state_variants_exist() {
        // Verify all PlaybackState variants are defined; they will be used
        // by the playback engine for state machine transitions.
        let _ = PlaybackState::Playing;
        let _ = PlaybackState::Paused;
        let _ = PlaybackState::Stopped;
        let _ = PlaybackState::Loading;
    }

    #[test]
    fn all_warning_kinds_exist() {
        // Verify all WarningKind variants are defined; they will be used
        // by the playback engine when emitting warnings.
        let _ = WarningKind::DsdDowngraded;
        let _ = WarningKind::ExclusiveModeFailed;
        let _ = WarningKind::SampleRateMismatch;
        let _ = WarningKind::LoadFailed;
    }
}
