//! Single-engine, thread-safe wrapper over libmpv2.
//!
//! Ownership model:
//! - `Mpv` is owned by a dedicated `std::thread` that drains its event
//!   queue via `Mpv::wait_event`.
//! - Command-handler tasks send `EngineCommand`s over a tokio MPSC
//!   channel to that thread.
//! - The thread emits Tauri events via an `AppHandle` for state changes,
//!   position updates, end-of-file, etc.
//!
//! This keeps the mpv handle confined to one thread and decouples the
//! async command handlers from the blocking event loop.

use crate::playback::config::{build_properties, MpvProperty, PlaybackPrefs, TrackAudioFormat};
use crate::playback::events::{self, PlaybackState, PositionUpdate, StateChanged, TrackChanged};
use libmpv2::events::{Event, PropertyData};
use libmpv2::{Format, Mpv};
use std::sync::{Arc, Mutex};
use std::thread::{self, JoinHandle};
use tauri::{AppHandle, Emitter, Runtime};
use tokio::sync::mpsc;

#[derive(Debug, thiserror::Error)]
pub enum EngineError {
    /// mpv init failed; the inner string is `libmpv2::Error`'s Display
    /// formatting. We store a String rather than the original Error
    /// because `libmpv2::Error::Loadfile` holds an `Rc<Error>` and is
    /// therefore not `Send`, which breaks passing the failure out of the
    /// engine thread.
    #[error("mpv init failed: {0}")]
    Init(String),

    #[error("engine thread has exited")]
    ThreadDown,
}

#[derive(Debug)]
pub enum EngineCommand {
    LoadAndPlay {
        track_id: i64,
        file_path: String,
        prefs: PlaybackPrefs,
        fmt: TrackAudioFormat,
    },
    Pause,
    Resume,
    Stop,
    Seek {
        position_ms: i64,
    },
    SetVolume {
        volume: u8,
    },
    ApplyDevice {
        prefs: PlaybackPrefs,
    },
}

/// Events the engine thread hands off to an async consumer for DB writes.
#[derive(Debug, Clone, Copy)]
pub enum PlaybackTracking {
    TrackEnded {
        track_id: i64,
        position_ms: i64,
        duration_ms: i64,
    },
    VolumeChanged {
        volume: u8,
    },
}

pub struct PlaybackEngine {
    tx: mpsc::UnboundedSender<EngineCommand>,
    /// Device snapshot populated once at thread start. Read via
    /// [`Self::devices_snapshot`] so callers get an owned clone rather
    /// than sharing the lock.
    devices: Arc<Mutex<Vec<super::device::AudioDevice>>>,
    tracking_rx: std::sync::Mutex<Option<mpsc::UnboundedReceiver<PlaybackTracking>>>,
    _thread: JoinHandle<()>,
}

