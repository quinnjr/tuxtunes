//! One behavioural suite, run against every [`DeviceTransport`].
//!
//! `FsTransport` and `FakeTransport` run it in CI. The platform
//! backends added in later phases register the same suite behind
//! `#[ignore]`, so a developer holding the hardware runs it with
//! `cargo test -- --ignored` and gets identical coverage.

use super::{DevicePath, DeviceTransport, TransportError};
use std::io::{Read, Write};

/// Build a fresh, empty transport. Called once per case so the cases
/// cannot contaminate one another.
pub type Factory<'a> = dyn Fn() -> Box<dyn DeviceTransport> + 'a;

pub fn run_suite(make: &Factory<'_>) {
    mkdir_then_stat_reports_dir(make());
    write_then_read_roundtrips(make());
    list_returns_children_only(make());
    list_of_missing_dir_is_empty(make());
    delete_removes_file(make());
    rename_moves_file(make());
    stat_missing_returns_none(make());
    write_into_missing_parent_errors(make());
    free_space_is_reported(make());
    overwrite_replaces_contents(make());
}

fn mkdir_then_stat_reports_dir(t: Box<dyn DeviceTransport>) {
    let dir = DevicePath::new("/Music/Bonobo/Migration");
    t.mkdir_all(&dir).expect("mkdir_all");
    let stat = t.stat(&dir).expect("stat").expect("dir exists");
    assert!(stat.is_dir, "mkdir_all should create a directory");
    assert_eq!(stat.path, dir);
    // Ancestors are created too.
    assert!(t.stat(&DevicePath::new("/Music")).unwrap().unwrap().is_dir);
}

fn write_then_read_roundtrips(t: Box<dyn DeviceTransport>) {
    let p = DevicePath::new("/Music/a.flac");
    t.mkdir_all(&DevicePath::new("/Music")).unwrap();
    {
        let mut w = t.open_write(&p, 3).expect("open_write");
        w.write_all(b"abc").expect("write_all");
        w.flush().expect("flush");
    }
    let mut buf = Vec::new();
    t.open_read(&p).expect("open_read").read_to_end(&mut buf).unwrap();
    assert_eq!(buf, b"abc");
    let stat = t.stat(&p).unwrap().expect("file exists");
    assert!(!stat.is_dir);
    assert_eq!(stat.size_bytes, 3);
}

fn list_returns_children_only(t: Box<dyn DeviceTransport>) {
    t.mkdir_all(&DevicePath::new("/Music/A/deep")).unwrap();
    write_file(&*t, "/Music/one.flac", b"1");
    write_file(&*t, "/Music/A/two.flac", b"22");
    let mut names: Vec<String> = t
        .list(&DevicePath::new("/Music"))
        .unwrap()
        .into_iter()
        .map(|s| s.path.file_name().unwrap().to_string())
        .collect();
    names.sort();
    assert_eq!(names, vec!["A".to_string(), "one.flac".to_string()]);
}

fn list_of_missing_dir_is_empty(t: Box<dyn DeviceTransport>) {
    assert!(t.list(&DevicePath::new("/nope/nothing")).unwrap().is_empty());
}

fn delete_removes_file(t: Box<dyn DeviceTransport>) {
    let p = DevicePath::new("/Music/a.flac");
    t.mkdir_all(&DevicePath::new("/Music")).unwrap();
    write_file(&*t, "/Music/a.flac", b"abc");
    t.delete(&p).expect("delete");
    assert_eq!(t.stat(&p).unwrap(), None);
}

fn rename_moves_file(t: Box<dyn DeviceTransport>) {
    if !t.capabilities().rename {
        return;
    }
    t.mkdir_all(&DevicePath::new("/Music")).unwrap();
    write_file(&*t, "/Music/a.tuxpart", b"abc");
    let from = DevicePath::new("/Music/a.tuxpart");
    let to = DevicePath::new("/Music/a.flac");
    t.rename(&from, &to).expect("rename");
    assert_eq!(t.stat(&from).unwrap(), None);
    assert_eq!(t.stat(&to).unwrap().unwrap().size_bytes, 3);
}

fn stat_missing_returns_none(t: Box<dyn DeviceTransport>) {
    assert_eq!(t.stat(&DevicePath::new("/Music/ghost.flac")).unwrap(), None);
}

fn write_into_missing_parent_errors(t: Box<dyn DeviceTransport>) {
    let err = t
        .open_write(&DevicePath::new("/Music/nope/a.flac"), 1)
        .and_then(|mut w| {
            w.write_all(b"x").map_err(|e| TransportError::Other(e.into()))?;
            w.flush().map_err(|e| TransportError::Other(e.into()))
        })
        .expect_err("writing under a missing parent must fail");
    assert!(
        matches!(err, TransportError::NotFound(_)),
        "expected NotFound, got {err:?}"
    );
}

fn free_space_is_reported(t: Box<dyn DeviceTransport>) {
    if !t.capabilities().free_space {
        return;
    }
    let info = t.free_space().expect("free_space");
    assert!(info.total_bytes > 0, "total_bytes should be reported");
    assert!(info.free_bytes <= info.total_bytes);
}

fn overwrite_replaces_contents(t: Box<dyn DeviceTransport>) {
    t.mkdir_all(&DevicePath::new("/Music")).unwrap();
    write_file(&*t, "/Music/a.flac", b"aaaaa");
    write_file(&*t, "/Music/a.flac", b"bb");
    let mut buf = Vec::new();
    t.open_read(&DevicePath::new("/Music/a.flac"))
        .unwrap()
        .read_to_end(&mut buf)
        .unwrap();
    assert_eq!(buf, b"bb", "an overwrite must truncate, not merge");
}

fn write_file(t: &dyn DeviceTransport, path: &str, bytes: &[u8]) {
    let p = DevicePath::new(path);
    let mut w = t.open_write(&p, bytes.len() as u64).expect("open_write");
    w.write_all(bytes).expect("write_all");
    w.flush().expect("flush");
}
