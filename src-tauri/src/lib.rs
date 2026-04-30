mod commands;
pub mod db;
pub mod library;
pub mod playback;
mod runtime;

use runtime::AppState;
use std::path::PathBuf;
use tauri::Manager;

fn data_dir(app: &tauri::App) -> PathBuf {
    app.path().app_data_dir().expect("app data dir resolves")
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    let runtime = tokio::runtime::Runtime::new().expect("tokio runtime");

    tauri::Builder::default()
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_shell::init())
        .invoke_handler(tauri::generate_handler![
            commands::library::get_library_stats,
        ])
        .setup(move |app| {
            let dir = data_dir(app);
            std::fs::create_dir_all(&dir).expect("create app data dir");
            let db_path = dir.join("tuxtunes.db");
            let state = runtime
                .block_on(AppState::new(&db_path))
                .expect("AppState init");
            app.manage(state);
            Ok(())
        })
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
