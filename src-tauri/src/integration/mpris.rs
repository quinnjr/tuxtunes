//! MPRIS D-Bus server: `org.mpris.MediaPlayer2.tuxtunes`.
//!
//! Surfaces playback state to system trays, lock-screens, and media-key
//! daemons. Bidirectional: D-Bus method calls (Play/Pause/Next/…) flow
//! into Tauri events the frontend's PlaybackService dispatches; engine
//! events (track-changed/state-changed/volume-changed) flow into
//! property updates here so external consumers see fresh state.

use crate::db::tracks::TrackRow;
use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use tauri::{AppHandle, Emitter, Manager, Runtime};
use zbus::object_server::SignalEmitter;
use zbus::zvariant::{ObjectPath, Value};
use zbus::{connection, interface};

/// Runtime-erased actions the zbus interface impls call. Concrete
/// implementations close over an `AppHandle<R>` and erase the runtime
/// type so the structs (which zbus pins to a concrete type via
/// `#[interface]`) don't need to be generic over `R`. The payload
/// goes as `serde_json::Value` so the closure can forward arbitrary
/// shapes to `AppHandle::emit` without needing to be generic itself.
type EmitFn = Arc<dyn Fn(&str, serde_json::Value) + Send + Sync>;
type WindowFn = Arc<dyn Fn() + Send + Sync>;

const BUS_NAME: &str = "org.mpris.MediaPlayer2.tuxtunes";
const OBJECT_PATH: &str = "/org/mpris/MediaPlayer2";

/// MPRIS event channel names. Same dispatch contract as `tray:*` —
/// the frontend's PlaybackService listens and runs the action against
/// its existing state machine.
pub const EVT_MPRIS_PLAY_PAUSE: &str = "mpris:play-pause";
pub const EVT_MPRIS_PLAY: &str = "mpris:play";
pub const EVT_MPRIS_PAUSE: &str = "mpris:pause";
pub const EVT_MPRIS_STOP: &str = "mpris:stop";
pub const EVT_MPRIS_NEXT: &str = "mpris:next";
pub const EVT_MPRIS_PREVIOUS: &str = "mpris:previous";
pub const EVT_MPRIS_SEEK: &str = "mpris:seek";
pub const EVT_MPRIS_SET_POSITION: &str = "mpris:set-position";
pub const EVT_MPRIS_SET_VOLUME: &str = "mpris:set-volume";

/// Shared state read by the MPRIS interface impl and written by the
/// Tauri-event listeners in lib.rs. A plain Mutex is fine — updates
/// are sub-millisecond and contention is bounded by the rate of
/// playback events (≤ a few per second).
#[derive(Debug, Clone, Default)]
pub struct MprisState {
    pub status: PlaybackStatus,
    pub track: Option<TrackRow>,
    pub position_us: i64,
    pub volume: f64,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum PlaybackStatus {
    Playing,
    Paused,
    #[default]
    Stopped,
}

impl PlaybackStatus {
    fn as_str(self) -> &'static str {
        match self {
            Self::Playing => "Playing",
            Self::Paused => "Paused",
            Self::Stopped => "Stopped",
        }
    }
}

pub type SharedState = Arc<Mutex<MprisState>>;

struct MediaPlayer2 {
    raise: WindowFn,
    quit: WindowFn,
}

#[interface(name = "org.mpris.MediaPlayer2")]
impl MediaPlayer2 {
    /// Bring the main window to the front. Implementing Raise lets
    /// system trays and other MPRIS clients un-minimize the player.
    async fn raise(&self) {
        (self.raise)();
    }

    async fn quit(&self) {
        (self.quit)();
    }

    #[zbus(property)]
    fn can_quit(&self) -> bool {
        true
    }

    #[zbus(property)]
    fn can_raise(&self) -> bool {
        true
    }

    #[zbus(property)]
    fn has_track_list(&self) -> bool {
        false
    }

    #[zbus(property)]
    fn identity(&self) -> &str {
        "TuxTunes"
    }

    #[zbus(property)]
    fn desktop_entry(&self) -> &str {
        "tuxtunes"
    }

    #[zbus(property)]
    fn supported_uri_schemes(&self) -> Vec<&str> {
        vec!["file"]
    }

    #[zbus(property)]
    fn supported_mime_types(&self) -> Vec<&str> {
        vec![
            "audio/flac",
            "audio/mpeg",
            "audio/mp4",
            "audio/aac",
            "audio/wav",
            "audio/ogg",
            "audio/x-vorbis+ogg",
            "audio/x-opus+ogg",
            "audio/x-aiff",
        ]
    }
}

struct Player {
    emit: EmitFn,
    state: SharedState,
}

#[interface(name = "org.mpris.MediaPlayer2.Player")]
impl Player {
    async fn play_pause(&self) {
        (self.emit)(EVT_MPRIS_PLAY_PAUSE, serde_json::Value::Null);
    }

