//! The Logs window: the system log and every run's log.

use std::collections::HashSet;

use chummy_log::runs::{self, Entry, RunQuery, RunSummary};
use chummy_log::system::{self as syslog, SysEntry};
use serde::Serialize;
use tauri::{AppHandle, Emitter, State};

use super::CmdResult;
use crate::events;
use crate::state::{AppState, lock};

/// Starts the system log and sends new entries to every window.
pub fn start(app: &AppHandle, state: &AppState) {
    let log = syslog::init(state.dirs.logs.join("system"));
    let app = app.clone();
    log.set_listener(move |e| {
        let _ = app.emit(events::SYSTEM_LOG, e);
    });
    let root = state.run_logs.clone();
    std::thread::spawn(move || runs::cleanup(&root));
    syslog::log(
        syslog::Level::Info,
        "app",
        "Chummy started",
        &[
            ("version", env!("CARGO_PKG_VERSION")),
            ("os", std::env::consts::OS),
            ("arch", std::env::consts::ARCH),
        ],
    );
}

fn running(state: &AppState) -> HashSet<String> {
    lock(&state.runs).keys().cloned().collect()
}

#[derive(Serialize)]
pub struct RunListItem {
    #[serde(flatten)]
    pub summary: RunSummary,
    /// "completed", "stopped", "failed", "limit", "interrupted" or "running".
    pub status: String,
}

/// Runs whose logs match `query`, newest first.
#[tauri::command]
pub async fn search_runs(
    state: State<'_, AppState>,
    query: RunQuery,
) -> CmdResult<Vec<RunListItem>> {
    let root = state.run_logs.clone();
    let live = running(&state);
    tauri::async_runtime::spawn_blocking(move || {
        runs::search(&root, &query, &live)
            .into_iter()
            .map(|s| RunListItem {
                status: runs::status_of(&s, &live),
                summary: s,
            })
            .collect()
    })
    .await
    .map_err(|e| e.to_string())
}

/// Every entry of one run's log.
#[tauri::command]
pub async fn read_run_log(state: State<'_, AppState>, run_id: String) -> CmdResult<Vec<Entry>> {
    let root = state.run_logs.clone();
    tauri::async_runtime::spawn_blocking(move || runs::read(&root, &run_id))
        .await
        .map_err(|e| e.to_string())?
        .ok_or_else(|| "That run's log isn't there any more (logs are kept for 30 days).".into())
}

/// Days with a system log, newest first.
#[tauri::command]
pub fn system_log_days() -> Vec<String> {
    syslog::global().map(|l| l.days()).unwrap_or_default()
}

/// One day's system log, oldest first.
#[tauri::command]
pub async fn read_system_log(day: String) -> CmdResult<Vec<SysEntry>> {
    tauri::async_runtime::spawn_blocking(move || {
        syslog::global()
            .map(|l| l.read_day(&day))
            .unwrap_or_default()
    })
    .await
    .map_err(|e| e.to_string())
}

/// Opens the logs folder in the file manager.
#[tauri::command]
pub fn reveal_logs(app: AppHandle, state: State<'_, AppState>) -> CmdResult<()> {
    use tauri_plugin_opener::OpenerExt;
    app.opener()
        .open_path(state.dirs.logs.to_string_lossy(), None::<&str>)
        .map_err(|e| e.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn list_items_flatten_the_summary() {
        let item = RunListItem {
            summary: RunSummary {
                header: Default::default(),
                end: None,
                bytes: 3,
                day: "2026-01-01".into(),
            },
            status: "running".into(),
        };
        let v = serde_json::to_value(item).unwrap();
        assert_eq!(v["status"], "running");
        assert_eq!(v["bytes"], 3);
        assert!(v["header"].is_object());
    }
}
