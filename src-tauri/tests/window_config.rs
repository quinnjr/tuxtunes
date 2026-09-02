//! Tauri merges `tauri.<platform>.conf.json` over `tauri.conf.json`
//! with JSON Merge Patch (RFC 7396), which replaces arrays wholesale.
//! The macOS override therefore restates the whole main-window object,
//! and any key later added to the base window would silently not apply
//! on macOS. This test pins the two in step: every base key must be
//! present with the same value in the override, except the ones the
//! override exists to change.

use serde_json::Value;

const MACOS_ONLY: &[&str] = &["decorations", "titleBarStyle", "hiddenTitle"];
/// Keys the base sets that macOS deliberately omits (no-ops there).
const BASE_ONLY: &[&str] = &["shadow"];

fn main_window(path: &str) -> serde_json::Map<String, Value> {
    let text = std::fs::read_to_string(concat!(env!("CARGO_MANIFEST_DIR"), "/").to_owned() + path)
        .unwrap_or_else(|e| panic!("read {path}: {e}"));
    let conf: Value = serde_json::from_str(&text).unwrap_or_else(|e| panic!("parse {path}: {e}"));
    conf["app"]["windows"][0]
        .as_object()
        .unwrap_or_else(|| panic!("{path}: app.windows[0] is not an object"))
        .clone()
}

#[test]
fn macos_window_override_mirrors_base_window() {
    let base = main_window("tauri.conf.json");
    let mac = main_window("tauri.macos.conf.json");

    for (key, value) in &base {
        if BASE_ONLY.contains(&key.as_str()) || MACOS_ONLY.contains(&key.as_str()) {
            continue;
        }
        assert_eq!(
            mac.get(key),
            Some(value),
            "tauri.macos.conf.json main window must restate `{key}` = {value}"
        );
    }
    for key in MACOS_ONLY {
        assert!(mac.contains_key(*key), "macOS override must set `{key}`");
    }
    assert_eq!(base.get("decorations"), Some(&Value::Bool(false)));
    assert_eq!(mac.get("decorations"), Some(&Value::Bool(true)));
    assert_eq!(mac.get("titleBarStyle"), Some(&Value::String("Overlay".into())));
}
