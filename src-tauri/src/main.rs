// Note: with `windows_subsystem` the release Windows process has no
// console, so `--help` / `--version` output has nowhere visible to go
// there. Linux, macOS, and dev builds are unaffected.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

use clap::Parser;

fn main() {
    // Contract documented on `gui_args::GuiArgs`: --help/--version exit
    // here via clap; positionals are accepted for `%U` and ignored.
    let _ = tuxtunes::gui_args::GuiArgs::parse();

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