impl PlaybackEngine {
    /// Spawn the engine thread and return a handle. Generic over the
    /// Tauri Runtime so the same code path supports production (Wry)
    /// and tests (MockRuntime). The handle is captured into the worker
    /// thread, so the Runtime parameter doesn't leak into PlaybackEngine
    /// itself — the struct stays runtime-agnostic.
    pub fn spawn<R: Runtime>(app: AppHandle<R>) -> Result<Self, EngineError> {
        let (tx, mut rx) = mpsc::unbounded_channel::<EngineCommand>();
        let (track_tx, track_rx) = mpsc::unbounded_channel::<PlaybackTracking>();
        let devices = Arc::new(Mutex::new(Vec::new()));
        let devices_shared = Arc::clone(&devices);

        // Init must happen on the thread that owns the Mpv so wait_event can
        // hold &mut Mpv without Send/Sync shenanigans. Use a oneshot channel
        // to surface the init result to the spawn() caller. We carry the
        // error as a String because libmpv2::Error holds Rc inside some
        // variants and isn't Send.
        let (init_tx, init_rx) = std::sync::mpsc::sync_channel::<Result<(), String>>(1);

        let thread = thread::Builder::new()
            .name("mpv-event-loop".into())
            .spawn(move || {
                // libmpv refuses to initialize unless LC_NUMERIC is "C"
                // (mpv_create/initialize fail with a "Non-C locale" error).
                // GTK — initialized by Tauri before we get here — calls
                // setlocale(LC_ALL, "") and applies the user's locale
                // process-wide, so restore the one category mpv requires.
                unsafe {
                    libc::setlocale(libc::LC_NUMERIC, c"C".as_ptr());
                }
                let mut mpv = match init_mpv() {
                    Ok(m) => {
                        let _ = init_tx.send(Ok(()));
                        m
                    }
                    Err(e) => {
                        let _ = init_tx.send(Err(e.to_string()));
                        return;
                    }
                };

                if let Ok(list) = super::device::enumerate(&mpv) {
                    if let Ok(mut guard) = devices_shared.lock() {
                        *guard = list;
                    }
                }

                let _ = mpv.observe_property("time-pos", Format::Double, 1);
                let _ = mpv.observe_property("duration", Format::Double, 2);
                let _ = mpv.observe_property("pause", Format::Flag, 3);
                let _ = mpv.observe_property("volume", Format::Int64, 4);

                let mut state = EventLoopState::default();

                loop {
                    while let Ok(cmd) = rx.try_recv() {
                        handle_command(&mpv, cmd, &mut state, &app, &track_tx);
                    }

                    if let Some(Ok(ev)) = mpv.wait_event(0.05) {
                        handle_event(ev, &app, &mut state, &track_tx);
                    }

                    if rx.is_closed() {
                        break;
                    }
                }
            })
            .expect("spawn mpv-event-loop thread");

        match init_rx.recv() {
            Ok(Ok(())) => Ok(Self {
                tx,
                devices,
                tracking_rx: std::sync::Mutex::new(Some(track_rx)),
                _thread: thread,
            }),
            Ok(Err(msg)) => Err(EngineError::Init(msg)),
            Err(_) => Err(EngineError::ThreadDown),
        }
    }

    pub fn send(&self, cmd: EngineCommand) -> Result<(), EngineError> {
        self.tx.send(cmd).map_err(|_| EngineError::ThreadDown)
    }

    pub fn devices_snapshot(&self) -> Vec<super::device::AudioDevice> {
        self.devices.lock().map(|g| g.clone()).unwrap_or_default()
    }

    pub fn take_tracking_rx(&self) -> Option<mpsc::UnboundedReceiver<PlaybackTracking>> {
        self.tracking_rx.lock().ok().and_then(|mut g| g.take())
    }
}

fn init_mpv() -> Result<Mpv, libmpv2::Error> {
    // `TUXTUNES_AO=null` skips opening a real audio device, matching
    // libmpv's null AO. Tests and CI set this so AO_INIT_FAILED isn't
    // raised on machines without ALSA/PulseAudio. Production never
    // sets it, so the runtime audio device is selected normally.
    let null_ao = std::env::var("TUXTUNES_AO").ok().as_deref() == Some("null");
    Mpv::with_initializer(|init| {
        if null_ao {
            init.set_property("ao", "null")?;
            // Tests: don't pace the null output at real time, so a whole
            // song reaches EOF in milliseconds.
            if let Err(e) = init.set_property("ao-null-untimed", true) {
                log::warn!("mpv init: skipping ao-null-untimed: {e}");
            }
        }
        // Best-effort init: an unknown property name on older/newer
        // libmpv versions shouldn't kill startup. Each setter logs and
        // continues. The two strict properties (vid + audio-buffer)
        // remain required because the engine misbehaves without them.
        init.set_property("vid", "no")?;
        init.set_property("audio-buffer", 2.0_f64)?;
        for (name, value) in [
            ("gapless-audio", "yes"),
            ("audio-pitch-correction", "no"),
            // The player owns the queue: a file must actually *end* so
            // mpv raises EndFile(EOF) and the frontend advances.
            // `keep-open=always` (a video-player habit) instead parks
            // playback paused at the last sample and never unloads, so
            // `track-ended` never fires and nothing auto-advances.
            ("keep-open", "no"),
            // Stay alive between files instead of terminating the core.
            ("idle", "yes"),
        ] {
            if let Err(e) = init.set_property(name, value) {
                log::warn!("mpv init: skipping {name}={value}: {e}");
            }
        }
        if let Err(e) = init.set_property("volume-max", 100_i64) {
            log::warn!("mpv init: skipping volume-max=100: {e}");
        }
        Ok(())
    })
}

