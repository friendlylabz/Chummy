//! Chummy app shell.
//!
//! This crate stays thin: it owns app state, exposes IPC commands and manages
//! windows. All real logic lives in the `chummy-*` crates, which have no Tauri
//! dependency and are tested on their own.

mod commands;
mod dialogs;
mod events;
mod host;
mod library;
mod notify;
mod settings;
mod state;
mod tray;
mod windows;

use tauri::{Manager, RunEvent, WindowEvent};

use commands::{agents, asks, chat, general, jobs, local, loops, providers, tools};
use state::AppState;

pub fn run() {
    let app = tauri::Builder::default()
        // First, so a second launch only focuses this one: two copies would
        // run the same scheduled jobs and write the same files.
        .plugin(tauri_plugin_single_instance::init(|app, _args, _cwd| {
            windows::show_main(app);
        }))
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_opener::init())
        .plugin(tauri_plugin_notification::init())
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
            let state = match AppState::open(&data_dir, &extra) {
                Ok(s) => s,
                Err(e) if e == state::ALREADY_RUNNING => {
                    eprintln!("chummy: {e}");
                    std::process::exit(0);
                }
                Err(e) => return Err(e.into()),
            };
            app.manage(state);
            let handle = app.handle().clone();
            if let Err(e) = tray::create(&handle) {
                eprintln!("chummy: no tray icon ({e}); closing the window will quit");
            }
            jobs::start(&handle)?;
            asks::start_forwarding(&handle);
            Ok(())
        })
        .on_window_event(|window, event| {
            if window.label() == "main"
                && let WindowEvent::CloseRequested { api, .. } = event
                && tray::close_requested(window.app_handle())
            {
                api.prevent_close();
            }
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
            tools::tools_status,
            tools::setup_tools,
            tools::set_tool_enabled,
            tools::set_tool_network,
            tools::set_tool_ask,
            general::set_general,
            jobs::list_jobs,
            jobs::save_job,
            jobs::delete_job,
            jobs::set_job_enabled,
            jobs::run_job_now,
            jobs::stop_job,
            jobs::job_history,
            jobs::set_jobs_paused,
            jobs::import_job,
            jobs::export_job,
            asks::list_asks,
            asks::answer_ask,
            tray::close_activity,
            tray::close_choice,
        ])
        .build(tauri::generate_context!())
        .expect("error while building Chummy");

    app.run(|app, event| match event {
        // Quitting some other way (the app menu, Cmd+Q, the last window
        // closing) goes through the same question as the close button.
        RunEvent::ExitRequested { api, .. } => {
            if let Some(state) = app.try_state::<AppState>()
                && !state.quitting.load(std::sync::atomic::Ordering::Relaxed)
            {
                api.prevent_exit();
                tray::request_quit(app);
            }
        }
        #[cfg(target_os = "macos")]
        RunEvent::Reopen { .. } => windows::show_main(app),
        RunEvent::Exit => {
            if let Some(state) = app.try_state::<AppState>() {
                // Never leave a model loaded in the background.
                tauri::async_runtime::block_on(state.engine.stop());
            }
        }
        _ => {}
    });
}
