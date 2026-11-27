//! Settings, theme, setup status, windows and app info.

use std::path::PathBuf;

use chummy_llm::ProviderConfig;
use chummy_store::Recovery;
use serde::Serialize;
use tauri::{AppHandle, State};
use tauri_plugin_opener::OpenerExt;

use super::{CmdResult, err};

use crate::settings::{Agent, General, LocalSettings, Logging, Permissions, Settings, Theme};
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
    /// Windows with a `--data-dir` folder: Chummy doesn't change Windows
    /// permissions, so others on the computer may be able to open it.
    pub data_dir_maybe_shared: bool,
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
        data_dir_maybe_shared: cfg!(windows)
            && default.as_deref() != Some(state.dirs.root.as_path()),
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
/// A link that's safe to show and open: http, https or mailto, no control
/// or invisible formatting characters (which can disguise an address), and
/// normalized, so a look-alike international domain shows as `xn--…`.
fn checked_link(raw: &str) -> Option<String> {
    let raw = raw.trim();
    let invisible = |c: char| {
        c.is_control()
            || c.is_whitespace()
            || matches!(c, '\u{200B}'..='\u{200F}' | '\u{202A}'..='\u{202E}' | '\u{2060}'..='\u{2064}' | '\u{2066}'..='\u{2069}' | '\u{FEFF}' | '\u{00AD}')
    };
    if raw.len() > 2000 || raw.chars().any(invisible) {
        return None;
    }
    let u = tauri::Url::parse(raw).ok()?;
    match u.scheme() {
        "http" | "https" if u.host_str().is_some() => Some(u.to_string()),
        "mailto" => Some(u.to_string()),
        _ => None,
    }
}

#[tauri::command]
pub async fn open_link(app: AppHandle, window: tauri::WebviewWindow, url: String) -> CmdResult<()> {
    let url = checked_link(&url).ok_or("Only web and email links can be opened.")?;
    let site = match tauri::Url::parse(&url)
        .ok()
        .and_then(|u| u.host_str().map(str::to_string))
    {
        Some(h) => format!("\n\nSite: {h}"),
        None => String::new(),
    };
    let msg = format!(
        "{url}{site}\n\nThis link came from a model's reply. Check the address before you open it."
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

#[cfg(test)]
mod tests {
    use super::checked_link;

    #[test]
    fn links_are_normalized_and_disguises_refused() {
        assert_eq!(
            checked_link("https://example.com/a").as_deref(),
            Some("https://example.com/a")
        );
        assert_eq!(
            checked_link(" mailto:me@example.com ").as_deref(),
            Some("mailto:me@example.com")
        );
        // A Cyrillic "а" in the name shows as punycode.
        assert!(
            checked_link("https://\u{0430}pple.com/")
                .unwrap()
                .starts_with("https://xn--")
        );
        for bad in [
            "javascript:alert(1)",
            "file:///etc/passwd",
            "https://example.com/\u{202E}gpj.exe",
            "https://exa\u{200B}mple.com",
            "https://a.com/ x",
        ] {
            assert!(checked_link(bad).is_none(), "{bad:?}");
        }
    }
}