fn apply_props(mpv: &Mpv, props: &[MpvProperty]) {
    for p in props {
        if let Err(e) = mpv.set_property(p.name, p.value.as_str()) {
            log::warn!("set_property {}={} failed: {e}", p.name, p.value);
        }
    }
}

fn handle_command<R: Runtime>(
    mpv: &Mpv,
    cmd: EngineCommand,
    state: &mut EventLoopState,
    app: &AppHandle<R>,
    track_tx: &mpsc::UnboundedSender<PlaybackTracking>,
) {
    let current_track = &mut state.current_track;
    match cmd {
        EngineCommand::LoadAndPlay {
            track_id,
            file_path,
            prefs,
            fmt,
        } => {
            // Starting a track while another plays: mpv will raise
            // EndFile(STOP) for the old one *after* we record the new
            // current track, so account for the old one here instead.
            if let Some(prev_id) = *current_track {
                let _ = track_tx.send(PlaybackTracking::TrackEnded {
                    track_id: prev_id,
                    position_ms: state.last_position_ms,
                    duration_ms: state.last_duration_ms,
                });
            }
            let props = build_properties(&prefs, fmt);
            apply_props(mpv, &props);
            if let Err(e) = mpv.command("loadfile", &[file_path.as_str(), "replace"]) {
                log::warn!("loadfile failed: {e}");
                return;
            }
            if let Err(e) = mpv.set_property("pause", false) {
                log::warn!("unpause after loadfile failed: {e}");
            }
            let prev = *current_track;
            *current_track = Some(track_id);
            let _ = app.emit(
                events::TRACK_CHANGED,
                TrackChanged {
                    track_id: Some(track_id),
                    prev_track_id: prev,
                },
            );
            let _ = app.emit(
                events::STATE_CHANGED,
                StateChanged {
                    state: PlaybackState::Loading,
                },
            );
        }
        EngineCommand::Pause => {
            let _ = mpv.set_property("pause", true);
        }
        EngineCommand::Resume => {
            let _ = mpv.set_property("pause", false);
        }
        EngineCommand::Stop => {
            let _ = mpv.command("stop", &[]);
            if let Some(prev_id) = *current_track {
                let _ = track_tx.send(PlaybackTracking::TrackEnded {
                    track_id: prev_id,
                    position_ms: state.last_position_ms,
                    duration_ms: state.last_duration_ms,
                });
            }
            *current_track = None;
            let _ = app.emit(
                events::STATE_CHANGED,
                StateChanged {
                    state: PlaybackState::Stopped,
                },
            );
        }
        EngineCommand::Seek { position_ms } => {
            let seconds = position_ms as f64 / 1000.0;
            let _ = mpv.set_property("time-pos", seconds);
        }
        EngineCommand::SetVolume { volume } => {
            let _ = mpv.set_property("volume", volume as i64);
        }
        EngineCommand::ApplyDevice { prefs } => {
            let device_id = prefs.selected_device_id.clone();
            if let Some(dev) = &device_id {
                let _ = mpv.set_property("audio-device", dev.as_str());
            }
            let _ = mpv.set_property(
                "audio-exclusive",
                if prefs.exclusive_mode { "yes" } else { "no" },
            );
            let _ = mpv.set_property("replaygain", prefs.replaygain_mode.as_mpv());
            // Surface the new device state to the frontend so format
            // chips and the settings panel can reflect what's
            // actually active. mpv exposes the resolved sample-rate /
            // bit-depth via `audio-out-params` after the next file
            // load, but nothing currently reads that back: we emit
            // nulls here and no repopulation happens on FileLoaded (its
            // handler only emits Playing state). Refreshing these from
            // `audio-out-params` is a deferred Phase-6 item.
            let _ = app.emit(
                events::DEVICE_CHANGED,
                events::DeviceChanged {
                    device_id,
                    sample_rate: None,
                    bit_depth: None,
                    exclusive: prefs.exclusive_mode,
                },
            );
        }
    }
}

