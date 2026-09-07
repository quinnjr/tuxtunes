//! Window-chrome and app-lifetime commands.
//!
//! The frontend decides whether to draw its own caption buttons and
//! hairline border per platform. The webview user agent is a fair
//! first guess, but it is configurable (`app.windows[].userAgent`) and
//! subject to UA reduction, so the compiled-in OS is the authority.

/// The OS this binary was built for, as a stable lowercase token
/// (`linux`, `macos`, `windows`, …).
#[tauri::command]
pub fn host_os() -> &'static str {
    std::env::consts::OS
}

/// Quit the whole app.
///
/// Not the same as closing the window: the tray keeps running when the
/// window goes, so File ▸ Exit has to end the process the way the
/// tray's own Quit does.
#[tauri::command]
pub fn quit_app(app: tauri::AppHandle) {
    app.exit(0);
}
