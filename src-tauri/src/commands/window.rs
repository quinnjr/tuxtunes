//! Window-chrome support commands.
//!
//! The frontend decides whether to draw its own traffic lights and
//! hairline border per platform. The webview user agent is a fair
//! first guess, but it is configurable (`app.windows[].userAgent`) and
//! subject to UA reduction, so the compiled-in OS is the authority.

/// The OS this binary was built for, as a stable lowercase token
/// (`linux`, `macos`, `windows`, …).
#[tauri::command]
pub fn host_os() -> &'static str {
    std::env::consts::OS
}
