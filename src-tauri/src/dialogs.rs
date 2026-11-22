//! Native dialogs shown by Rust. Confirmations that widen what agents can do
//! are always shown from here, never from the web UI, so no script in a window
//! can grant access by itself.

use std::path::PathBuf;

use tauri::{AppHandle, Manager, WebviewWindow};
use tauri_plugin_dialog::{DialogExt, MessageDialogButtons, MessageDialogKind};

fn parent(app: &AppHandle, label: &str) -> Option<WebviewWindow> {
    app.get_webview_window(label)
}

/// Asks a yes/no question. Runs on a blocking thread.
pub async fn confirm(
    app: &AppHandle,
    window: &str,
    title: &str,
    message: &str,
    ok: &str,
    warning: bool,
) -> bool {
    let app = app.clone();
    let (window, title, message, ok) = (
        window.to_string(),
        title.to_string(),
        message.to_string(),
        ok.to_string(),
    );
    tauri::async_runtime::spawn_blocking(move || {
        let mut d = app
            .dialog()
            .message(message)
            .title(title)
            .kind(if warning {
                MessageDialogKind::Warning
            } else {
                MessageDialogKind::Info
            })
            .buttons(MessageDialogButtons::OkCancelCustom(ok, "Cancel".into()));
        if let Some(w) = parent(&app, &window) {
            d = d.parent(&w);
        }
        d.blocking_show()
    })
    .await
    .unwrap_or(false)
}

/// Shows an error message.
pub async fn error(app: &AppHandle, window: &str, title: &str, message: &str) {
    let app = app.clone();
    let (window, title, message) = (window.to_string(), title.to_string(), message.to_string());
    let _ = tauri::async_runtime::spawn_blocking(move || {
        let mut d = app
            .dialog()
            .message(message)
            .title(title)
            .kind(MessageDialogKind::Error);
        if let Some(w) = parent(&app, &window) {
            d = d.parent(&w);
        }
        d.blocking_show();
    })
    .await;
}

/// Lets the user pick a folder.
pub async fn pick_folder(app: &AppHandle, window: &str, title: &str) -> Option<PathBuf> {
    let app = app.clone();
    let (window, title) = (window.to_string(), title.to_string());
    tauri::async_runtime::spawn_blocking(move || {
        let mut d = app.dialog().file().set_title(title);
        if let Some(w) = parent(&app, &window) {
            d = d.set_parent(&w);
        }
        d.blocking_pick_folder().and_then(|p| p.into_path().ok())
    })
    .await
    .ok()
    .flatten()
}

/// Lets the user pick a loop file to import.
pub async fn pick_loop_file(app: &AppHandle, window: &str) -> Option<PathBuf> {
    let app = app.clone();
    let window = window.to_string();
    tauri::async_runtime::spawn_blocking(move || {
        let mut d = app
            .dialog()
            .file()
            .set_title("Import a loop")
            .add_filter("Chummy loop", &["json"]);
        if let Some(w) = parent(&app, &window) {
            d = d.set_parent(&w);
        }
        d.blocking_pick_file().and_then(|p| p.into_path().ok())
    })
    .await
    .ok()
    .flatten()
}

/// Asks where to save an exported loop.
pub async fn save_loop_file(app: &AppHandle, window: &str, suggested: &str) -> Option<PathBuf> {
    let app = app.clone();
    let (window, suggested) = (window.to_string(), suggested.to_string());
    tauri::async_runtime::spawn_blocking(move || {
        let mut d = app
            .dialog()
            .file()
            .set_title("Export loop")
            .set_file_name(suggested)
            .add_filter("Chummy loop", &["json"]);
        if let Some(w) = parent(&app, &window) {
            d = d.set_parent(&w);
        }
        d.blocking_save_file().and_then(|p| p.into_path().ok())
    })
    .await
    .ok()
    .flatten()
}
