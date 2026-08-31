//! Sink for device-sync events, decoupling the engine from Tauri.

use super::events::{
    self, DeviceComplete, DeviceFailed, DeviceProgress, DeviceWarning,
};
use tauri::{AppHandle, Emitter, Runtime};

/// Receives progress, warning and terminal events from a running sync.
pub trait DeviceObserver: Send + Sync {
    fn progress(&self, ev: &DeviceProgress);
    fn warning(&self, ev: &DeviceWarning);
    fn complete(&self, ev: &DeviceComplete);
    fn failed(&self, ev: &DeviceFailed);
}

/// Discards every event.
pub struct NoopObserver;

impl DeviceObserver for NoopObserver {
    fn progress(&self, _ev: &DeviceProgress) {}
    fn warning(&self, _ev: &DeviceWarning) {}
    fn complete(&self, _ev: &DeviceComplete) {}
    fn failed(&self, _ev: &DeviceFailed) {}
}

/// Forwards events to the frontend. Emission errors are ignored: a
/// closed window must never fail a sync that is otherwise succeeding.
pub struct TauriObserver<R: Runtime> {
    app: AppHandle<R>,
}

impl<R: Runtime> TauriObserver<R> {
    pub fn new(app: AppHandle<R>) -> Self {
        Self { app }
    }
}

impl<R: Runtime> DeviceObserver for TauriObserver<R> {
    fn progress(&self, ev: &DeviceProgress) {
        let _ = self.app.emit(events::PROGRESS, ev);
    }
    fn warning(&self, ev: &DeviceWarning) {
        let _ = self.app.emit(events::WARNING, ev);
    }
    fn complete(&self, ev: &DeviceComplete) {
        let _ = self.app.emit(events::COMPLETE, ev);
    }
    fn failed(&self, ev: &DeviceFailed) {
        let _ = self.app.emit(events::FAILED, ev);
    }
}

/// Captures every event for assertions.
#[cfg(test)]
#[derive(Debug, Default)]
pub struct RecordingObserver {
    inner: std::sync::Mutex<Recorded>,
}

#[cfg(test)]
#[derive(Debug, Default)]
struct Recorded {
    progress: Vec<DeviceProgress>,
    warnings: Vec<DeviceWarning>,
    complete: Option<DeviceComplete>,
    failed: Option<DeviceFailed>,
}

#[cfg(test)]
impl RecordingObserver {
    pub fn progress_events(&self) -> Vec<DeviceProgress> {
        self.inner.lock().unwrap().progress.clone()
    }

    pub fn warnings(&self) -> Vec<DeviceWarning> {
        self.inner.lock().unwrap().warnings.clone()
    }

    /// Every phase seen, in order, with consecutive repeats collapsed.
    pub fn phases(&self) -> Vec<super::events::DevicePhase> {
        let mut out: Vec<super::events::DevicePhase> = Vec::new();
        for p in self.inner.lock().unwrap().progress.iter() {
            if out.last() != Some(&p.phase) {
                out.push(p.phase);
            }
        }
        out
    }

    pub fn complete(&self) -> Option<DeviceComplete> {
        self.inner.lock().unwrap().complete
    }

    pub fn failed(&self) -> Option<DeviceFailed> {
        self.inner.lock().unwrap().failed.clone()
    }

    pub fn has_warning(&self, kind: super::events::DeviceWarningKind) -> bool {
        self.warnings().iter().any(|w| w.kind == kind)
    }
}

#[cfg(test)]
impl DeviceObserver for RecordingObserver {
    fn progress(&self, ev: &DeviceProgress) {
        self.inner.lock().unwrap().progress.push(ev.clone());
    }
    fn warning(&self, ev: &DeviceWarning) {
        self.inner.lock().unwrap().warnings.push(ev.clone());
    }
    fn complete(&self, ev: &DeviceComplete) {
        self.inner.lock().unwrap().complete = Some(*ev);
    }
    fn failed(&self, ev: &DeviceFailed) {
        self.inner.lock().unwrap().failed = Some(ev.clone());
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::device::events::{DevicePhase, DeviceWarningKind};

    fn sample_progress(phase: DevicePhase) -> DeviceProgress {
        DeviceProgress {
            device_id: 1,
            phase,
            current: 1,
            total: 2,
            message: "x".into(),
        }
    }

    #[test]
    fn noop_observer_accepts_every_event() {
        let obs = NoopObserver;
        obs.progress(&sample_progress(DevicePhase::Planning));
        obs.warning(&DeviceWarning {
            device_id: 1,
            kind: DeviceWarningKind::UnsupportedCodec,
            detail: "ape".into(),
        });
        obs.complete(&DeviceComplete::default());
        obs.failed(&DeviceFailed {
            device_id: 1,
            error: "e".into(),
        });
    }

    #[test]
    fn recording_observer_captures_in_order() {
        let obs = RecordingObserver::default();
        obs.progress(&sample_progress(DevicePhase::Planning));
        obs.warning(&DeviceWarning {
            device_id: 1,
            kind: DeviceWarningKind::UnsupportedCodec,
            detail: "ape".into(),
        });
        assert_eq!(obs.progress_events().len(), 1);
        assert_eq!(obs.warnings().len(), 1);
        assert!(obs.has_warning(DeviceWarningKind::UnsupportedCodec));
        assert!(obs.complete().is_none());
    }

    #[test]
    fn phases_collapses_consecutive_repeats() {
        let obs = RecordingObserver::default();
        obs.progress(&sample_progress(DevicePhase::Planning));
        obs.progress(&sample_progress(DevicePhase::Uploading));
        obs.progress(&sample_progress(DevicePhase::Uploading));
        obs.progress(&sample_progress(DevicePhase::Finalizing));
        assert_eq!(
            obs.phases(),
            vec![
                DevicePhase::Planning,
                DevicePhase::Uploading,
                DevicePhase::Finalizing
            ]
        );
    }

    #[test]
    fn tauri_observer_emits_without_panicking() {
        let app: tauri::App<tauri::test::MockRuntime> = tauri::test::mock_app();
        let obs = TauriObserver::new(app.handle().clone());
        obs.progress(&sample_progress(DevicePhase::Uploading));
        obs.complete(&DeviceComplete::default());
    }
}
