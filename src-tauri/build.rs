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
        ]),
    ))
    .expect("failed to run tauri-build");
}
