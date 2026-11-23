fn main() {
    // Windows needs a manifest asking for Common Controls v6 (dialogs, the
    // tray menu). tauri-build would only put it in the app's binary, so test
    // binaries would fail to start (STATUS_ENTRYPOINT_NOT_FOUND). Instead the
    // linker embeds the same manifest into every binary, tests included.
    let windows_msvc = std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("windows")
        && std::env::var("CARGO_CFG_TARGET_ENV").as_deref() == Ok("msvc");
    let mut attributes = tauri_build::Attributes::new();
    if windows_msvc {
        let manifest = std::path::Path::new(&std::env::var("CARGO_MANIFEST_DIR").unwrap())
            .join("windows-app-manifest.xml");
        println!("cargo:rerun-if-changed={}", manifest.display());
        println!("cargo:rustc-link-arg=/MANIFEST:EMBED");
        println!("cargo:rustc-link-arg=/MANIFESTINPUT:{}", manifest.display());
        attributes = attributes
            .windows_attributes(tauri_build::WindowsAttributes::new_without_app_manifest());
    }
    // Declaring the app's commands here makes Tauri generate one permission per
    // command (`allow-<command>`). Each window's capability file then lists only
    // the commands that window may call.
    tauri_build::try_build(
        attributes.app_manifest(tauri_build::AppManifest::new().commands(&[
            "setup_status",
            "open_window",
            "get_settings",
            "open_link",
            "set_theme",
            "app_info",
            "reveal_data_dir",
            "pick_workspace",
            "save_agent",
            "delete_agent",
            "set_active_agent",
            "set_permission",
            "block_catalog",
            "loop_templates",
            "list_loops",
            "load_loop",
            "analyze_loop",
            "save_loop",
            "add_recommended_loop",
            "delete_loop",
            "import_loop",
            "export_loop",
            "get_chat",
            "list_chats",
            "new_chat",
            "set_active_chat",
            "delete_chat",
            "delete_chat_message",
            "restore_chat_message",
            "send_message",
            "test_run",
            "stop_run",
            "list_runs",
            "list_presets",
            "save_provider",
            "delete_provider",
            "set_default_provider",
            "list_models",
            "test_provider",
            "local_status",
            "inspect_model",
            "download_model",
            "cancel_download",
            "delete_local_model",
            "install_runtime",
            "cancel_runtime_install",
            "remove_runtime",
            "set_hf_token",
            "set_local_device",
            "stop_engine",
            "engine_log",
            "tools_status",
            "setup_tools",
            "set_tool_enabled",
            "set_tool_network",
            "set_tool_ask",
            "sandbox_status",
            "check_sandbox",
            "set_sandbox_kind",
            "set_container_options",
            "get_container_image",
            "set_openshell_image",
            "choose_sandbox_program",
            "clear_sandbox_program",
            "remove_openshell_sandboxes",
            "set_agent_access",
            "agent_loop_config",
            "set_agent_block_config",
            "export_agent_loop_config",
            "import_agent_loop_config",
            "usage_today",
            "set_general",
            "list_jobs",
            "save_job",
            "delete_job",
            "set_job_enabled",
            "run_job_now",
            "stop_job",
            "job_history",
            "set_jobs_paused",
            "import_job",
            "export_job",
            "list_asks",
            "answer_ask",
            "search_runs",
            "read_run_log",
            "system_log_days",
            "read_system_log",
            "reveal_logs",
            "set_logging",
            "log_key_status",
            "export_log_key",
            "import_log_key",
            "close_activity",
            "close_choice",
        ])),
    )
    .expect("failed to run tauri-build");
}
