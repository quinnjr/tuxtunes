//! Desktop notifications fired on track-changed when the main window
//! is unfocused. Notification icon points at the on-disk artwork the
//! phase-4 ingest worker extracted, so the desktop notification daemon
//! shows the album cover without re-decoding the audio file.

use crate::db::tracks::TrackRow;
use std::path::Path;

/// Whether to suppress notifications. Returns true (i.e. fire) by
/// default; exposes a hook for the future `notifications_enabled`
/// preference. Until that lands, all notifications fire when the
/// window is unfocused.
pub fn enabled() -> bool {
    // Reserved for `Preference::notifications_enabled` once the design
    // doc's pref surface lands. Keeping the function so consumers don't
    // have to be rewired when it does.
    true
}

/// Fire a "Now Playing" notification. Best-effort — every step is
/// fallible (notification daemon might be down, artwork might be
/// missing) and the entire path returns Result so the caller can log
/// without taking the app down.
pub fn show_track(row: &TrackRow) -> Result<(), notify_rust::error::Error> {
    if !enabled() {
        return Ok(());
    }

    let mut n = notify_rust::Notification::new();
    n.summary(&row.title);

    // "Artist · Album" — only render the separator when both sides have
    // content, so the body never starts or ends with a stray middot.
    let artist = row.artist.as_deref().unwrap_or("");
    let album = row.album.as_deref().unwrap_or("");
    let body = match (artist.is_empty(), album.is_empty()) {
        (true, true) => String::new(),
        (false, true) => artist.to_string(),
        (true, false) => album.to_string(),
        (false, false) => format!("{artist} · {album}"),
    };
    n.body(&body);
    n.appname("TuxTunes");

    if let Some(path) = artwork_for(row) {
        n.icon(&path);
    } else {
        n.icon("audio-x-generic");
    }

    n.timeout(notify_rust::Timeout::Milliseconds(4000));
    n.show().map(|_| ())
}

/// Resolve the artwork path for a track row. Phase-4 stores extracted
/// covers as `cover.<ext>` next to each managed file; we look for that
/// before falling back to the symbolic icon.
fn artwork_for(row: &TrackRow) -> Option<String> {
    let parent = Path::new(&row.file_path).parent()?;
    for name in &["cover.jpg", "cover.png", "cover.jpeg", "cover.webp"] {
        let candidate = parent.join(name);
        if candidate.exists() {
            return candidate.to_str().map(|s| s.to_string());
        }
    }
    None
}
