//! A [`DeviceTransport`] over a mounted path.
//!
//! Covers gvfs/mtpfs mounts, SD-card readers, and DAPs in USB
//! mass-storage mode — and, on Windows, gives users a working fallback
//! while the native WPD backend is still beta.

use super::{
    Capabilities, DevicePath, DeviceTransport, FilesystemKind, ObjectId, ObjectStat, StorageInfo,
    TransportError,
};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};

/// Free space is only reported where we have a real syscall for it.
const FREE_SPACE_SUPPORTED: bool = cfg!(unix);

pub struct FsTransport {
    root: PathBuf,
}

impl FsTransport {
    pub fn new(root: PathBuf) -> Self {
        // Resolve the root once so containment is judged against real
        // paths. A device root is very often reached through a symlink
        // (/run/media/... , ~/Music), and comparing un-resolved paths
        // would make every check below meaningless.
        let root = std::fs::canonicalize(&root).unwrap_or(root);
        Self { root }
    }

    /// Map a device path onto a host path, refusing anything that would
    /// escape `root`. `DevicePath` normalisation removes empty
    /// segments but preserves `..`, so this is the check that matters.
    fn resolve(&self, path: &DevicePath) -> Result<PathBuf, TransportError> {
        let denied = || TransportError::PermissionDenied(path.as_str().to_string());
        let mut out = self.root.clone();
        for segment in path.as_str().split('/').filter(|s| !s.is_empty()) {
            if segment == ".." || segment == "." {
                return Err(denied());
            }
            // `DevicePath` normalises on '/' only. A segment carrying a
            // backslash, a drive prefix, or a UNC root is absolute to
            // `PathBuf::push`, which silently discards `self.root` —
            // and the prune phase would then delete outside the mount.
            if segment.contains('\\')
                || segment.contains(':')
                || Path::new(segment).is_absolute()
                || Path::new(segment).components().count() != 1
            {
                return Err(denied());
            }
            out.push(segment);
        }

        // The segment checks are textual, so they cannot see a symlink.
        // Resolve the deepest part of the path that exists and confirm
        // *that* is still inside the root: a symlinked album directory
        // would otherwise let writes — and, worse, prune's deletes —
        // land anywhere on the filesystem.
        let anchor = existing_ancestor(&out);
        if let Ok(real) = std::fs::canonicalize(&anchor) {
            if !real.starts_with(&self.root) {
                return Err(denied());
            }
        }
        if !out.starts_with(&self.root) {
            return Err(denied());
        }
        Ok(out)
    }
}

/// The nearest ancestor of `path` that exists, for canonicalisation.
///
/// `canonicalize` fails on a path that is not there yet, which is the
/// normal case for a file about to be written.
fn existing_ancestor(path: &Path) -> PathBuf {
    let mut cursor = path;
    loop {
        if cursor.exists() {
            return cursor.to_path_buf();
        }
        match cursor.parent() {
            Some(parent) => cursor = parent,
            None => return cursor.to_path_buf(),
        }
    }
}

fn map_io(e: std::io::Error, path: &str) -> TransportError {
    match e.kind() {
        std::io::ErrorKind::NotFound => TransportError::NotFound(path.to_string()),
        std::io::ErrorKind::PermissionDenied => TransportError::PermissionDenied(path.to_string()),
        std::io::ErrorKind::StorageFull => TransportError::NoSpace,
        _ => TransportError::Other(anyhow::Error::from(e)),
    }
}

fn stat_of(path: &DevicePath, host: &Path) -> Result<Option<ObjectStat>, TransportError> {
    match std::fs::metadata(host) {
        Ok(md) => Ok(Some(ObjectStat {
            path: path.clone(),
            object_id: ObjectId(path.as_str().to_string()),
            is_dir: md.is_dir(),
            size_bytes: if md.is_dir() { 0 } else { md.len() },
        })),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(map_io(e, path.as_str())),
    }
}

impl DeviceTransport for FsTransport {
    fn capabilities(&self) -> Capabilities {
        Capabilities {
            playlist_objects: false,
            free_space: FREE_SPACE_SUPPORTED,
            rename: true,
            max_path_bytes: 255,
            filesystem: FilesystemKind::Unknown,
        }
    }

