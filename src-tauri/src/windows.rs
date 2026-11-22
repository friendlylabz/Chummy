//! Secondary windows. The main window is declared in `tauri.conf.json`; the
//! editor, settings and scheduler windows are created on demand. Each is a
//! singleton: opening one that already exists focuses it.

use serde::Deserialize;
use tauri::{AppHandle, Emitter, Manager, WebviewUrl, WebviewWindowBuilder};

#[derive(Debug, Clone, Copy, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AppWindow {
    Editor,
    Settings,
    Scheduler,
}

impl AppWindow {
    /// Window label. Must match the `windows` list in that window's capability file.
    pub fn label(self) -> &'static str {
        match self {
            AppWindow::Editor => "editor",
            AppWindow::Settings => "settings",
            AppWindow::Scheduler => "scheduler",
        }
    }

    fn page(self) -> &'static str {
        match self {
            AppWindow::Editor => "editor.html",
            AppWindow::Settings => "settings.html",
            AppWindow::Scheduler => "scheduler.html",
        }
    }

    fn title(self) -> &'static str {
        match self {
            AppWindow::Editor => "Chummy — Loop Editor",
            AppWindow::Settings => "Chummy — Settings",
            AppWindow::Scheduler => "Chummy — Scheduler",
        }
    }

    /// (width, height, min_width, min_height)
    fn size(self) -> (f64, f64, f64, f64) {
        match self {
            AppWindow::Editor => (1280.0, 800.0, 800.0, 520.0),
            AppWindow::Settings => (860.0, 600.0, 640.0, 440.0),
            AppWindow::Scheduler => (1000.0, 680.0, 720.0, 480.0),
        }
    }
}

/// Event sent to an already-open window to show a section (Settings pane,
/// editor loop). Payload: the section string.
pub const GOTO: &str = "window:goto";

/// Opens `window`, or focuses it if it is already open. `section` picks what
/// to show (e.g. `agents` in Settings, a loop id in the editor); new windows
/// get it as the URL fragment.
pub fn open(app: &AppHandle, window: AppWindow, section: Option<&str>) -> tauri::Result<()> {
    let section = section.filter(|s| {
        s.chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_' || c == ':')
    });
    if let Some(existing) = app.get_webview_window(window.label()) {
        existing.unminimize()?;
        existing.show()?;
        existing.set_focus()?;
        if let Some(s) = section {
            existing.emit_to(window.label(), GOTO, s)?;
        }
        return Ok(());
    }

    let (w, h, min_w, min_h) = window.size();
    let url = match section {
        Some(s) => format!("{}#{s}", window.page()),
        None => window.page().to_string(),
    };
    WebviewWindowBuilder::new(app, window.label(), WebviewUrl::App(url.into()))
        .title(window.title())
        .inner_size(w, h)
        .min_inner_size(min_w, min_h)
        .center()
        .build()?;
    Ok(())
}
