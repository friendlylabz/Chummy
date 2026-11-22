//! Desktop notifications, for questions that come while no Chummy window is
//! in front and for finished jobs. Shown from Rust only; windows can't send
//! notifications themselves.

use tauri::{AppHandle, Manager};
use tauri_plugin_notification::NotificationExt;

use crate::state::AppState;

/// Whether one of Chummy's windows is visible and focused.
pub fn any_window_focused(app: &AppHandle) -> bool {
    app.webview_windows()
        .values()
        .any(|w| w.is_visible().unwrap_or(false) && w.is_focused().unwrap_or(false))
}

/// Shows a notification, unless they're switched off in Settings.
pub fn show(app: &AppHandle, title: &str, body: &str) {
    let state = app.state::<AppState>();
    if !state.settings().general.notifications {
        return;
    }
    let _ = app.notification().builder().title(title).body(body).show();
}
