//! The device transport abstraction.
//!
//! A device is an object store, not a filesystem: MTP addresses objects
//! by opaque handle, and only some devices support renaming or report
//! free space. [`DeviceTransport`] is the narrowest interface the sync
//! engine needs, so the platform backends (libmtp, Windows Portable
//! Devices) stay small enough to be the only untested code in the
//! subsystem.

use std::io::{Read, Write};

#[cfg(test)]
pub mod conformance;
#[cfg(test)]
pub mod fake;
pub mod fs;

/// A `/`-separated, device-rooted path.
///
/// Normalised on construction: always leading-slash, never
/// trailing-slash (except the root itself), no empty segments.
#[derive(Debug, Clone, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
pub struct DevicePath(String);

impl DevicePath {
    /// Normalise `raw` into a device-rooted path.
    pub fn new(raw: &str) -> Self {
        let joined = raw
            .split('/')
            .filter(|s| !s.is_empty())
            .collect::<Vec<_>>()
            .join("/");
        Self(format!("/{joined}"))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }

    /// The containing directory, or `None` at the root.
    pub fn parent(&self) -> Option<Self> {
        if self.0 == "/" {
            return None;
        }
        let idx = self.0.rfind('/')?;
        Some(Self(if idx == 0 {
            "/".to_string()
        } else {
            self.0[..idx].to_string()
        }))
    }

    /// The final segment, or `None` at the root.
    pub fn file_name(&self) -> Option<&str> {
        if self.0 == "/" {
            return None;
        }
        self.0.rsplit('/').next().filter(|s| !s.is_empty())
    }

    /// Append one or more segments.
    pub fn join(&self, segment: &str) -> Self {
        Self::new(&format!("{}/{segment}", self.0))
    }

    /// Render `self` relative to the directory `from`, walking up with
    /// `..` as needed. Used by the m3u8 writer so a playlist stays
    /// valid wherever the device mounts its storage.
    pub fn relative_to(&self, from: &Self) -> Option<String> {
        let target: Vec<&str> = self.0.split('/').filter(|s| !s.is_empty()).collect();
        let base: Vec<&str> = from.0.split('/').filter(|s| !s.is_empty()).collect();
        let common = target
            .iter()
            .zip(base.iter())
            .take_while(|(a, b)| a == b)
            .count();
        let mut parts: Vec<&str> = vec![".."; base.len() - common];
        parts.extend_from_slice(&target[common..]);
        if parts.is_empty() {
            return None;
        }
        Some(parts.join("/"))
    }
}

/// An opaque device-side object identifier: an MTP handle, a WPD object
/// id, or (for [`fs::FsTransport`]) the path itself.
#[derive(Debug, Clone, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
pub struct ObjectId(pub String);

/// One entry returned by [`DeviceTransport::list`] or `stat`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ObjectStat {
    pub path: DevicePath,
    pub object_id: ObjectId,
    pub is_dir: bool,
    pub size_bytes: u64,
}

/// Device storage totals, when the device reports them.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
pub struct StorageInfo {
    pub total_bytes: u64,
    pub free_bytes: u64,
}

/// The device's filesystem, which decides how aggressively names must
/// be sanitised.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "snake_case")]
pub enum FilesystemKind {
    Fat32,
    ExFat,
    Ext,
    Unknown,
}

/// What a given transport and device can actually do.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
pub struct Capabilities {
    /// Whether native MTP abstract playlist objects can be created.
    pub playlist_objects: bool,
    pub free_space: bool,
    /// Whether write-then-rename is available. Without it the engine
    /// writes in place and records the manifest row only on success.
    pub rename: bool,
    pub max_path_bytes: usize,
    pub filesystem: FilesystemKind,
}

