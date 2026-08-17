#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

fn main() {
    // WebKitGTK's DMA-BUF renderer crashes the whole app with a Wayland
    // "Error 71 (Protocol error)" on NVIDIA proprietary drivers. Disable
    // it before the webview initializes unless the user has already made
    // an explicit choice. Must run before any GTK/webview code and while
    // the process is still single-threaded.
    #[cfg(target_os = "linux")]
    if std::env::var_os("WEBKIT_DISABLE_DMABUF_RENDERER").is_none() {
        std::env::set_var("WEBKIT_DISABLE_DMABUF_RENDERER", "1");
    }

    tuxtunes::run();
}