    async fn play(&self) {
        (self.emit)(EVT_MPRIS_PLAY, serde_json::Value::Null);
    }

    async fn pause(&self) {
        (self.emit)(EVT_MPRIS_PAUSE, serde_json::Value::Null);
    }

    async fn stop(&self) {
        (self.emit)(EVT_MPRIS_STOP, serde_json::Value::Null);
    }

    async fn next(&self) {
        (self.emit)(EVT_MPRIS_NEXT, serde_json::Value::Null);
    }

    async fn previous(&self) {
        (self.emit)(EVT_MPRIS_PREVIOUS, serde_json::Value::Null);
    }

    /// `offset` is microseconds, positive = forward. The frontend
    /// resolves into an absolute position before calling seek().
    async fn seek(&self, offset: i64) {
        (self.emit)(EVT_MPRIS_SEEK, serde_json::json!(offset));
    }

    /// `position` is microseconds. The track-id arg is the
    /// MPRIS-spec "trackId" object path — we don't validate against
    /// the current track here; the frontend can compare to the
    /// currently-playing id.
    async fn set_position(&self, _track_id: ObjectPath<'_>, position: i64) {
        (self.emit)(EVT_MPRIS_SET_POSITION, serde_json::json!(position));
    }

    async fn open_uri(&self, _uri: String) {
        // OpenUri is optional and we don't expose external URIs.
    }

    #[zbus(property)]
    fn playback_status(&self) -> &str {
        self.state
            .lock()
            .map(|s| s.status.as_str())
            .unwrap_or("Stopped")
    }

    #[zbus(property)]
    fn loop_status(&self) -> &str {
        "None"
    }

    #[zbus(property)]
    fn rate(&self) -> f64 {
        1.0
    }

    #[zbus(property)]
    fn shuffle(&self) -> bool {
        false
    }

    #[zbus(property)]
    fn metadata(&self) -> HashMap<String, zbus::zvariant::OwnedValue> {
        match self.state.lock() {
            Ok(s) => build_metadata(&s),
            Err(_) => HashMap::new(),
        }
    }

    #[zbus(property)]
    fn volume(&self) -> f64 {
        self.state.lock().map(|s| s.volume).unwrap_or(1.0)
    }

    #[zbus(property)]
    async fn set_volume(&self, value: f64) {
        // Spec: 0.0–1.0. Emit as percent for the frontend to range-check
        // and call set_volume on the engine.
        let pct = (value.clamp(0.0, 1.0) * 100.0) as i64;
        (self.emit)(EVT_MPRIS_SET_VOLUME, serde_json::json!(pct));
    }

    #[zbus(property)]
    fn position(&self) -> i64 {
        self.state.lock().map(|s| s.position_us).unwrap_or(0)
    }

    #[zbus(property)]
    fn minimum_rate(&self) -> f64 {
        1.0
    }

    #[zbus(property)]
    fn maximum_rate(&self) -> f64 {
        1.0
    }

    #[zbus(property)]
    fn can_go_next(&self) -> bool {
        true
    }

    #[zbus(property)]
    fn can_go_previous(&self) -> bool {
        false
    }

    #[zbus(property)]
    fn can_play(&self) -> bool {
        true
    }

    #[zbus(property)]
    fn can_pause(&self) -> bool {
        true
    }

    #[zbus(property)]
    fn can_seek(&self) -> bool {
        true
    }

    #[zbus(property)]
    fn can_control(&self) -> bool {
        true
    }
}

/// Build the MPRIS metadata HashMap for the current state. Pure
/// function — no D-Bus interaction — so unit tests can drive it
/// without a session bus. Hands back an empty map when no track is
/// loaded.
pub fn build_metadata(state: &MprisState) -> HashMap<String, zbus::zvariant::OwnedValue> {
    let mut map: HashMap<String, zbus::zvariant::OwnedValue> = HashMap::new();
    let Some(track) = state.track.as_ref() else {
        return map;
    };

    let trackid_path = format!("/org/tuxtunes/track/{}", track.id);
    if let Ok(p) = ObjectPath::try_from(trackid_path) {
        if let Ok(v) = Value::from(p).try_to_owned() {
            map.insert("mpris:trackid".into(), v);
        }
    }

    let length_us = track.duration_ms.saturating_mul(1000);
    if let Ok(v) = Value::from(length_us).try_to_owned() {
        map.insert("mpris:length".into(), v);
    }

    if let Ok(v) = Value::from(track.title.as_str()).try_to_owned() {
        map.insert("xesam:title".into(), v);
    }
    if let Some(artist) = &track.artist {
        if let Ok(v) = Value::from(vec![artist.as_str()]).try_to_owned() {
            map.insert("xesam:artist".into(), v);
        }
    }
    if let Some(album) = &track.album {
        if let Ok(v) = Value::from(album.as_str()).try_to_owned() {
            map.insert("xesam:album".into(), v);
        }
    }

    // Album art URL — file:// URI for the on-disk cover next to the
    // track file. Most notification daemons / lock screens accept it.
    let parent = std::path::Path::new(&track.file_path).parent();
    if let Some(parent) = parent {
        for name in &["cover.jpg", "cover.png", "cover.jpeg", "cover.webp"] {
            let candidate = parent.join(name);
            if candidate.exists() {
                if let Some(s) = candidate.to_str() {
                    let url = format!("file://{s}");
                    if let Ok(v) = Value::from(url).try_to_owned() {
                        map.insert("mpris:artUrl".into(), v);
                    }
                }
                break;
            }
        }
    }

    map
}

