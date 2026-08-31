//! An in-memory [`DeviceTransport`] with fault injection.
//!
//! This is what lets the entire sync engine — every phase, every
//! failure path — be tested with no hardware on any platform. Writes
//! buffer and commit only on a clean close, matching a real device
//! where a cable pulled mid-transfer leaves no usable object.

use super::{
    Capabilities, DevicePath, DeviceTransport, FilesystemKind, ObjectId, ObjectStat, StorageInfo,
    TransportError,
};
use std::collections::BTreeMap;
use std::io::{Read, Write};
use std::sync::{Arc, Mutex};

#[derive(Debug, Clone)]
enum Node {
    Dir,
    File(Vec<u8>),
}

/// A fault to inject into the next write. Kept separate from
/// [`TransportError`] because that type is not `Clone`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Fault {
    NoSpace,
    Disconnected,
    Other,
}

impl From<Fault> for TransportError {
    fn from(f: Fault) -> Self {
        match f {
            Fault::NoSpace => TransportError::NoSpace,
            Fault::Disconnected => TransportError::Disconnected,
            Fault::Other => TransportError::Other(anyhow::anyhow!("injected fault")),
        }
    }
}

#[derive(Debug)]
struct Inner {
    nodes: BTreeMap<String, Node>,
    next_write_fault: Option<Fault>,
    playlist_objects_fail: bool,
    total_bytes: u64,
    free_bytes: u64,
    /// Every playlist object registered, for assertions.
    playlist_objects: Vec<(DevicePath, Vec<ObjectId>)>,
}

/// An in-memory device. Cloning shares the same backing store, so a
/// clone handed to the engine still reflects assertions made here.
#[derive(Debug, Clone)]
pub struct FakeTransport {
    inner: Arc<Mutex<Inner>>,
    caps: Capabilities,
}

impl Default for FakeTransport {
    fn default() -> Self {
        Self::new()
    }
}

impl FakeTransport {
    pub fn new() -> Self {
        Self::with_caps(Capabilities {
            playlist_objects: true,
            free_space: true,
            rename: true,
            max_path_bytes: 255,
            filesystem: FilesystemKind::ExFat,
        })
    }

    pub fn with_caps(caps: Capabilities) -> Self {
        let mut nodes = BTreeMap::new();
        nodes.insert("/".to_string(), Node::Dir);
        Self {
            inner: Arc::new(Mutex::new(Inner {
                nodes,
                next_write_fault: None,
                playlist_objects_fail: false,
                total_bytes: 64 * 1024 * 1024 * 1024,
                free_bytes: 32 * 1024 * 1024 * 1024,
                playlist_objects: Vec::new(),
            })),
            caps,
        }
    }

    /// Make the next [`DeviceTransport::open_write`] stream fail on its
    /// first write. The partial object is discarded.
    pub fn fail_next_write_with(&self, fault: Fault) {
        self.inner.lock().unwrap().next_write_fault = Some(fault);
    }

    /// Make [`DeviceTransport::create_playlist_object`] fail, to prove
    /// the engine treats it as a warning rather than a failure.
    pub fn fail_playlist_objects(&self) {
        self.inner.lock().unwrap().playlist_objects_fail = true;
    }

    pub fn set_free_bytes(&self, free: u64) {
        self.inner.lock().unwrap().free_bytes = free;
    }

    /// Every file currently on the device, as `(path, contents)`.
    pub fn files(&self) -> Vec<(String, Vec<u8>)> {
        self.inner
            .lock()
            .unwrap()
            .nodes
            .iter()
            .filter_map(|(k, v)| match v {
                Node::File(b) => Some((k.clone(), b.clone())),
                Node::Dir => None,
            })
            .collect()
    }

    /// The contents of one file, if present.
    pub fn read_to_string(&self, path: &str) -> Option<String> {
        match self.inner.lock().unwrap().nodes.get(DevicePath::new(path).as_str()) {
            Some(Node::File(b)) => Some(String::from_utf8_lossy(b).into_owned()),
            _ => None,
        }
    }

    pub fn playlist_objects(&self) -> Vec<(DevicePath, Vec<ObjectId>)> {
        self.inner.lock().unwrap().playlist_objects.clone()
    }
}

/// Buffers writes and commits them into the store on a clean close.
struct FakeWriter {
    inner: Arc<Mutex<Inner>>,
    path: String,
    buf: Vec<u8>,
    fault: Option<Fault>,
    failed: bool,
    committed: bool,
}

