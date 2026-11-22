//! Chummy app shell.
//!
//! This crate stays thin: it owns app state, exposes IPC commands and manages
//! windows. All real logic lives in the `chummy-*` crates, which have no Tauri
//! dependency and are tested on their own.

mod commands;
mod dialogs;
mod events;
mod library;
mod settings;
mod state;
mod windows;

use tauri::{Manager, RunEvent};

use commands::{agents, chat, general, local, loops, providers};
use state::AppState;

pub fn run() {
    let app = tauri::Builder::default()
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_opener::init())
        .setup(|app| {
            // The Chummy folder: one per-user folder for settings, loops,
            // models and the local engine runtime.
            let data_dir = app.path().app_local_data_dir()?;
            // The app's own install location is protected too.
            let exe_dir = std::env::current_exe()
                .ok()
                .and_then(|p| p.parent().map(|p| p.to_path_buf()));
            let resource_dir = app.path().resource_dir().ok();
            let extra: Vec<&std::path::Path> = exe_dir
                .iter()
                .chain(resource_dir.iter())
                .map(|p| p.as_path())
                .collect();
            let state = AppState::open(&data_dir, &extra)?;
            app.manage(state);
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            general::setup_status,
            general::open_window,
            general::get_settings,
            general::set_theme,
            general::app_info,
            general::reveal_data_dir,
            agents::pick_workspace,
            agents::save_agent,
            agents::delete_agent,
            agents::set_active_agent,
            agents::set_permission,
            loops::block_catalog,
            loops::loop_templates,
            loops::list_loops,
            loops::load_loop,
            loops::analyze_loop,
            loops::save_loop,
            loops::delete_loop,
            loops::import_loop,
            loops::export_loop,
            chat::get_chat,
            chat::clear_chat,
            chat::send_message,
            chat::test_run,
            chat::stop_run,
            chat::list_runs,
            providers::list_presets,
            providers::save_provider,
            providers::delete_provider,
            providers::set_default_provider,
            providers::list_models,
            providers::test_provider,
            local::local_status,
            local::inspect_model,
            local::download_model,
            local::cancel_download,
            local::delete_local_model,
            local::install_runtime,
            local::cancel_runtime_install,
            local::remove_runtime,
            local::set_hf_token,
            local::set_local_device,
            local::stop_engine,
            local::engine_log,
        ])
        .build(tauri::generate_context!())
        .expect("error while building Chummy");

    app.run(|app, event| {
        if let RunEvent::Exit = event
            && let Some(state) = app.try_state::<AppState>()
        {
            // Never leave a model loaded in the background.
            tauri::async_runtime::block_on(state.engine.stop());
        }
    });
}