/// Convert a 0..=100 percent volume into the MPRIS spec's 0.0..=1.0
/// double. Inverse of `set_volume`'s mapping.
pub fn percent_to_mpris_volume(pct: u8) -> f64 {
    (pct.min(100) as f64) / 100.0
}

/// MPRIS spec → percent: clamp to [0,1] then scale to integer percent.
pub fn mpris_volume_to_percent(value: f64) -> i64 {
    (value.clamp(0.0, 1.0) * 100.0) as i64
}

/// Map a frontend playback state string into the MPRIS PlaybackStatus
/// enum the D-Bus property publishes.
pub fn playback_status_from_str(s: &str) -> PlaybackStatus {
    match s {
        "playing" => PlaybackStatus::Playing,
        "paused" => PlaybackStatus::Paused,
        _ => PlaybackStatus::Stopped,
    }
}

impl PlaybackStatus {
    /// String form of the enum used by the D-Bus property. Public so
    /// callers (and tests) can verify the spec-mandated wording.
    pub fn dbus_str(self) -> &'static str {
        self.as_str()
    }
}

/// Handle returned by [`install`]: holds both the shared state and the
/// live D-Bus connection so callers can emit PropertiesChanged signals
/// in addition to mutating the state.
pub struct Mpris {
    pub state: SharedState,
    pub conn: zbus::Connection,
}

/// Stand the server up on the session bus and return both the shared
/// state and the live connection. Called from lib.rs at startup.
/// Generic over the Tauri runtime so tests can drive it with a
/// MockRuntime AppHandle just like production drives it with Wry.
pub async fn install<R: Runtime>(app: AppHandle<R>) -> zbus::Result<Mpris> {
    install_with_bus_name(app, BUS_NAME).await
}

/// Install variant that lets a caller (typically a test) pick a unique
/// bus name. Production uses [`install`] which pins to the spec name.
pub async fn install_with_bus_name<R: Runtime>(
    app: AppHandle<R>,
    bus_name: &str,
) -> zbus::Result<Mpris> {
    let state: SharedState = Arc::new(Mutex::new(MprisState {
        volume: 1.0,
        ..Default::default()
    }));

    // Three runtime-erased closures the zbus interfaces hold instead
    // of an `AppHandle<R>` directly. Closing over a clone of the
    // handle keeps the runtime parameter out of the struct types
    // (which the `#[interface]` macro pins to a concrete shape).
    let emit_app = app.clone();
    let emit: EmitFn = Arc::new(move |event: &str, payload: serde_json::Value| {
        let _ = emit_app.emit(event, payload);
    });

    let raise_app = app.clone();
    let raise: WindowFn = Arc::new(move || {
        let Some(window) = raise_app.get_webview_window("main") else {
            return;
        };
        let _ = window.show();
        let _ = window.set_focus();
    });

    let quit_app = app.clone();
    let quit: WindowFn = Arc::new(move || {
        quit_app.exit(0);
    });

    let media_player = MediaPlayer2 { raise, quit };
    let player = Player {
        emit,
        state: Arc::clone(&state),
    };

    let conn = connection::Builder::session()?
        .name(bus_name)?
        .serve_at(OBJECT_PATH, media_player)?
        .serve_at(OBJECT_PATH, player)?
        .build()
        .await?;

    Ok(Mpris { state, conn })
}

/// Update the state and emit a `PropertiesChanged` signal so listeners
/// see the new values without polling. Best-effort — the signal is
/// fire-and-forget.
pub async fn update_state<F>(conn: &zbus::Connection, state: &SharedState, f: F) -> zbus::Result<()>
where
    F: FnOnce(&mut MprisState),
{
    {
        let Ok(mut s) = state.lock() else {
            return Ok(());
        };
        f(&mut s);
    }
    let iface = conn
        .object_server()
        .interface::<_, Player>(OBJECT_PATH)
        .await?;
    let emitter: SignalEmitter<'_> = iface.signal_emitter().clone();
    let player = iface.get().await;
    player.playback_status_changed(&emitter).await?;
    player.metadata_changed(&emitter).await?;
    player.position_changed(&emitter).await?;
    player.volume_changed(&emitter).await?;
    Ok(())
}