impl FakeWriter {
    fn commit(&mut self) {
        if self.committed || self.failed {
            return;
        }
        self.committed = true;
        let mut inner = self.inner.lock().unwrap();
        inner
            .nodes
            .insert(self.path.clone(), Node::File(std::mem::take(&mut self.buf)));
    }
}

impl Write for FakeWriter {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        if let Some(fault) = self.fault.take() {
            self.failed = true;
            return Err(std::io::Error::other(format!("{fault:?}")));
        }
        self.buf.extend_from_slice(buf);
        Ok(buf.len())
    }

    fn flush(&mut self) -> std::io::Result<()> {
        if self.failed {
            return Err(std::io::Error::other("write already failed"));
        }
        self.commit();
        Ok(())
    }
}

impl Drop for FakeWriter {
    fn drop(&mut self) {
        self.commit();
    }
}

impl DeviceTransport for FakeTransport {
    fn capabilities(&self) -> Capabilities {
        self.caps
    }

    fn list(&self, dir: &DevicePath) -> Result<Vec<ObjectStat>, TransportError> {
        let inner = self.inner.lock().unwrap();
        Ok(inner
            .nodes
            .iter()
            .filter(|(k, _)| {
                let p = DevicePath::new(k);
                p.parent().as_ref() == Some(dir) && k.as_str() != "/"
            })
            .map(|(k, v)| ObjectStat {
                path: DevicePath::new(k),
                object_id: ObjectId(k.clone()),
                is_dir: matches!(v, Node::Dir),
                size_bytes: match v {
                    Node::File(b) => b.len() as u64,
                    Node::Dir => 0,
                },
            })
            .collect())
    }

    fn stat(&self, path: &DevicePath) -> Result<Option<ObjectStat>, TransportError> {
        let inner = self.inner.lock().unwrap();
        Ok(inner.nodes.get(path.as_str()).map(|v| ObjectStat {
            path: path.clone(),
            object_id: ObjectId(path.as_str().to_string()),
            is_dir: matches!(v, Node::Dir),
            size_bytes: match v {
                Node::File(b) => b.len() as u64,
                Node::Dir => 0,
            },
        }))
    }

    fn mkdir_all(&self, dir: &DevicePath) -> Result<ObjectId, TransportError> {
        let mut inner = self.inner.lock().unwrap();
        let mut cursor = DevicePath::new("/");
        for segment in dir.as_str().split('/').filter(|s| !s.is_empty()) {
            cursor = cursor.join(segment);
            match inner.nodes.get(cursor.as_str()) {
                Some(Node::File(_)) => {
                    return Err(TransportError::Other(anyhow::anyhow!(
                        "{} exists as a file",
                        cursor.as_str()
                    )))
                }
                Some(Node::Dir) => {}
                None => {
                    inner.nodes.insert(cursor.as_str().to_string(), Node::Dir);
                }
            }
        }
        Ok(ObjectId(dir.as_str().to_string()))
    }

    fn open_write(
        &self,
        path: &DevicePath,
        _size_hint: u64,
    ) -> Result<Box<dyn Write + Send>, TransportError> {
        let mut inner = self.inner.lock().unwrap();
        let parent = path
            .parent()
            .ok_or_else(|| TransportError::NotFound("cannot write to the root".into()))?;
        if !matches!(inner.nodes.get(parent.as_str()), Some(Node::Dir)) {
            return Err(TransportError::NotFound(parent.as_str().to_string()));
        }
        let fault = inner.next_write_fault.take();
        Ok(Box::new(FakeWriter {
            inner: Arc::clone(&self.inner),
            path: path.as_str().to_string(),
            buf: Vec::new(),
            fault,
            failed: false,
            committed: false,
        }))
    }

    fn open_read(&self, path: &DevicePath) -> Result<Box<dyn Read + Send>, TransportError> {
        let inner = self.inner.lock().unwrap();
        match inner.nodes.get(path.as_str()) {
            Some(Node::File(b)) => Ok(Box::new(std::io::Cursor::new(b.clone()))),
            Some(Node::Dir) => Err(TransportError::Other(anyhow::anyhow!("is a directory"))),
            None => Err(TransportError::NotFound(path.as_str().to_string())),
        }
    }

