//! Chummy app shell.
//!
//! This crate stays thin: it owns app state, exposes IPC commands and manages
//! windows. All real logic lives in the `chummy-*` crates, which have no Tauri
//! dependency and are tested on their own.

mod commands;
mod events;
mod state;
mod windows;

use state::AppState;

pub fn run() {
    tauri::Builder::default()
        .manage(AppState::default())
        .invoke_handler(tauri::generate_handler![
            commands::setup_status,
            commands::open_window,
            commands::get_settings,
            commands::set_theme,
        ])
        .run(tauri::generate_context!())
        .expect("error while running Chummy");
}
