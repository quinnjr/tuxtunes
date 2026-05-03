//! System tray with playback transport controls.
//!
//! Tauri 2's native tray API. Menu actions emit `tray:*` Tauri events
//! rather than dispatching directly to the playback engine — the
//! frontend's PlaybackService already knows the current state and
//! holds the queue, so funneling tray clicks through it keeps the
//! state machine in one place. Tray clicks and UI clicks then go
//! through the same code path.

use crate::db::tracks;
use std::sync::OnceLock;
use tauri::menu::{MenuBuilder, MenuItemBuilder};
use tauri::tray::{TrayIconBuilder, TrayIconEvent};
use tauri::{AppHandle, Emitter, Manager, Runtime};

type LabelUpdater = Box<dyn Fn(&str) + Send + Sync>;

/// Closure that updates the "Now Playing" menu item's label. The
/// closure captures the typed `MenuItem<R>` at install time, so this
/// static can stay runtime-erased even though Tauri's menu types are
/// runtime-generic. install() can run with any `R: Runtime` (Wry in
/// production, MockRuntime in tests).
static UPDATE_NOW_PLAYING: OnceLock<LabelUpdater> = OnceLock::new();

const ID_PLAY_PAUSE: &str = "tray:play-pause";
const ID_NEXT: &str = "tray:next";
const ID_PREV: &str = "tray:prev";
const ID_SHOW: &str = "tray:show";
const ID_QUIT: &str = "tray:quit";
const ID_NOW_PLAYING: &str = "tray:now-playing";

/// Event channel names emitted toward the frontend. The Angular
/// PlaybackService listens on these and dispatches via its own
/// state-aware code paths.
pub const EVT_TRAY_TOGGLE_PLAY: &str = "tray:toggle-play";
pub const EVT_TRAY_NEXT: &str = "tray:next";
pub const EVT_TRAY_PREV: &str = "tray:prev";

/// Build the tray, mount its menu, and wire click → event dispatch.
/// Failures are logged and the app continues — a tray that won't build
/// shouldn't take the rest of the app down with it.
pub fn install<R: Runtime>(app: &AppHandle<R>) {
    if let Err(e) = try_install(app) {
        log::warn!("tray install failed: {e}");
    }
}

fn try_install<R: Runtime>(app: &AppHandle<R>) -> tauri::Result<()> {
    let now_playing = MenuItemBuilder::with_id(ID_NOW_PLAYING, "Nothing playing")
        .enabled(false)
        .build(app)?;
    let play_pause = MenuItemBuilder::with_id(ID_PLAY_PAUSE, "Play / Pause").build(app)?;
    let next = MenuItemBuilder::with_id(ID_NEXT, "Next").build(app)?;
    let prev = MenuItemBuilder::with_id(ID_PREV, "Previous").build(app)?;
    let show = MenuItemBuilder::with_id(ID_SHOW, "Show TuxTunes").build(app)?;
    let quit = MenuItemBuilder::with_id(ID_QUIT, "Quit").build(app)?;

    let menu = MenuBuilder::new(app)
        .item(&now_playing)
        .separator()
        .item(&play_pause)
        .item(&next)
        .item(&prev)
        .separator()
        .item(&show)
        .item(&quit)
        .build()?;

    // Capture the now-playing handle in a runtime-erased closure
    // before the menu builder takes ownership. set_now_playing_label
    // routes through this OnceLock without needing to walk the menu
    // tree or thread `<R>` through every callsite.
    let item_for_closure = now_playing.clone();
    let _ = UPDATE_NOW_PLAYING.set(Box::new(move |label: &str| {
        let _ = item_for_closure.set_text(label);
    }));

    let _tray = TrayIconBuilder::with_id("tuxtunes-main")
        .tooltip("TuxTunes")
        .menu(&menu)
        .show_menu_on_left_click(false)
        .on_menu_event(|app, event| handle_menu(app, event.id().as_ref()))
        .on_tray_icon_event(|tray, event| handle_icon(tray.app_handle(), &event))
        .build(app)?;

    Ok(())
}

fn handle_menu<R: Runtime>(app: &AppHandle<R>, id: &str) {
    match id {
        ID_PLAY_PAUSE => {
            let _ = app.emit(EVT_TRAY_TOGGLE_PLAY, ());
        }
        ID_NEXT => {
            let _ = app.emit(EVT_TRAY_NEXT, ());
        }
        ID_PREV => {
            let _ = app.emit(EVT_TRAY_PREV, ());
        }
        ID_SHOW => toggle_main_window(app),
        ID_QUIT => app.exit(0),
        _ => {}
    }
}

fn handle_icon<R: Runtime>(app: &AppHandle<R>, event: &TrayIconEvent) {
    // Single left-click toggles main window visibility (matches the
    // MPRIS Raise contract and common Linux media-player behavior).
    if let TrayIconEvent::Click {
        button: tauri::tray::MouseButton::Left,
        button_state: tauri::tray::MouseButtonState::Up,
        ..
    } = event
    {
        toggle_main_window(app);
    }
}

fn toggle_main_window<R: Runtime>(app: &AppHandle<R>) {
    let Some(window) = app.get_webview_window("main") else {
        return;
    };
    let visible = window.is_visible().unwrap_or(false);
    let res = if visible {
        window.hide()
    } else {
        window.show().and_then(|()| window.set_focus())
    };
    if let Err(e) = res {
        log::warn!("toggle main window: {e}");
    }
}

/// Update the "Now Playing" menu line. Called from a tokio task that
/// consumes track-changed events. The OnceLock-stored closure routes
/// the update without requiring callers to know the runtime type.
pub fn set_now_playing_label<R: Runtime>(_app: &AppHandle<R>, label: Option<&str>) {
    let Some(update) = UPDATE_NOW_PLAYING.get() else {
        return;
    };
    let text = label
        .map(|t| format!("Now Playing: {t}"))
        .unwrap_or_else(|| "Nothing playing".to_string());
    update(&text);
}

/// Render a track row's "title — artist" string for tray label and
/// notification body. Centralized so both surfaces stay in sync.
pub fn track_label(row: &tracks::TrackRow) -> String {
    match &row.artist {
        Some(a) if !a.is_empty() => format!("{} — {}", row.title, a),
        _ => row.title.clone(),
    }
}

/// Test hook exposing handle_menu so integration tests can drive
/// each of the menu-event branches without standing up a real tray.
/// Hidden from rustdoc — not part of the public API.
#[doc(hidden)]
pub fn dispatch_menu_for_test<R: Runtime>(app: &AppHandle<R>, id: &str) {
    handle_menu(app, id);
}

/// Test hook exposing handle_icon's branches so tests can exercise
/// the MouseButton::Left filter without a real tray icon emitting
/// events. Hidden from rustdoc — not part of the public API.
#[doc(hidden)]
pub fn dispatch_icon_for_test<R: Runtime>(app: &AppHandle<R>, event: &TrayIconEvent) {
    handle_icon(app, event);
}