/// Transport failures, categorised so the engine can decide between
/// aborting, warning, and skipping without matching on strings.
#[derive(Debug, thiserror::Error)]
pub enum TransportError {
    #[error("device disconnected")]
    Disconnected,
    #[error("device is out of space")]
    NoSpace,
    #[error("not found: {0}")]
    NotFound(String),
    #[error("permission denied: {0}")]
    PermissionDenied(String),
    #[error("unsupported operation: {0}")]
    Unsupported(&'static str),
    /// The destination exists but TuxTunes has no manifest row for it,
    /// so it belongs to someone else and must not be overwritten.
    #[error("{0} already exists and was not written by TuxTunes")]
    Occupied(String),
    /// The user cancelled mid-transfer. Not a failure.
    #[error("cancelled")]
    Cancelled,
    #[error("transport error: {0}")]
    Other(#[source] anyhow::Error),
}

/// A connected device's object store.
///
/// `Send + Sync` so the engine's futures stay `Send` under
/// `tokio::spawn`. An implementation wrapping a library that is not
/// thread-safe per handle — `libmtp` is the case in point — owns an
/// internal mutex rather than relaxing this bound; the engine drives
/// one device from one task anyway, so the lock is never contended.
pub trait DeviceTransport: Send + Sync {
    fn capabilities(&self) -> Capabilities;

    /// Immediate children of `dir`. A missing directory yields an empty
    /// list rather than an error, so callers need not pre-check.
    fn list(&self, dir: &DevicePath) -> Result<Vec<ObjectStat>, TransportError>;

    fn stat(&self, path: &DevicePath) -> Result<Option<ObjectStat>, TransportError>;

    /// Create `dir` and any missing ancestors, returning `dir`'s id.
    fn mkdir_all(&self, dir: &DevicePath) -> Result<ObjectId, TransportError>;

    /// Open `path` for writing. The parent directory must already
    /// exist. `size_hint` lets MTP pre-declare the object size.
    fn open_write(
        &self,
        path: &DevicePath,
        size_hint: u64,
    ) -> Result<Box<dyn Write + Send>, TransportError>;

    fn open_read(&self, path: &DevicePath) -> Result<Box<dyn Read + Send>, TransportError>;

    fn rename(&self, from: &DevicePath, to: &DevicePath) -> Result<(), TransportError>;

    fn delete(&self, path: &DevicePath) -> Result<(), TransportError>;

    fn free_space(&self) -> Result<StorageInfo, TransportError>;

    /// Register a native playlist object. `Ok(None)` means the device
    /// or transport has no such concept, which is not an error — the
    /// `.m3u8` file is the guarantee.
    fn create_playlist_object(
        &self,
        path: &DevicePath,
        entries: &[ObjectId],
    ) -> Result<Option<ObjectId>, TransportError>;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn device_path_normalises_slashes() {
        assert_eq!(DevicePath::new("/Music//Bonobo/").as_str(), "/Music/Bonobo");
        assert_eq!(DevicePath::new("Music/x").as_str(), "/Music/x");
        assert_eq!(DevicePath::new("/").as_str(), "/");
        assert_eq!(DevicePath::new("").as_str(), "/");
    }

    #[test]
    fn device_path_parent_and_file_name() {
        let p = DevicePath::new("/Music/Bonobo/Migration/01 Kerala.flac");
        assert_eq!(p.file_name(), Some("01 Kerala.flac"));
        assert_eq!(p.parent().unwrap().as_str(), "/Music/Bonobo/Migration");
        assert_eq!(DevicePath::new("/Music").parent().unwrap().as_str(), "/");
        assert_eq!(DevicePath::new("/").parent(), None);
        assert_eq!(DevicePath::new("/").file_name(), None);
    }

    #[test]
    fn device_path_relative_to_walks_up() {
        let file = DevicePath::new("/Music/Bonobo/Migration/01 Kerala.flac");
        let from = DevicePath::new("/Music/Playlists");
        assert_eq!(
            file.relative_to(&from).unwrap(),
            "../Bonobo/Migration/01 Kerala.flac"
        );
    }

    #[test]
    fn device_path_relative_to_same_dir_has_no_prefix() {
        let file = DevicePath::new("/Music/Playlists/a.m3u8");
        let from = DevicePath::new("/Music/Playlists");
        assert_eq!(file.relative_to(&from).unwrap(), "a.m3u8");
    }

    #[test]
    fn device_path_join_appends_segments() {
        let p = DevicePath::new("/Music");
        assert_eq!(p.join("Bonobo").as_str(), "/Music/Bonobo");
        assert_eq!(p.join("/Bonobo/x/").as_str(), "/Music/Bonobo/x");
    }
}