/// Minimum interval between `position-update` events emitted to the UI.
/// mpv observes `time-pos` at ~10-60 Hz; the scrubber only needs ~4 Hz.
const POSITION_EMIT_INTERVAL_MS: i64 = 250;

#[derive(Default)]
struct EventLoopState {
    current_track: Option<i64>,
    last_position_ms: i64,
    last_duration_ms: i64,
    last_emitted_position_ms: i64,
    last_emitted_state: Option<PlaybackState>,
    last_emitted_volume: Option<u8>,
}

impl EventLoopState {
    fn emit_state<R: Runtime>(&mut self, app: &AppHandle<R>, state: PlaybackState) {
        if self.last_emitted_state == Some(state) {
            return;
        }
        self.last_emitted_state = Some(state);
        let _ = app.emit(events::STATE_CHANGED, StateChanged { state });
    }
}

fn handle_event<R: Runtime>(
    event: Event<'_>,
    app: &AppHandle<R>,
    state: &mut EventLoopState,
    track_tx: &mpsc::UnboundedSender<PlaybackTracking>,
) {
    match event {
        Event::PropertyChange { name, change, .. } => match (name, change) {
            ("time-pos", PropertyData::Double(pos)) => {
                state.last_position_ms = (pos * 1000.0) as i64;
                let delta = (state.last_position_ms - state.last_emitted_position_ms).abs();
                if delta >= POSITION_EMIT_INTERVAL_MS {
                    state.last_emitted_position_ms = state.last_position_ms;
                    let _ = app.emit(
                        events::POSITION_UPDATE,
                        PositionUpdate {
                            position_ms: state.last_position_ms,
                            duration_ms: state.last_duration_ms,
                        },
                    );
                }
            }
            ("duration", PropertyData::Double(dur)) => {
                state.last_duration_ms = (dur * 1000.0) as i64;
            }
            ("pause", PropertyData::Flag(paused)) => {
                state.emit_state(
                    app,
                    if paused {
                        PlaybackState::Paused
                    } else {
                        PlaybackState::Playing
                    },
                );
            }
            ("volume", PropertyData::Int64(vol)) => {
                let clamped = vol.clamp(0, 100) as u8;
                if state.last_emitted_volume != Some(clamped) {
                    state.last_emitted_volume = Some(clamped);
                    let _ = app.emit(
                        events::VOLUME_CHANGED,
                        events::VolumeChanged { volume: clamped },
                    );
                    let _ = track_tx.send(PlaybackTracking::VolumeChanged { volume: clamped });
                }
            }
            _ => {}
        },
        Event::FileLoaded => {
            // The command handler emits `Loading` directly (it has no
            // EventLoopState), and mpv's `pause=false` property change
            // can land here before the file is loaded — both leave
            // `last_emitted_state` claiming `Playing` while the UI is
            // sitting on `Loading`. FileLoaded is the authoritative
            // "audio is running" moment, so always emit it.
            state.last_emitted_state = None;
            state.emit_state(app, PlaybackState::Playing);
        }
        Event::EndFile(reason) => {
            // libmpv2's EndFileReason is a `c_uint` alias from
            // libmpv2_sys — 0=EOF, 2=STOP, 3=QUIT, 4=ERROR, 5=REDIRECT.
            // STOP/REDIRECT arrive for the *previous* file after a
            // `loadfile … replace` or a Stop command, by which time
            // `current_track` already names the new track (or was
            // cleared by Stop). Touching state here would wipe the new
            // track — and then its own EOF would never emit
            // `track-ended`, silently killing auto-advance. Only a
            // genuine end of the current file is handled here.
            const REASON_EOF: libmpv2::EndFileReason = 0;
            const REASON_ERROR: libmpv2::EndFileReason = 4;
            if reason != REASON_EOF && reason != REASON_ERROR {
                return;
            }
            let prev = state.current_track;
            if let Some(id) = prev {
                let _ = track_tx.send(PlaybackTracking::TrackEnded {
                    track_id: id,
                    position_ms: state.last_position_ms,
                    duration_ms: state.last_duration_ms,
                });
                if reason == REASON_EOF {
                    let _ = app.emit(events::TRACK_ENDED, events::TrackEnded { track_id: id });
                }
            }
            state.current_track = None;
            state.last_emitted_position_ms = 0;
            state.emit_state(app, PlaybackState::Stopped);
            let _ = app.emit(
                events::TRACK_CHANGED,
                TrackChanged {
                    track_id: None,
                    prev_track_id: prev,
                },
            );
        }
        _ => {}
    }
}

