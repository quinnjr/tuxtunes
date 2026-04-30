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
use tauri::{AppHandle, Emitter};
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
}

pub struct PlaybackEngine {
    tx: mpsc::UnboundedSender<EngineCommand>,
    /// Device snapshot populated once at thread start.
    pub devices: Arc<Mutex<Vec<super::device::AudioDevice>>>,
    tracking_rx: std::sync::Mutex<Option<mpsc::UnboundedReceiver<PlaybackTracking>>>,
    _thread: JoinHandle<()>,
}

impl PlaybackEngine {
    /// Spawn the engine thread and return a handle.
    pub fn spawn(app: AppHandle) -> Result<Self, EngineError> {
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

                let mut current_track: Option<i64> = None;
                let mut last_position_ms: i64 = 0;
                let mut last_duration_ms: i64 = 0;

                loop {
                    while let Ok(cmd) = rx.try_recv() {
                        handle_command(&mpv, cmd, &mut current_track, &app);
                    }

                    if let Some(Ok(ev)) = mpv.wait_event(0.05) {
                        handle_event(
                            ev,
                            &app,
                            &mut current_track,
                            &mut last_position_ms,
                            &mut last_duration_ms,
                            &track_tx,
                        );
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
    Mpv::with_initializer(|init| {
        init.set_property("vid", "no")?;
        init.set_property("gapless-audio", "yes")?;
        init.set_property("audio-pitch-correction", "no")?;
        init.set_property("audio-resample-mode", "no")?;
        init.set_property("keep-open", "always")?;
        init.set_property("audio-buffer", 2.0_f64)?;
        init.set_property("volume-max", 100_i64)?;
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

fn handle_command(mpv: &Mpv, cmd: EngineCommand, current_track: &mut Option<i64>, app: &AppHandle) {
    match cmd {
        EngineCommand::LoadAndPlay {
            track_id,
            file_path,
            prefs,
            fmt,
        } => {
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
            if let Some(dev) = prefs.selected_device_id {
                let _ = mpv.set_property("audio-device", dev.as_str());
            }
            let _ = mpv.set_property(
                "audio-exclusive",
                if prefs.exclusive_mode { "yes" } else { "no" },
            );
        }
    }
}

fn handle_event(
    event: Event<'_>,
    app: &AppHandle,
    current_track: &mut Option<i64>,
    last_position_ms: &mut i64,
    last_duration_ms: &mut i64,
    track_tx: &mpsc::UnboundedSender<PlaybackTracking>,
) {
    match event {
        Event::PropertyChange { name, change, .. } => match (name, change) {
            ("time-pos", PropertyData::Double(pos)) => {
                *last_position_ms = (pos * 1000.0) as i64;
                let _ = app.emit(
                    events::POSITION_UPDATE,
                    PositionUpdate {
                        position_ms: *last_position_ms,
                        duration_ms: *last_duration_ms,
                    },
                );
            }
            ("duration", PropertyData::Double(dur)) => {
                *last_duration_ms = (dur * 1000.0) as i64;
            }
            ("pause", PropertyData::Flag(paused)) => {
                let state = if paused {
                    PlaybackState::Paused
                } else {
                    PlaybackState::Playing
                };
                let _ = app.emit(events::STATE_CHANGED, StateChanged { state });
            }
            _ => {}
        },
        Event::FileLoaded => {
            let _ = app.emit(
                events::STATE_CHANGED,
                StateChanged {
                    state: PlaybackState::Playing,
                },
            );
        }
        Event::EndFile(_) => {
            let prev = *current_track;
            if let Some(id) = prev {
                let _ = track_tx.send(PlaybackTracking::TrackEnded {
                    track_id: id,
                    position_ms: *last_position_ms,
                    duration_ms: *last_duration_ms,
                });
            }
            *current_track = None;
            let _ = app.emit(
                events::STATE_CHANGED,
                StateChanged {
                    state: PlaybackState::Stopped,
                },
            );
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
