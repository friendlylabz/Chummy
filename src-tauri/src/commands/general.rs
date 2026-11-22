//! Settings, theme, setup status, windows and app info.

use std::path::PathBuf;

use chummy_llm::ProviderConfig;
use chummy_store::Recovery;
use serde::Serialize;
use tauri::{AppHandle, State};
use tauri_plugin_opener::OpenerExt;

use super::{CmdResult, err};
use crate::settings::{LocalSettings, Permissions, Settings, Theme, Workspace};
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
    pub workspace: Option<Workspace>,
    pub permissions: Permissions,
    pub providers: Vec<ProviderView>,
    pub default_provider: Option<String>,
    pub local: LocalSettings,
    pub hf_token_set: bool,
}

pub fn settings_view(state: &AppState, s: &Settings) -> SettingsView {
    SettingsView {
        theme: s.theme,
        workspace: s.workspace.clone(),
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
    }
}

/// Whether a workspace, a model and a loop are configured. Chat stays disabled
/// until all three are.
#[tauri::command]
pub fn setup_status(state: State<'_, AppState>) -> SetupStatus {
    state.setup_status()
}

/// Opens (or focuses) a secondary window. Async on purpose: creating windows
/// from a synchronous command can deadlock on Windows.
#[tauri::command]
pub async fn open_window(app: AppHandle, window: AppWindow) -> CmdResult<()> {
    windows::open(&app, window).map_err(err)
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
    /// Set if settings had to be recovered from a backup or reset at startup.
    pub recovery: Option<Recovery>,
}

#[tauri::command]
pub fn app_info(state: State<'_, AppState>) -> AppInfo {
    AppInfo {
        version: env!("CARGO_PKG_VERSION"),
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