#[cfg(test)]
mod end_file_tests {
    use super::*;
    use tauri::Listener;

    #[allow(clippy::type_complexity)]
    fn harness() -> (
        tauri::App<tauri::test::MockRuntime>,
        EventLoopState,
        mpsc::UnboundedSender<PlaybackTracking>,
        mpsc::UnboundedReceiver<PlaybackTracking>,
        std::sync::Arc<std::sync::Mutex<Vec<String>>>,
    ) {
        let app = tauri::test::mock_app();
        let seen = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
        for name in [
            events::TRACK_ENDED,
            events::TRACK_CHANGED,
            events::STATE_CHANGED,
        ] {
            let seen = std::sync::Arc::clone(&seen);
            app.handle().listen(name, move |e| {
                seen.lock().unwrap().push(format!("{name} {}", e.payload()));
            });
        }
        let (tx, rx) = mpsc::unbounded_channel();
        let state = EventLoopState {
            current_track: Some(7),
            last_position_ms: 12_000,
            last_duration_ms: 200_000,
            ..Default::default()
        };
        (app, state, tx, rx, seen)
    }

    #[test]
    fn end_file_stop_after_replace_leaves_the_new_track_alone() {
        let (app, mut state, tx, mut rx, seen) = harness();
        // The old file's EndFile(STOP) arrives after the new track was recorded.
        handle_event(Event::EndFile(2), app.handle(), &mut state, &tx);
        assert_eq!(
            state.current_track,
            Some(7),
            "replace must not clear the new track"
        );
        assert!(
            seen.lock().unwrap().is_empty(),
            "no events for a replaced file: {:?}",
            seen.lock().unwrap()
        );
        assert!(
            rx.try_recv().is_err(),
            "tracking for the old track is sent by the command, not here"
        );
        // Redirect (5) and quit (3) are the same story.
        handle_event(Event::EndFile(5), app.handle(), &mut state, &tx);
        assert_eq!(state.current_track, Some(7));
    }

    #[test]
    fn end_file_eof_emits_track_ended_and_clears() {
        let (app, mut state, tx, mut rx, seen) = harness();
        handle_event(Event::EndFile(0), app.handle(), &mut state, &tx);
        assert_eq!(state.current_track, None);
        let events = seen.lock().unwrap();
        assert!(
            events
                .iter()
                .any(|e| e.starts_with(events::TRACK_ENDED) && e.contains("\"track_id\":7")),
            "{events:?}"
        );
        assert!(
            events
                .iter()
                .any(|e| e.starts_with(events::TRACK_CHANGED) && e.contains("\"track_id\":null")),
            "{events:?}"
        );
        assert!(matches!(
            rx.try_recv(),
            Ok(PlaybackTracking::TrackEnded {
                track_id: 7,
                position_ms: 12_000,
                ..
            })
        ));
    }

    #[test]
    fn end_file_error_clears_without_track_ended() {
        let (app, mut state, tx, _rx, seen) = harness();
        handle_event(Event::EndFile(4), app.handle(), &mut state, &tx);
        assert_eq!(state.current_track, None);
        let events = seen.lock().unwrap();
        assert!(
            !events.iter().any(|e| e.starts_with(events::TRACK_ENDED)),
            "{events:?}"
        );
        assert!(
            events.iter().any(|e| e.starts_with(events::TRACK_CHANGED)),
            "{events:?}"
        );
    }
}
