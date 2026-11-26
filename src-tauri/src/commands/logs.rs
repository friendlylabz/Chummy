//! The Logs window: the system log and every run's log.

use std::collections::HashSet;

use chummy_log::crypt;
use chummy_log::runs::{self, Entry, RunQuery, RunSummary};
use chummy_log::system::{self as syslog, Level, SysEntry};
use serde::Serialize;
use tauri::{AppHandle, Emitter, State, WebviewWindow};

use super::CmdResult;
use crate::dialogs;
use crate::events;
use crate::settings::Logging;
use crate::state::AppState;

/// Largest log key file read.
const MAX_KEY_FILE: u64 = 64 * 1024;

/// Starts the system log and sends new entries to every window.
pub fn start(app: &AppHandle, state: &AppState) {
    let log = syslog::init(state.dirs.logs.join("system"));
    log.set_min_level(state.settings().logging.system.min());
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
    state.session.run_ids().into_iter().collect()
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
    let key = state.log_key(false).ok().flatten();
    tauri::async_runtime::spawn_blocking(move || {
        runs::search(&root, &query, &live, key.as_ref())
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
    let key = state.log_key(false).ok().flatten();
    tauri::async_runtime::spawn_blocking(move || runs::read(&root, &run_id, key.as_ref()))
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

/// Saves the logging settings. Turning encryption off asks first.
#[tauri::command]
pub async fn set_logging(
    app: AppHandle,
    window: WebviewWindow,
    state: State<'_, AppState>,
    logging: Logging,
) -> CmdResult<Logging> {
    let mut logging = logging;
    let old = state.settings().logging;
    if old.encrypt && !logging.encrypt && logging.runs.detail().is_some() {
        let ok = dialogs::confirm(
            &app,
            window.label(),
            "Write run logs without encryption?",
            "New run logs will be plain text in the Chummy folder. They can contain your messages, \
             the agent's answers, text from files it read, and anything secret that passed through \
             them, such as an API key a tool returned. Anything that can read your files could read \
             them.\n\nLogs already written stay encrypted.",
            "Don't encrypt",
            true,
        )
        .await;
        if !ok {
            logging.encrypt = true;
        }
    }
    state.update(&app, |s| s.logging = logging.clone())?;
    if let Some(l) = syslog::global() {
        l.set_min_level(logging.system.min());
    }
    Ok(logging)
}

#[derive(Serialize)]
pub struct LogKeyStatus {
    /// The key's fingerprint, if there is one.
    pub fingerprint: Option<String>,
    /// Why the keychain couldn't be used, if it couldn't.
    pub error: Option<String>,
}

#[tauri::command]
pub fn log_key_status(state: State<'_, AppState>) -> LogKeyStatus {
    match state.log_key(false) {
        Ok(k) => LogKeyStatus {
            fingerprint: k.map(|k| k.fingerprint()),
            error: None,
        },
        Err(e) => LogKeyStatus {
            fingerprint: None,
            error: Some(e),
        },
    }
}

/// Saves the log key to a file, sealed with a passphrase.
#[tauri::command]
pub async fn export_log_key(
    app: AppHandle,
    window: WebviewWindow,
    state: State<'_, AppState>,
    passphrase: String,
) -> CmdResult<bool> {
    if passphrase.chars().count() < crypt::MIN_PASSPHRASE {
        return Err(format!(
            "Use a passphrase of at least {} characters.",
            crypt::MIN_PASSPHRASE
        ));
    }
    let key = state.log_key(true)?.ok_or("There's no log key yet.")?;
    let file = tauri::async_runtime::spawn_blocking(move || crypt::export_key(&key, &passphrase))
        .await
        .map_err(|e| e.to_string())??;
    let Some(path) = dialogs::save_json_file(
        &app,
        window.label(),
        "Export the log key",
        "Chummy log key",
        "chummy-log-key.json",
    )
    .await
    else {
        return Ok(false);
    };
    std::fs::write(&path, file).map_err(|e| format!("Couldn't save {}: {e}", path.display()))?;
    syslog::log(Level::Warn, "security", "The log key was exported", &[]);
    Ok(true)
}

/// Loads a log key from a file, replacing the current one (after asking, if
/// there is one). Returns the new key's fingerprint.
#[tauri::command]
pub async fn import_log_key(
    app: AppHandle,
    window: WebviewWindow,
    state: State<'_, AppState>,
    passphrase: String,
) -> CmdResult<Option<String>> {
    let Some(path) =
        dialogs::pick_json_file(&app, window.label(), "Import a log key", "Chummy log key").await
    else {
        return Ok(None);
    };
    if std::fs::metadata(&path).map(|m| m.len()).unwrap_or(0) > MAX_KEY_FILE {
        return Err("That file is too big to be a log key.".into());
    }
    let text = std::fs::read_to_string(&path).map_err(|e| format!("Couldn't read it: {e}"))?;
    let key = tauri::async_runtime::spawn_blocking(move || crypt::import_key(&text, &passphrase))
        .await
        .map_err(|e| e.to_string())??;
    if let Some(current) = state.log_key(false).ok().flatten() {
        if current == key {
            return Ok(Some(key.fingerprint()));
        }
        let msg = format!(
            "The key you're importing ({}) will replace this computer's log key ({}).\n\n\
             Logs written with the current key can't be read here afterwards, unless you export it \
             first and import it again later. New logs use the imported key.",
            key.fingerprint(),
            current.fingerprint()
        );
        if !dialogs::confirm(
            &app,
            window.label(),
            "Replace the log key?",
            &msg,
            "Replace",
            true,
        )
        .await
        {
            return Ok(None);
        }
    }
    let fp = key.fingerprint();
    state.set_log_key(key)?;
    syslog::log(
        Level::Warn,
        "security",
        "A log key was imported",
        &[("fingerprint", &fp)],
    );
    Ok(Some(fp))
}

/// Opens the logs folder in the file manager.
#[tauri::command]
pub fn reveal_logs(app: AppHandle, state: State<'_, AppState>) -> CmdResult<()> {
    use tauri_plugin_opener::OpenerExt;
    app.opener()
        .open_path(state.dirs.logs.to_string_lossy(), None::<&str>)
        .map_err(|e| e.to_string())
}
