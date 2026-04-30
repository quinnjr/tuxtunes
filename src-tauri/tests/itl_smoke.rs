//! Smoke test: load the user's real iTunes library via itl-rs and confirm
//! the basic accessors work. Skips if no library is present (keeps CI
//! stable — CI has no iTunes library).

use std::path::PathBuf;

fn fixture_path() -> Option<PathBuf> {
    let home = std::env::var_os("HOME")?;
    let path = PathBuf::from(home).join("Music/iTunes/iTunes Library.itl");
    if path.exists() {
        Some(path)
    } else {
        None
    }
}

#[test]
fn loads_real_itl_if_available() {
    let Some(path) = fixture_path() else {
        eprintln!("skipping: ~/Music/iTunes/iTunes Library.itl not present");
        return;
    };
    let lib = itl_rs::ItlFile::open(&path).expect("open .itl");
    assert!(!lib.tracks().is_empty());
    assert!(!lib.playlists().is_empty());

    // At least one track should have a persistent id.
    assert!(lib.tracks().iter().any(|t| t.persistent_id() != 0));
    // At least one playlist should have a title.
    assert!(lib.playlists().iter().any(|p| p.title().is_some()));
}