    fn list(&self, dir: &DevicePath) -> Result<Vec<ObjectStat>, TransportError> {
        let host = self.resolve(dir)?;
        let entries = match std::fs::read_dir(&host) {
            Ok(e) => e,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
            Err(e) => return Err(map_io(e, dir.as_str())),
        };
        let mut out = Vec::new();
        for entry in entries {
            let entry = entry.map_err(|e| map_io(e, dir.as_str()))?;
            let name = entry.file_name();
            let Some(name) = name.to_str() else { continue };
            let child = dir.join(name);
            if let Some(stat) = stat_of(&child, &entry.path())? {
                out.push(stat);
            }
        }
        Ok(out)
    }

    fn stat(&self, path: &DevicePath) -> Result<Option<ObjectStat>, TransportError> {
        let host = self.resolve(path)?;
        stat_of(path, &host)
    }

    fn mkdir_all(&self, dir: &DevicePath) -> Result<ObjectId, TransportError> {
        let host = self.resolve(dir)?;
        std::fs::create_dir_all(&host).map_err(|e| map_io(e, dir.as_str()))?;
        Ok(ObjectId(dir.as_str().to_string()))
    }

    fn open_write(
        &self,
        path: &DevicePath,
        _size_hint: u64,
    ) -> Result<Box<dyn Write + Send>, TransportError> {
        let host = self.resolve(path)?;
        // The parent must already exist: the engine calls mkdir_all
        // first, and silently creating it here would hide layout bugs.
        let parent = host
            .parent()
            .ok_or_else(|| TransportError::NotFound("cannot write to the root".into()))?;
        if !parent.is_dir() {
            return Err(TransportError::NotFound(
                path.parent()
                    .map(|p| p.as_str().to_string())
                    .unwrap_or_default(),
            ));
        }
        let file = std::fs::File::create(&host).map_err(|e| map_io(e, path.as_str()))?;
        Ok(Box::new(file))
    }

    fn open_read(&self, path: &DevicePath) -> Result<Box<dyn Read + Send>, TransportError> {
        let host = self.resolve(path)?;
        let file = std::fs::File::open(&host).map_err(|e| map_io(e, path.as_str()))?;
        Ok(Box::new(file))
    }

    fn rename(&self, from: &DevicePath, to: &DevicePath) -> Result<(), TransportError> {
        let a = self.resolve(from)?;
        let b = self.resolve(to)?;
        std::fs::rename(&a, &b).map_err(|e| map_io(e, from.as_str()))
    }

    fn delete(&self, path: &DevicePath) -> Result<(), TransportError> {
        let host = self.resolve(path)?;
        let md = std::fs::metadata(&host).map_err(|e| map_io(e, path.as_str()))?;
        if md.is_dir() {
            std::fs::remove_dir(&host).map_err(|e| map_io(e, path.as_str()))
        } else {
            std::fs::remove_file(&host).map_err(|e| map_io(e, path.as_str()))
        }
    }

    fn free_space(&self) -> Result<StorageInfo, TransportError> {
        free_space_of(&self.root)
    }

    fn create_playlist_object(
        &self,
        _path: &DevicePath,
        _entries: &[ObjectId],
    ) -> Result<Option<ObjectId>, TransportError> {
        // A plain filesystem has no playlist object concept. The
        // `.m3u8` file the engine already wrote is the whole story.
        Ok(None)
    }
}

#[cfg(unix)]
fn free_space_of(root: &Path) -> Result<StorageInfo, TransportError> {
    use std::os::unix::ffi::OsStrExt;
    let c_path = std::ffi::CString::new(root.as_os_str().as_bytes())
        .map_err(|e| TransportError::Other(anyhow::Error::from(e)))?;
    // SAFETY: `c_path` is a valid NUL-terminated string that outlives
    // the call, and `stat` is a fresh, correctly sized allocation that
    // statvfs fully initialises on success.
    let stat = unsafe {
        let mut stat: libc::statvfs = std::mem::zeroed();
        if libc::statvfs(c_path.as_ptr(), &mut stat) != 0 {
            return Err(TransportError::Other(anyhow::Error::from(
                std::io::Error::last_os_error(),
            )));
        }
        stat
    };
    let block = stat.f_frsize as u64;
    Ok(StorageInfo {
        total_bytes: stat.f_blocks as u64 * block,
        free_bytes: stat.f_bavail as u64 * block,
    })
}

