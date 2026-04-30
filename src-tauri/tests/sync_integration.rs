//! End-to-end integration smoke for the ITL sync path. Skipped when the
//! user's real iTunes library isn't present (keeps CI green).
//!
//! This test verifies the fixture's shape matches what the Phase 3
//! reconcilers depend on — but does NOT run the reconcilers themselves,
//! because they require a live Tauri `AppHandle` (for progress events).
//! Full end-to-end coverage is provided by the import wizard UI
//! (Task 15) + manual smoke (Task 16).

use std::path::PathBuf;

fn fixture_path() -> Option<PathBuf> {
    let home = std::env::var_os("HOME")?;
    let p = PathBuf::from(home).join("Music/iTunes/iTunes Library.itl");
    if p.exists() {
        Some(p)
    } else {
        None
    }
}

#[test]
fn fixture_shape_matches_reconciler_assumptions() {
    let Some(path) = fixture_path() else {
        eprintln!("skipping: ~/Music/iTunes/iTunes Library.itl not present");
        return;
    };

    let lib = itl_rs::ItlFile::open(&path).expect("open itl");

    let tracks = lib.tracks();
    let playlists = lib.playlists();
    println!(
        "fixture: {} tracks, {} playlists",
        tracks.len(),
        playlists.len(),
    );
    assert!(
        tracks.len() > 1000,
        "expected a full library (>1000 tracks), got {}",
        tracks.len(),
    );
    assert!(!playlists.is_empty(), "expected at least one playlist");

    // Persistent IDs are the reconciler's primary key. They must be
    // populated for the large majority of tracks. (iTunes sometimes
    // writes 0 for broken/incomplete rows — we skip those in the
    // reconciler and count them as warnings.)
    let tracks_with_pid = tracks.iter().filter(|t| t.persistent_id() != 0).count();
    assert!(
        tracks_with_pid * 100 / tracks.len() >= 95,
        "tracks with persistent_id: only {tracks_with_pid}/{} populated",
        tracks.len(),
    );

    // Track file path (iTunes-style URL) must be present for most tracks.
    // Real-world observation: ~80% is typical (cloud/streaming tracks,
    // Apple Music refs, dead links often have no local_path).
    let tracks_with_path = tracks.iter().filter(|t| t.local_path().is_some()).count();
    assert!(
        tracks_with_path * 100 / tracks.len() >= 80,
        "tracks with local_path: only {tracks_with_path}/{} populated",
        tracks.len(),
    );

    // Playlist persistent_id: same story. Reconciler skips pid==0.
    let playlists_with_pid = playlists.iter().filter(|p| p.persistent_id() != 0).count();
    assert!(
        playlists_with_pid * 100 / playlists.len() >= 95,
        "playlists with persistent_id: {playlists_with_pid}/{}",
        playlists.len(),
    );

    // Classifier assumptions.
    let folders = playlists.iter().filter(|p| p.is_folder()).count();
    let smart = playlists.iter().filter(|p| p.is_smart()).count();
    let regular = playlists.len() - folders - smart;
    println!("classification: {regular} regular, {smart} smart, {folders} folder");
    assert!(folders > 0, "expected some folder playlists");
    // Smart count is known-low on iTunes 12+ (Part B scoped out); we
    // only assert it's not negative / nonsensical.
    assert!(smart <= playlists.len());

    // Folder playlists have no direct tracks (by itl-rs definition).
    for p in playlists.iter().filter(|p| p.is_folder()) {
        assert!(
            p.track_ids().is_empty(),
            "folder {:?} has {} tracks",
            p.title(),
            p.track_ids().len(),
        );
    }

    // At least one playlist has a parent (folder nesting present).
    let nested = playlists
        .iter()
        .filter(|p| p.parent_persistent_id().is_some())
        .count();
    assert!(
        nested > 0,
        "expected at least one folder-nested playlist in the fixture",
    );
}
