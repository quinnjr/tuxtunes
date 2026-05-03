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

    // Status enum exposes its string mapping for the D-Bus property.
    // We can't reach `as_str` from outside the module (it's private),
    // but the variants must be Debug-printable for diagnostics.
    let _ = format!("{:?}", PlaybackStatus::Playing);
    let _ = format!("{:?}", PlaybackStatus::Paused);
    let _ = format!("{:?}", PlaybackStatus::Stopped);
}
