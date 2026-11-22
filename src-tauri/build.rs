fn main() {
    // Declaring the app's commands here makes Tauri generate one permission per
    // command (`allow-<command>`). Each window's capability file then lists only
    // the commands that window may call.
    tauri_build::try_build(tauri_build::Attributes::new().app_manifest(
        tauri_build::AppManifest::new().commands(&[
            "setup_status",
            "open_window",
            "get_settings",
            "set_theme",
            "app_info",
            "reveal_data_dir",
            "choose_workspace",
            "clear_workspace",
            "set_permission",
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
        ]),
    ))
    .expect("failed to run tauri-build");
}
