//! Window-chrome support commands.
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

/// Quit the app, the same way the tray's Quit item and MPRIS's `Quit`
/// method do — through the shared shutdown, so playback is stopped and
/// what it owes the database is written before the process ends.
///
/// Closing the window quits too (nothing holds the app open once the
/// last window is destroyed); this is the menu's way of asking for the
/// same thing.
#[tauri::command]
pub async fn quit_app(app: tauri::AppHandle) -> Result<(), String> {
    crate::integration::lifecycle::shutdown(&app).await;
    Ok(())
}
