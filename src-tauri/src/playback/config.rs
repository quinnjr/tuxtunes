//! Per-track and per-device mpv property configuration.
//!
//! Given a chosen device, exclusive-mode preference, ReplayGain mode,
//! and the track's own audio format, produce the ordered list of
//! `(property, value)` pairs to apply to mpv before `loadfile`.

use serde::{Deserialize, Serialize};

/// User preferences for the playback engine (from `Preference` table).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct PlaybackPrefs {
    pub selected_device_id: Option<String>,
    pub exclusive_mode: bool,
    pub replaygain_mode: ReplayGainMode,
    pub volume: u8,
}

impl Default for PlaybackPrefs {
    fn default() -> Self {
        Self {
            selected_device_id: None,
            exclusive_mode: false,
            replaygain_mode: ReplayGainMode::Off,
            volume: 100,
        }
    }
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ReplayGainMode {
    Track,
    Album,
    Off,
}

impl ReplayGainMode {
    pub fn as_mpv(self) -> &'static str {
        match self {
            Self::Track => "track",
            Self::Album => "album",
            Self::Off => "no",
        }
    }
}

/// The audio format of a specific track, from its `Track` row.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TrackAudioFormat {
    pub sample_rate: Option<u32>,
    pub bit_depth: Option<u8>,
    pub is_dsd: bool,
}

/// One property-value pair to apply to the mpv handle before `loadfile`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MpvProperty {
    pub name: &'static str,
    pub value: String,
}

/// Build the property list to apply for the given prefs + track.
///
/// Order matters: device first, then exclusive flag, then sample-rate /
/// format. `audio-samplerate = 0` means "match source"; we set an
/// explicit value when the track's rate is known so mpv can switch the
/// device to it in exclusive mode.
pub fn build_properties(prefs: &PlaybackPrefs, track: TrackAudioFormat) -> Vec<MpvProperty> {
    let mut out = Vec::new();

    if let Some(dev) = &prefs.selected_device_id {
        out.push(MpvProperty {
            name: "audio-device",
            value: dev.clone(),
        });
    }

    out.push(MpvProperty {
        name: "audio-exclusive",
        value: if prefs.exclusive_mode { "yes" } else { "no" }.to_string(),
    });

    if let Some(rate) = track.sample_rate {
        out.push(MpvProperty {
            name: "audio-samplerate",
            value: rate.to_string(),
        });
    }

    if let Some(bits) = track.bit_depth {
        let fmt = match bits {
            16 => "s16",
            24 => "s24",
            32 => "s32",
            _ => "float",
        };
        out.push(MpvProperty {
            name: "audio-format",
            value: fmt.into(),
        });
    }

    if track.is_dsd {
        out.push(MpvProperty {
            name: "ad-lavc-o",
            value: "dsd_format=dop".into(),
        });
    }

    out.push(MpvProperty {
        name: "replaygain",
        value: prefs.replaygain_mode.as_mpv().into(),
    });

    out.push(MpvProperty {
        name: "volume",
        value: prefs.volume.to_string(),
    });

    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn no_device_prefs() -> PlaybackPrefs {
        PlaybackPrefs {
            selected_device_id: None,
            exclusive_mode: false,
            ..Default::default()
        }
    }

    #[test]
    fn omits_audio_device_when_none_selected() {
        let p = build_properties(
            &no_device_prefs(),
            TrackAudioFormat {
                sample_rate: None,
                bit_depth: None,
                is_dsd: false,
            },
        );
        assert!(p.iter().find(|x| x.name == "audio-device").is_none());
    }

    #[test]
    fn includes_audio_device_when_selected() {
        let prefs = PlaybackPrefs {
            selected_device_id: Some("alsa/hw:0,0".into()),
            ..Default::default()
        };
        let p = build_properties(
            &prefs,
            TrackAudioFormat {
                sample_rate: None,
                bit_depth: None,
                is_dsd: false,
            },
        );
        let dev = p.iter().find(|x| x.name == "audio-device").unwrap();
        assert_eq!(dev.value, "alsa/hw:0,0");
    }

    #[test]
    fn sets_sample_rate_and_format_when_known() {
        let prefs = no_device_prefs();
        let p = build_properties(
            &prefs,
            TrackAudioFormat {
                sample_rate: Some(96_000),
                bit_depth: Some(24),
                is_dsd: false,
            },
        );
        let rate = p.iter().find(|x| x.name == "audio-samplerate").unwrap();
        let fmt = p.iter().find(|x| x.name == "audio-format").unwrap();
        assert_eq!(rate.value, "96000");
        assert_eq!(fmt.value, "s24");
    }

    #[test]
    fn dsd_track_gets_dop_decoder_option() {
        let prefs = no_device_prefs();
        let p = build_properties(
            &prefs,
            TrackAudioFormat {
                sample_rate: Some(2_822_400),
                bit_depth: None,
                is_dsd: true,
            },
        );
        let decoder = p.iter().find(|x| x.name == "ad-lavc-o").unwrap();
        assert_eq!(decoder.value, "dsd_format=dop");
    }

    #[test]
    fn exclusive_mode_serialises_to_mpv_yes_no() {
        let mut prefs = no_device_prefs();
        prefs.exclusive_mode = true;
        let p = build_properties(
            &prefs,
            TrackAudioFormat {
                sample_rate: None,
                bit_depth: None,
                is_dsd: false,
            },
        );
        let excl = p.iter().find(|x| x.name == "audio-exclusive").unwrap();
        assert_eq!(excl.value, "yes");
    }

    #[test]
    fn replaygain_mode_maps_to_mpv_values() {
        assert_eq!(ReplayGainMode::Track.as_mpv(), "track");
        assert_eq!(ReplayGainMode::Album.as_mpv(), "album");
        assert_eq!(ReplayGainMode::Off.as_mpv(), "no");
    }

    #[test]
    fn volume_always_appears_in_output() {
        let prefs = PlaybackPrefs {
            volume: 42,
            ..Default::default()
        };
        let p = build_properties(
            &prefs,
            TrackAudioFormat {
                sample_rate: None,
                bit_depth: None,
                is_dsd: false,
            },
        );
        let vol = p.iter().find(|x| x.name == "volume").unwrap();
        assert_eq!(vol.value, "42");
    }

    #[test]
    fn audio_format_for_each_bit_depth() {
        // Cover the four arms of the bits → mpv-format match: 16, 24,
        // 32, and the fall-through (e.g. 8-bit, 64-bit) → "float".
        for (bits, expected) in [(16u8, "s16"), (24, "s24"), (32, "s32"), (8, "float")] {
            let p = build_properties(
                &no_device_prefs(),
                TrackAudioFormat {
                    sample_rate: None,
                    bit_depth: Some(bits),
                    is_dsd: false,
                },
            );
            let fmt = p.iter().find(|x| x.name == "audio-format").unwrap();
            assert_eq!(fmt.value, expected, "wrong format for {bits}-bit");
        }
    }

    #[test]
    fn prefs_roundtrip_through_serde() {
        // Exercises Serialize + Deserialize on PlaybackPrefs (and the
        // nested ReplayGainMode).
        let prefs = PlaybackPrefs {
            selected_device_id: Some("pipewire".into()),
            exclusive_mode: true,
            replaygain_mode: ReplayGainMode::Album,
            volume: 75,
        };
        let json = serde_json::to_string(&prefs).unwrap();
        let back: PlaybackPrefs = serde_json::from_str(&json).unwrap();
        assert_eq!(prefs, back);
    }
}
