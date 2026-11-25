//! Chummy app shell.
//!
//! This crate stays thin: it owns app state, exposes IPC commands and manages
//! windows. All real logic lives in the `chummy-*` crates, which have no Tauri
//! dependency and are tested on their own.

mod bundled_python;
mod chats;
mod cli;
mod commands;
mod dialogs;
mod events;
mod host;
mod in_app;
mod library;
mod notify;
mod settings;
mod state;
mod tray;
mod trust;
mod windows;

use tauri::{Manager, RunEvent, WindowEvent};

use commands::{
    agent_setup, agents, asks, chat, general, jobs, local, logs, loops, providers, review, sandbox,
    tools,
};
use state::AppState;

pub fn run() {
    // `chummy --self-test`: checks that this build may compile WebAssembly
    // (on macOS the hardened runtime needs the JIT entitlements), then exits.
    if std::env::args().any(|a| a == "--self-test") {
        match chummy_tools::host::self_test() {
            Ok(()) => {
                println!("chummy self-test: ok");
                std::process::exit(0);
            }
            Err(e) => {
                eprintln!("chummy self-test failed: {e}");
                std::process::exit(1);
            }
        }
    }
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
            // models and the local engine runtime, unless `--data-dir`
            // names another one.
            let cwd = std::env::current_dir().unwrap_or_default();
            let data_dir = match cli::data_dir_arg(std::env::args_os(), &cwd)
                .and_then(|d| d.map(|d| cli::prepare_data_dir(&d)).transpose())
            {
                Ok(Some(d)) => d,
                Ok(None) => app.path().app_local_data_dir()?,
                Err(e) => {
                    eprintln!("chummy: {e}");
                    return Err(e.into());
                }
            };
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
            let handle = app.handle().clone();
            logs::start(&handle, &state);
            app.manage(state);
            commands::tools::refresh_builtins(&handle);
            // Container and OpenShell are checked at start-up; until a check
            // passes, no tools are offered.
            commands::sandbox::check_soon(&handle);
            if let Err(e) = tray::create(&handle) {
                chummy_log::system::warn(
                    "app",
                    format!("No tray icon ({e}); closing the window will quit"),
                );
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
            general::open_link,
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
            loops::add_recommended_loop,
            loops::delete_loop,
            loops::import_loop,
            loops::export_loop,
            review::get_review,
            review::accept_review,
            review::discard_review,
            review::review_library_loop,
            chat::get_chat,
            chat::list_chats,
            chat::new_chat,
            chat::set_active_chat,
            chat::delete_chat,
            chat::delete_chat_message,
            chat::restore_chat_message,
            chat::send_message,
            chat::test_run,
            chat::stop_run,
            chat::list_runs,
            chat::run_pane,
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
            sandbox::sandbox_status,
            sandbox::check_sandbox,
            sandbox::set_sandbox_kind,
            sandbox::set_container_options,
            sandbox::get_container_image,
            sandbox::set_openshell_image,
            sandbox::choose_sandbox_program,
            sandbox::clear_sandbox_program,
            sandbox::remove_openshell_sandboxes,
            agent_setup::set_agent_access,
            agent_setup::agent_loop_config,
            agent_setup::set_agent_block_config,
            agent_setup::set_agent_model_options,
            agent_setup::export_agent_loop_config,
            agent_setup::import_agent_loop_config,
            agent_setup::usage_today,
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
            logs::search_runs,
            logs::read_run_log,
            logs::system_log_days,
            logs::read_system_log,
            logs::reveal_logs,
            logs::set_logging,
            logs::log_key_status,
            logs::export_log_key,
            logs::import_log_key,
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
