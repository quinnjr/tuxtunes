//! Shutting the app down.
//!
//! Three surfaces end the process — the tray's Quit item, MPRIS's
//! `Quit` method, and File ▸ Exit — and `AppHandle::exit` is abrupt:
//! it ends the event loop with `process::exit`, running no destructors
//! and joining no worker. Anything the app still owed the database is
//! simply lost.
//!
//! So they all come through here instead, and any future pre-exit step
//! lands in one place rather than in whichever surface is being edited.

use crate::playback::EngineCommand;
use crate::runtime::AppState;
use std::time::Duration;
use tauri::{AppHandle, Manager, Runtime};

/// How long to let the tracking consumer commit before exiting. It is
/// a spawned task with no join handle to wait on, so this is a grace
/// period rather than a guarantee — enough for a single-row SQLite
/// write, short enough not to feel like a hang.
const DRAIN_GRACE: Duration = Duration::from_millis(250);

/// Stop playback, give the writes it triggers a moment to land, then
/// end the process.
///
/// Stopping first is what makes the play count real: the engine only
/// emits `TrackEnded` — which is what decides play versus skip — when
/// it is told to stop, so quitting mid-track without this records
/// nothing. The same window covers a volume change the user made a
/// moment before quitting, which is persisted from the same channel.
///
/// Not covered: file operations already in flight. The ingest and
/// organize workers copy or rename before committing the new path, so
/// a quit at the wrong moment can still leave a file the database does
/// not know about — the same hazard as closing the window, and one
/// that needs the workers to offer a quiesce handshake.
pub async fn shutdown<R: Runtime>(app: &AppHandle<R>) {
    if let Some(state) = app.try_state::<AppState>() {
        if let Err(e) = state.engine.send(EngineCommand::Stop) {
            log::warn!("shutdown: could not stop playback: {e}");
        }
        tokio::time::sleep(DRAIN_GRACE).await;
    }
    app.exit(0);
}

/// [`shutdown`] from a synchronous callback (a tray menu handler, a
/// D-Bus method). Returns immediately; the exit happens on the async
/// runtime a moment later.
pub fn shutdown_soon<R: Runtime>(app: &AppHandle<R>) {
    let app = app.clone();
    tauri::async_runtime::spawn(async move {
        shutdown(&app).await;
    });
}
