pub mod audio;
pub mod convert;
pub mod device;
pub mod library;
pub mod playback;
pub mod playlists;
pub mod preferences;
pub mod releases;
pub mod smart;
pub mod sync;
pub mod window;

/// Open `target` (a folder or a URL) with the desktop's default handler
/// via `xdg-open`. The crate is Linux-only, so xdg-open is the standard
/// cross-DE entry point (`tauri-plugin-shell::Shell::open` is
/// deprecated). The child is reaped on a thread rather than dropped, so
/// clicks do not leave zombie processes behind.
///
/// Tests exercise the callers against temp paths and fake links; with
/// `TUXTUNES_NO_XDG_OPEN` set nothing is launched.
pub(crate) fn xdg_open(target: &std::ffi::OsStr) -> Result<(), String> {
    if std::env::var_os("TUXTUNES_NO_XDG_OPEN").is_some() {
        log::info!(
            "TUXTUNES_NO_XDG_OPEN set; not opening {}",
            target.to_string_lossy()
        );
        return Ok(());
    }
    let mut child = std::process::Command::new("xdg-open")
        .arg(target)
        .spawn()
        .map_err(|e| e.to_string())?;
    std::thread::spawn(move || {
        let _ = child.wait();
    });
    Ok(())
}
