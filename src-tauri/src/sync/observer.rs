//! Sink for sync progress events, decoupling the reconcilers from tauri.

use crate::sync::events::{self, SyncComplete, SyncFailed, SyncProgress, SyncWarning};
use tauri::{AppHandle, Emitter, Runtime};

/// Receives progress/warning/terminal events from a running sync.
pub trait SyncObserver: Send + Sync {
    fn progress(&self, ev: &SyncProgress);
    fn warning(&self, ev: &SyncWarning);
    fn complete(&self, ev: &SyncComplete);
    fn failed(&self, ev: &SyncFailed);
}

/// Discards every event. Used by tests and any caller that doesn't care
/// about progress.
pub struct NoopObserver;

impl SyncObserver for NoopObserver {
    fn progress(&self, _ev: &SyncProgress) {}
    fn warning(&self, _ev: &SyncWarning) {}
    fn complete(&self, _ev: &SyncComplete) {}
    fn failed(&self, _ev: &SyncFailed) {}
}

/// Forwards events to the frontend over the existing Tauri event
/// channels. Emission errors are ignored, matching prior behavior.
pub struct TauriObserver<R: Runtime> {
    app: AppHandle<R>,
}

impl<R: Runtime> TauriObserver<R> {
    pub fn new(app: AppHandle<R>) -> Self {
        Self { app }
    }
}

impl<R: Runtime> SyncObserver for TauriObserver<R> {
    fn progress(&self, ev: &SyncProgress) {
        let _ = self.app.emit(events::PROGRESS, ev);
    }
    fn warning(&self, ev: &SyncWarning) {
        let _ = self.app.emit(events::WARNING, ev);
    }
    fn complete(&self, ev: &SyncComplete) {
        let _ = self.app.emit(events::COMPLETE, ev);
    }
    fn failed(&self, ev: &SyncFailed) {
        let _ = self.app.emit(events::FAILED, ev);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sync::events::{
        SyncComplete, SyncFailed, SyncPhase, SyncProgress, SyncWarning, WarningKind,
    };

    fn sample_progress() -> SyncProgress {
        SyncProgress {
            source_id: 1,
            phase: SyncPhase::ApplyingTracks,
            current: 1,
            total: 2,
            message: "x".into(),
        }
    }

    #[test]
    fn noop_observer_accepts_all_events() {
        let obs = NoopObserver;
        obs.progress(&sample_progress());
        obs.warning(&SyncWarning {
            source_id: 1,
            kind: WarningKind::UnmappablePath,
            detail: "d".into(),
        });
        obs.complete(&SyncComplete {
            source_id: 1,
            inserted_tracks: 0,
            updated_tracks: 0,
            deleted_tracks: 0,
            inserted_playlists: 0,
            updated_playlists: 0,
            deleted_playlists: 0,
        });
        obs.failed(&SyncFailed {
            source_id: 1,
            error: "e".into(),
        });
    }

    #[test]
    fn tauri_observer_emits_without_panicking() {
        let app: tauri::App<tauri::test::MockRuntime> = tauri::test::mock_app();
        let obs = TauriObserver::new(app.handle().clone());
        obs.progress(&sample_progress());
    }
}