    fn rename(&self, from: &DevicePath, to: &DevicePath) -> Result<(), TransportError> {
        if !self.caps.rename {
            return Err(TransportError::Unsupported("rename"));
        }
        let mut inner = self.inner.lock().unwrap();
        let prefix = from.as_str().to_string();
        let moves: Vec<String> = inner
            .nodes
            .keys()
            .filter(|k| **k == prefix || k.starts_with(&format!("{prefix}/")))
            .cloned()
            .collect();
        if moves.is_empty() {
            return Err(TransportError::NotFound(prefix));
        }
        for key in moves {
            let node = inner.nodes.remove(&key).expect("key just listed");
            let suffix = &key[prefix.len()..];
            inner
                .nodes
                .insert(format!("{}{suffix}", to.as_str()), node);
        }
        Ok(())
    }

    fn delete(&self, path: &DevicePath) -> Result<(), TransportError> {
        let mut inner = self.inner.lock().unwrap();
        match inner.nodes.get(path.as_str()) {
            None => return Err(TransportError::NotFound(path.as_str().to_string())),
            Some(Node::Dir) => {
                let has_children = inner
                    .nodes
                    .keys()
                    .any(|k| k.starts_with(&format!("{}/", path.as_str())));
                if has_children {
                    return Err(TransportError::Other(anyhow::anyhow!("directory not empty")));
                }
            }
            Some(Node::File(_)) => {}
        }
        inner.nodes.remove(path.as_str());
        Ok(())
    }

    fn free_space(&self) -> Result<StorageInfo, TransportError> {
        let inner = self.inner.lock().unwrap();
        Ok(StorageInfo {
            total_bytes: inner.total_bytes,
            free_bytes: inner.free_bytes,
        })
    }

    fn create_playlist_object(
        &self,
        path: &DevicePath,
        entries: &[ObjectId],
    ) -> Result<Option<ObjectId>, TransportError> {
        if !self.caps.playlist_objects {
            return Ok(None);
        }
        let mut inner = self.inner.lock().unwrap();
        if inner.playlist_objects_fail {
            return Err(TransportError::Other(anyhow::anyhow!(
                "device rejected the playlist object"
            )));
        }
        inner
            .playlist_objects
            .push((path.clone(), entries.to_vec()));
        Ok(Some(ObjectId(format!("pl:{}", path.as_str()))))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::device::transport::conformance;

    #[test]
    fn fake_transport_passes_conformance() {
        conformance::run_suite(&|| Box::new(FakeTransport::new()));
    }

    #[test]
    fn injected_write_failure_leaves_no_file() {
        let t = FakeTransport::new();
        t.mkdir_all(&DevicePath::new("/Music")).unwrap();
        t.fail_next_write_with(Fault::NoSpace);
        let p = DevicePath::new("/Music/a.flac");
        let mut w = t.open_write(&p, 3).unwrap();
        assert!(w.write_all(b"abc").is_err());
        drop(w);
        assert_eq!(t.stat(&p).unwrap(), None);
    }

    #[test]
    fn fault_applies_only_to_the_next_write() {
        let t = FakeTransport::new();
        t.mkdir_all(&DevicePath::new("/Music")).unwrap();
        t.fail_next_write_with(Fault::Other);
        let mut bad = t.open_write(&DevicePath::new("/Music/a.flac"), 3).unwrap();
        assert!(bad.write_all(b"abc").is_err());
        drop(bad);
        let mut good = t.open_write(&DevicePath::new("/Music/b.flac"), 3).unwrap();
        good.write_all(b"abc").unwrap();
        good.flush().unwrap();
        assert!(t.stat(&DevicePath::new("/Music/b.flac")).unwrap().is_some());
    }

    #[test]
    fn playlist_objects_are_recorded() {
        let t = FakeTransport::new();
        let p = DevicePath::new("/Music/Playlists/a.m3u8");
        let id = t
            .create_playlist_object(&p, &[ObjectId("1".into())])
            .unwrap();
        assert!(id.is_some());
        assert_eq!(t.playlist_objects().len(), 1);
    }

    #[test]
    fn playlist_object_failure_is_injectable() {
        let t = FakeTransport::new();
        t.fail_playlist_objects();
        assert!(t
            .create_playlist_object(&DevicePath::new("/a.m3u8"), &[])
            .is_err());
    }

    #[test]
    fn deleting_a_non_empty_directory_errors() {
        let t = FakeTransport::new();
        t.mkdir_all(&DevicePath::new("/Music/A")).unwrap();
        assert!(t.delete(&DevicePath::new("/Music")).is_err());
    }
}
