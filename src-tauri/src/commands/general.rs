//! Settings, theme, setup status, windows and app info.

use std::path::PathBuf;

use chummy_llm::ProviderConfig;
use chummy_store::Recovery;
use serde::Serialize;
use tauri::{AppHandle, State};
use tauri_plugin_opener::OpenerExt;

use super::{CmdResult, err};
use std::collections::BTreeMap;

use crate::settings::{
    Agent, General, LocalSettings, Logging, Permissions, Settings, Theme, ToolPref,
};
use crate::state::{AppState, HF_TOKEN_KEY, SetupStatus};
use crate::windows::{self, AppWindow};

/// A provider as the UI sees it: never the key itself, only whether one is set.
#[derive(Debug, Clone, Serialize)]
pub struct ProviderView {
    #[serde(flatten)]
    pub config: ProviderConfig,
    pub has_key: bool,
}

/// Settings as sent to windows. Contains no secrets.
#[derive(Debug, Clone, Serialize)]
pub struct SettingsView {
    pub theme: Theme,
    pub agents: Vec<Agent>,
    pub active_agent: Option<String>,
    pub permissions: Permissions,
    pub providers: Vec<ProviderView>,
    pub default_provider: Option<String>,
    pub local: LocalSettings,
    pub hf_token_set: bool,
    pub tools: BTreeMap<String, ToolPref>,
    pub general: General,
    pub logging: Logging,
    pub sandbox: chummy_tools::SandboxSettings,
}

pub fn settings_view(state: &AppState, s: &Settings) -> SettingsView {
    SettingsView {
        theme: s.theme,
        agents: s.agents.clone(),
        active_agent: s.active_agent.clone(),
        permissions: s.permissions.clone(),
        providers: s
            .providers
            .iter()
            .map(|p| ProviderView {
                has_key: state.has_secret(&p.secret_key()),
                config: p.clone(),
            })
            .collect(),
        default_provider: s.default_provider.clone(),
        local: s.local.clone(),
        hf_token_set: state.has_secret(HF_TOKEN_KEY),
        tools: s.tools.clone(),
        general: s.general.clone(),
        logging: s.logging.clone(),
        sandbox: s.sandbox.clone(),
    }
}

/// Whether a model, a loop and an agent are configured. Chat needs an agent.
#[tauri::command]
pub fn setup_status(state: State<'_, AppState>) -> SetupStatus {
    state.setup_status()
}

/// Opens (or focuses) a secondary window. Async on purpose: creating windows
/// from a synchronous command can deadlock on Windows.
#[tauri::command]
pub async fn open_window(
    app: AppHandle,
    window: AppWindow,
    section: Option<String>,
) -> CmdResult<()> {
    windows::open(&app, window, section.as_deref()).map_err(err)
}

#[tauri::command]
pub fn get_settings(state: State<'_, AppState>) -> SettingsView {
    settings_view(&state, &state.settings())
}

#[tauri::command]
pub fn set_theme(app: AppHandle, state: State<'_, AppState>, theme: Theme) -> CmdResult<()> {
    state.update(&app, |s| s.theme = theme)
}

#[derive(Serialize)]
pub struct AppInfo {
    pub version: &'static str,
    pub data_dir: PathBuf,
    /// Chosen with `--data-dir` rather than the default.
    pub data_dir_custom: bool,
    /// Only you can open it (Unix: 0700, everything inside private too).
    pub data_dir_locked: bool,
    /// Set if settings had to be recovered from a backup or reset at startup.
    pub recovery: Option<Recovery>,
}

#[tauri::command]
pub fn app_info(app: AppHandle, state: State<'_, AppState>) -> AppInfo {
    use tauri::Manager;
    let default = app.path().app_local_data_dir().ok();
    AppInfo {
        version: env!("CARGO_PKG_VERSION"),
        data_dir_custom: default.as_deref() != Some(state.dirs.root.as_path()),
        data_dir_locked: chummy_store::is_locked(&state.dirs.root),
        data_dir: state.dirs.root.clone(),
        recovery: state.recovery.clone(),
    }
}

/// Opens the Chummy folder in the system file manager.
#[tauri::command]
pub fn reveal_data_dir(app: AppHandle, state: State<'_, AppState>) -> CmdResult<()> {
    app.opener()
        .open_path(state.dirs.root.to_string_lossy(), None::<&str>)
        .map_err(err)
}

/// Saves the General settings (closing, notifications, scheduled runs).
#[tauri::command]
pub fn set_general(app: AppHandle, state: State<'_, AppState>, general: General) -> CmdResult<()> {
    let mut g = general;
    g.max_job_runs = g.max_job_runs.clamp(1, 4);
    let max = g.max_job_runs as usize;
    state.update(&app, |s| s.general = g)?;
    if let Some(m) = state.jobs.get() {
        m.set_max_running(max);
    }
    crate::tray::refresh(&app);
    Ok(())
}

/// Opens a link from a reply in the browser, after showing the full address
/// and asking. Only web and mail links.
#[tauri::command]
pub async fn open_link(app: AppHandle, window: tauri::WebviewWindow, url: String) -> CmdResult<()> {
    let url = url.trim().to_string();
    let lower = url.to_ascii_lowercase();
    let ok_scheme = ["https://", "http://", "mailto:"]
        .iter()
        .any(|p| lower.starts_with(p));
    if !ok_scheme || url.len() > 2000 || url.chars().any(|c| c.is_control() || c.is_whitespace()) {
        return Err("Only web and email links can be opened.".into());
    }
    let msg = format!(
        "{url}\n\nThis link came from a model's reply. Check the address before you open it."
    );
    if crate::dialogs::confirm(
        &app,
        window.label(),
        "Open this link in your browser?",
        &msg,
        "Open",
        false,
    )
    .await
    {
        app.opener().open_url(&url, None::<&str>).map_err(err)?;
    }
    Ok(())
}
