//! Binary-level proof that `main.rs` wires `gui_args::GuiArgs`
//! before any GUI code: `--version` exits 0 with the version on
//! stdout (safe headless — the webview never initializes), while an
//! unknown flag exits 2.
//!
//! These tests execute the real binary, so every spawn goes through
//! a bounded wait: if a future change ever lets the GUI event loop
//! start here, the test fails fast instead of hanging the runner.

use std::io::Read;
use std::process::{Command, Output, Stdio};
use std::time::{Duration, Instant};

/// Run to completion with a hard timeout (std-only, no extra deps).
/// Kills the child and panics on timeout so a GUI that fails to exit
/// early shows up red, not wedged.
fn run_with_timeout(cmd: &mut Command, timeout: Duration) -> Output {
    let mut child = cmd
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("spawn tuxtunes");
    let start = Instant::now();
    loop {
        match child.try_wait().expect("poll tuxtunes exit") {
            Some(status) => {
                let mut stdout = Vec::new();
                let mut stderr = Vec::new();
                if let Some(mut out) = child.stdout.take() {
                    out.read_to_end(&mut stdout).expect("read stdout");
                }
                if let Some(mut err) = child.stderr.take() {
                    err.read_to_end(&mut stderr).expect("read stderr");
                }
                return Output {
                    status,
                    stdout,
                    stderr,
                };
            }
            None if start.elapsed() >= timeout => {
                child.kill().expect("kill hung tuxtunes");
                let _ = child.wait();
                panic!("tuxtunes did not exit within {timeout:?} — GUI event loop started?");
            }
            None => std::thread::sleep(Duration::from_millis(50)),
        }
    }
}

#[test]
fn gui_binary_version_flag_exits_zero_without_gui() {
    let out = run_with_timeout(
        Command::new(env!("CARGO_BIN_EXE_tuxtunes")).arg("--version"),
        Duration::from_secs(15),
    );
    assert!(out.status.success());
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(
        stdout.contains(env!("CARGO_PKG_VERSION")),
        "unexpected --version output: {stdout}"
    );
}

#[test]
fn gui_binary_unknown_flag_exits_two() {
    let out = run_with_timeout(
        Command::new(env!("CARGO_BIN_EXE_tuxtunes")).arg("--bogus-flag"),
        Duration::from_secs(15),
    );
    assert_eq!(out.status.code(), Some(2));
}
