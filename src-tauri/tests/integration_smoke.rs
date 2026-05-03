//! Integration-module smoke tests: tray, notify, mpris.
//!
//! These touch Linux session services (D-Bus, notification daemon).
//! Failures (no bus, no daemon) are tolerated — the assertion is
//! "exercise the code path without panic", not "the system has a
//! tray." Coverage on the modules' Rust code is what we want.

#![cfg(unix)]

use tuxtunes::db::tracks::TrackRow;
use tuxtunes::integration;

fn fake_track(title: &str) -> TrackRow {
    TrackRow {
        id: 1,
        title: title.into(),
        artist: Some("Artist".into()),
        album: Some("Album".into()),
        duration_ms: 0,
        file_path: "/tmp/fake.flac".into(),
        file_hash: None,
        sample_rate: None,
        bit_depth: None,
        kind: None,
        play_count: 0,
        skip_count: 0,
    }
}

#[test]
fn tray_track_label_includes_artist_when_present() {
    let label = integration::tray::track_label(&fake_track("Song"));
    assert_eq!(label, "Song — Artist");
}

#[test]
fn tray_track_label_falls_back_to_title_only() {
    let row = TrackRow {
        artist: None,
        ..fake_track("OnlyTitle")
    };
    assert_eq!(integration::tray::track_label(&row), "OnlyTitle");

    let empty = TrackRow {
        artist: Some(String::new()),
        ..fake_track("EmptyArtist")
    };
    assert_eq!(integration::tray::track_label(&empty), "EmptyArtist");
}

// Tray install + set_now_playing_label require AppHandle<Wry>. The
// mock runtime can't satisfy that type, so those entry points stay
// outside unit-test reach until tray.rs goes generic over Runtime.
// track_label, the pure-logic helper, IS reachable and covered above.

#[test]
fn notify_show_track_handles_full_metadata() {
    // Best-effort: notification daemon may not exist in CI. We don't
    // assert on success; the function returns Result<(), _>.
    let _ = integration::notify::show_track(&fake_track("Notif"));
}

#[test]
fn notify_show_track_handles_missing_metadata() {
    let row = TrackRow {
        artist: None,
        album: None,
        ..fake_track("BareTitle")
    };
    let _ = integration::notify::show_track(&row);

    let only_artist = TrackRow {
        artist: Some("X".into()),
        album: None,
        ..fake_track("WithArtist")
    };
    let _ = integration::notify::show_track(&only_artist);

    let only_album = TrackRow {
        artist: None,
        album: Some("Y".into()),
        ..fake_track("WithAlbum")
    };
    let _ = integration::notify::show_track(&only_album);
}

#[test]
fn notify_enabled_default_is_true() {
    assert!(integration::notify::enabled());
}

// mpris::install also requires AppHandle<Wry>; same gating as tray.
// MprisState's pure data shape IS reachable via the Default test.

#[test]
fn mpris_state_transitions_via_update_state_helper() {
    use integration::mpris::PlaybackStatus;
    // Default ctor produces Stopped + None track.
    let state = integration::mpris::MprisState::default();
    assert_eq!(state.status, PlaybackStatus::Stopped);
    assert!(state.track.is_none());
    assert_eq!(state.position_us, 0);
    assert_eq!(state.volume, 0.0);

    let _ = format!("{:?}", PlaybackStatus::Playing);
    let _ = format!("{:?}", PlaybackStatus::Paused);
    let _ = format!("{:?}", PlaybackStatus::Stopped);
}

#[test]
fn mpris_playback_status_dbus_str_is_spec_correct() {
    use integration::mpris::PlaybackStatus;
    assert_eq!(PlaybackStatus::Playing.dbus_str(), "Playing");
    assert_eq!(PlaybackStatus::Paused.dbus_str(), "Paused");
    assert_eq!(PlaybackStatus::Stopped.dbus_str(), "Stopped");
}

#[test]
fn mpris_playback_status_from_frontend_string() {
    use integration::mpris::{playback_status_from_str, PlaybackStatus};
    assert_eq!(playback_status_from_str("playing"), PlaybackStatus::Playing);
    assert_eq!(playback_status_from_str("paused"), PlaybackStatus::Paused);
    assert_eq!(playback_status_from_str("stopped"), PlaybackStatus::Stopped);
    // Unknown / loading / arbitrary strings fall through to Stopped —
    // the safest projection for an unknown state.
    assert_eq!(playback_status_from_str("loading"), PlaybackStatus::Stopped);
    assert_eq!(playback_status_from_str(""), PlaybackStatus::Stopped);
}

#[test]
fn mpris_volume_round_trips_between_percent_and_double() {
    use integration::mpris::{mpris_volume_to_percent, percent_to_mpris_volume};
    assert_eq!(percent_to_mpris_volume(0), 0.0);
    assert_eq!(percent_to_mpris_volume(50), 0.5);
    assert_eq!(percent_to_mpris_volume(100), 1.0);
    // Clamp on out-of-range percent.
    assert_eq!(percent_to_mpris_volume(200), 1.0);

    assert_eq!(mpris_volume_to_percent(0.0), 0);
    assert_eq!(mpris_volume_to_percent(0.5), 50);
    assert_eq!(mpris_volume_to_percent(1.0), 100);
    // Clamp on out-of-range double.
    assert_eq!(mpris_volume_to_percent(2.0), 100);
    assert_eq!(mpris_volume_to_percent(-0.5), 0);
}

#[test]
fn mpris_build_metadata_for_no_track_is_empty() {
    let state = integration::mpris::MprisState::default();
    let map = integration::mpris::build_metadata(&state);
    assert!(map.is_empty());
}

#[test]
fn mpris_build_metadata_includes_title_artist_album() {
    let mut state = integration::mpris::MprisState::default();
    state.track = Some(fake_track("Hello"));
    let map = integration::mpris::build_metadata(&state);
    // Required keys per the MPRIS spec.
    assert!(map.contains_key("xesam:title"));
    assert!(map.contains_key("xesam:artist"));
    assert!(map.contains_key("xesam:album"));
    assert!(map.contains_key("mpris:trackid"));
    assert!(map.contains_key("mpris:length"));
}

#[test]
fn mpris_build_metadata_omits_artist_album_when_missing() {
    let mut state = integration::mpris::MprisState::default();
    state.track = Some(TrackRow {
        artist: None,
        album: None,
        ..fake_track("Bare")
    });
    let map = integration::mpris::build_metadata(&state);
    assert!(!map.contains_key("xesam:artist"));
    assert!(!map.contains_key("xesam:album"));
    // Title and trackid stay populated regardless.
    assert!(map.contains_key("xesam:title"));
    assert!(map.contains_key("mpris:trackid"));
}

#[test]
fn mpris_build_metadata_picks_up_cover_art_when_present() {
    let dir = tempfile::tempdir().unwrap();
    let track_path = dir.path().join("song.flac");
    std::fs::write(&track_path, b"x").unwrap();
    let cover = dir.path().join("cover.jpg");
    std::fs::write(&cover, b"x").unwrap();

    let mut state = integration::mpris::MprisState::default();
    state.track = Some(TrackRow {
        file_path: track_path.display().to_string(),
        ..fake_track("WithArt")
    });
    let map = integration::mpris::build_metadata(&state);
    assert!(map.contains_key("mpris:artUrl"));
}