#[cfg(not(unix))]
fn free_space_of(_root: &Path) -> Result<StorageInfo, TransportError> {
    Err(TransportError::Unsupported("free_space"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::device::transport::conformance;
    use std::sync::atomic::{AtomicUsize, Ordering};

    #[test]
    fn fs_transport_passes_conformance() {
        let dir = tempfile::tempdir().unwrap();
        let counter = AtomicUsize::new(0);
        conformance::run_suite(&|| {
            // A fresh sub-root per case, so cases cannot collide.
            let n = counter.fetch_add(1, Ordering::Relaxed);
            let root = dir.path().join(format!("case{n}"));
            std::fs::create_dir_all(&root).unwrap();
            Box::new(FsTransport::new(root))
        });
    }

    #[test]
    fn rejects_paths_that_escape_the_root() {
        let dir = tempfile::tempdir().unwrap();
        let t = FsTransport::new(dir.path().to_path_buf());
        let err = t.stat(&DevicePath::new("/../../etc/passwd")).unwrap_err();
        assert!(
            matches!(err, TransportError::PermissionDenied(_)),
            "expected PermissionDenied, got {err:?}"
        );
    }

    #[test]
    fn rejects_windows_separators_and_drive_prefixes() {
        let dir = tempfile::tempdir().unwrap();
        let t = FsTransport::new(dir.path().to_path_buf());
        // A root_path is unvalidated free text, and PathBuf::push
        // discards the root for an absolute component.
        for hostile in [
            r"/C:\Users\me\Music",
            r"/..\..\etc",
            "/C:/Users/me/Music",
            r"/\\server\share",
        ] {
            let err = t.stat(&DevicePath::new(hostile)).unwrap_err();
            assert!(
                matches!(err, TransportError::PermissionDenied(_)),
                "{hostile:?} should be refused, got {err:?}"
            );
        }
    }

    #[test]
    #[cfg(unix)]
    fn a_symlink_cannot_lead_writes_or_deletes_out_of_the_root() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("device");
        let outside = dir.path().join("elsewhere");
        std::fs::create_dir_all(&root).unwrap();
        std::fs::create_dir_all(&outside).unwrap();
        // An escape hatch inside the device root.
        std::os::unix::fs::symlink(&outside, root.join("Music")).unwrap();

        let t = FsTransport::new(root);
        let err = t
            .stat(&DevicePath::new("/Music/x.flac"))
            .expect_err("a symlinked directory must not lead outside the root");
        assert!(
            matches!(err, TransportError::PermissionDenied(_)),
            "got {err:?}"
        );
    }

    #[test]
    #[cfg(unix)]
    fn a_symlinked_root_is_still_usable() {
        // The common case: the device is reached *through* a symlink.
        // Canonicalising the root at construction is what keeps this
        // working while the check above still refuses escapes.
        let dir = tempfile::tempdir().unwrap();
        let real = dir.path().join("real");
        std::fs::create_dir_all(&real).unwrap();
        let link = dir.path().join("link");
        std::os::unix::fs::symlink(&real, &link).unwrap();

        let t = FsTransport::new(link);
        t.mkdir_all(&DevicePath::new("/Music")).unwrap();
        assert!(real.join("Music").is_dir());
    }

    #[test]
    fn reports_no_playlist_object_support() {
        let dir = tempfile::tempdir().unwrap();
        let t = FsTransport::new(dir.path().to_path_buf());
        assert!(!t.capabilities().playlist_objects);
        assert_eq!(
            t.create_playlist_object(&DevicePath::new("/a.m3u8"), &[])
                .unwrap(),
            None
        );
    }
}
