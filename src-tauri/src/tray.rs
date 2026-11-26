//! The tray icon, and what closing and quitting do.
//!
//! The tray icon is always there while Chummy runs, so it's always clear
//! that Chummy is running. Closing the main window asks by default: quit
//! (stopping every run), or keep running in the tray so scheduled jobs go
//! on. Nothing hides in the tray without the user choosing it.

use std::sync::atomic::Ordering;

use serde::Serialize;
use tauri::menu::{CheckMenuItem, Menu, MenuItem, PredefinedMenuItem};
use tauri::tray::{MouseButton, MouseButtonState, TrayIconBuilder, TrayIconEvent};
use tauri::{AppHandle, Emitter, Manager, State, Wry};

use crate::commands::{CmdResult, jobs};
use crate::events;
use crate::settings::CloseAction;
use crate::state::AppState;
use crate::windows::{self, AppWindow};

const TRAY_ID: &str = "chummy";

struct TrayItems {
    status: MenuItem<Wry>,
    pause: CheckMenuItem<Wry>,
}

/// Creates the tray icon. Fails on desktops without a tray; Chummy then
/// never offers to keep running in the tray.
pub fn create(app: &AppHandle) -> tauri::Result<()> {
    let open = MenuItem::with_id(app, "open", "Open Chummy", true, None::<&str>)?;
    let automation = MenuItem::with_id(app, "automation", "Automation", true, None::<&str>)?;
    let status = MenuItem::with_id(app, "status", "Nothing running", false, None::<&str>)?;
    let pause = CheckMenuItem::with_id(
        app,
        "pause",
        "Pause scheduled jobs",
        true,
        false,
        None::<&str>,
    )?;
    let quit = MenuItem::with_id(app, "quit", "Quit Chummy", true, None::<&str>)?;
    let menu = Menu::with_items(
        app,
        &[
            &open,
            &automation,
            &PredefinedMenuItem::separator(app)?,
            &status,
            &pause,
            &PredefinedMenuItem::separator(app)?,
            &quit,
        ],
    )?;
    let mut builder = TrayIconBuilder::with_id(TRAY_ID)
        .tooltip("Chummy")
        .menu(&menu)
        .show_menu_on_left_click(false)
        .on_menu_event(|app, e| match e.id().as_ref() {
            "open" => windows::show_main(app),
            "automation" => {
                let _ = windows::open(app, AppWindow::Automation, None);
            }
            "pause" => {
                let state = app.state::<AppState>();
                if let Some(m) = state.jobs.get() {
                    m.set_paused(!m.paused());
                }
                refresh(app);
            }
            "quit" => request_quit(app),
            _ => {}
        })
        .on_tray_icon_event(|tray, e| {
            if let TrayIconEvent::Click {
                button: MouseButton::Left,
                button_state: MouseButtonState::Up,
                ..
            } = e
            {
                windows::show_main(tray.app_handle());
            }
        });
    if let Some(icon) = app.default_window_icon() {
        builder = builder.icon(icon.clone());
    }
    builder.build(app)?;
    app.manage(TrayItems { status, pause });
    app.state::<AppState>()
        .tray_ready
        .store(true, Ordering::Relaxed);
    refresh(app);
    Ok(())
}

/// What's going on, for the tray and the quit question.
#[derive(Debug, Clone, Serialize)]
pub struct Activity {
    /// Runs going now (chats, tests and jobs).
    pub runs: usize,
    /// Questions waiting for an answer.
    pub waiting: usize,
    /// Jobs switched on with a schedule.
    pub scheduled: usize,
    pub paused: bool,
    /// The tray icon exists, so Chummy can keep running there.
    pub tray: bool,
    /// "close" (the window's close button) or "quit".
    pub reason: &'static str,
}

pub fn activity(app: &AppHandle, reason: &'static str) -> Activity {
    let state = app.state::<AppState>();
    let (_, scheduled, paused) = jobs::summary(&state);
    Activity {
        runs: state.session.run_count(),
        waiting: state.session.asks.pending().len(),
        scheduled,
        paused,
        tray: state.tray_ready.load(Ordering::Relaxed),
        reason,
    }
}

fn plural(n: usize, one: &str, many: &str) -> String {
    format!("{n} {}", if n == 1 { one } else { many })
}

/// Updates the tray's status line, tooltip and pause check.
pub fn refresh(app: &AppHandle) {
    let Some(items) = app.try_state::<TrayItems>() else {
        return;
    };
    let a = activity(app, "close");
    let mut parts = Vec::new();
    if a.runs > 0 {
        parts.push(format!("{} running", a.runs));
    }
    if a.waiting > 0 {
        parts.push(plural(a.waiting, "question waiting", "questions waiting"));
    }
    let line = if parts.is_empty() {
        if a.paused {
            "Scheduled jobs paused".to_string()
        } else if a.scheduled > 0 {
            plural(a.scheduled, "job scheduled", "jobs scheduled")
        } else {
            "Nothing running".to_string()
        }
    } else {
        parts.join(" · ")
    };
    let _ = items.status.set_text(&line);
    let _ = items.pause.set_checked(a.paused);
    if let Some(tray) = app.tray_by_id(TRAY_ID) {
        let _ = tray.set_tooltip(Some(format!("Chummy — {line}")));
    }
}

/// The main window's close button. Returns true to keep the window open.
pub fn close_requested(app: &AppHandle) -> bool {
    let state = app.state::<AppState>();
    if state.quitting.load(Ordering::Relaxed) {
        return false;
    }
    let a = activity(app, "close");
    match state.settings().general.close {
        CloseAction::Tray if a.tray => hide_to_tray(app),
        CloseAction::Quit if a.runs == 0 && a.waiting == 0 => quit(app),
        _ => ask(app, a),
    }
    true
}

/// Quitting from the tray or the app menu: asks if anything would stop.
pub fn request_quit(app: &AppHandle) {
    let a = activity(app, "quit");
    if a.runs == 0 && a.waiting == 0 {
        quit(app);
    } else {
        ask(app, a);
    }
}

fn ask(app: &AppHandle, a: Activity) {
    windows::show_main(app);
    let _ = app.emit_to("main", events::CLOSE_REQUESTED, a);
}

pub fn hide_to_tray(app: &AppHandle) {
    if let Some(w) = app.get_webview_window("main") {
        let _ = w.hide();
    }
}

/// Stops every run and exits.
pub fn quit(app: &AppHandle) {
    let state = app.state::<AppState>();
    state.quitting.store(true, Ordering::Relaxed);
    if let Some(m) = state.jobs.get() {
        m.stop_all();
    }
    state.session.stop_all();
    app.exit(0);
}

#[tauri::command]
pub fn close_activity(app: AppHandle) -> Activity {
    activity(&app, "close")
}

/// The answer to the quit question. `remember` saves the choice for next
/// time (Settings → General can change it back).
#[tauri::command]
pub fn close_choice(
    app: AppHandle,
    state: State<'_, AppState>,
    choice: CloseAction,
    remember: bool,
) -> CmdResult<()> {
    if remember && choice != CloseAction::Ask {
        state.update(&app, |s| s.general.close = choice)?;
    }
    match choice {
        CloseAction::Quit => quit(&app),
        CloseAction::Tray if state.tray_ready.load(Ordering::Relaxed) => hide_to_tray(&app),
        CloseAction::Tray => {
            if let Some(w) = app.get_webview_window("main") {
                let _ = w.minimize();
            }
        }
        CloseAction::Ask => {}
    }
    Ok(())
}
