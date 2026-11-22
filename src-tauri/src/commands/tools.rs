//! Tools: setting up the sandbox, and each tool's switches.
//!
//! Tools are off by default. Using one needs the Tools permission and the
//! tool's own switch. A tool that asks for the internet also needs the
//! Internet permission and its own internet switch, which asks for
//! confirmation. There is no way to run a tool outside the sandbox.

use serde::Serialize;
use tauri::{AppHandle, Emitter, State, WebviewWindow};

use super::{CmdResult, err};
use crate::dialogs;
use crate::events;
use crate::state::{AppState, lock, tools_http};

#[derive(Debug, Clone, Serialize)]
pub struct ToolView {
    #[serde(flatten)]
    pub manifest: chummy_tools::ToolManifest,
    pub enabled: bool,
    /// The user let this tool use the internet.
    pub network_allowed: bool,
    /// Every call waits for the user's OK.
    pub ask: bool,
}

#[derive(Debug, Clone, Serialize)]
pub struct ToolsStatus {
    /// The WebAssembly Python, once installed.
    pub runtime: Option<chummy_tools::PythonInfo>,
    pub python_version: &'static str,
    pub download_bytes: u64,
    pub installing: bool,
    pub tools: Vec<ToolView>,
    /// How many tools ship with Chummy.
    pub builtin_count: usize,
}

pub fn status(state: &AppState) -> ToolsStatus {
    let s = state.settings();
    ToolsStatus {
        runtime: chummy_tools::python_status(&state.tool_paths),
        python_version: chummy_tools::PYTHON_VERSION,
        download_bytes: chummy_tools::PYTHON_DOWNLOAD_BYTES,
        installing: *lock(&state.tools_installing),
        tools: chummy_tools::installed(&state.tool_paths)
            .into_iter()
            .map(|m| {
                let pref = s.tool(&m.name);
                ToolView {
                    enabled: pref.enabled,
                    network_allowed: pref.network && m.network,
                    ask: pref.ask,
                    manifest: m,
                }
            })
            .collect(),
        builtin_count: chummy_tools::builtins().len(),
    }
}

#[derive(Debug, Clone, Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ToolsProgress {
    Download { done: u64, total: u64 },
    Step { message: String },
    Finished,
    Failed { error: String },
}

#[tauri::command]
pub fn tools_status(state: State<'_, AppState>) -> ToolsStatus {
    status(&state)
}

fn changed(app: &AppHandle, state: &AppState) {
    let _ = app.emit(events::TOOLS_CHANGED, status(state));
}

/// Downloads the WebAssembly Python and installs the built-in tools. Asks
/// first. Returns false if the user said no.
#[tauri::command]
pub async fn setup_tools(
    app: AppHandle,
    window: WebviewWindow,
    state: State<'_, AppState>,
) -> CmdResult<bool> {
    if *lock(&state.tools_installing) {
        return Err("Tools are already being set up.".into());
    }
    let mb = chummy_tools::PYTHON_DOWNLOAD_BYTES as f64 / 1_000_000.0;
    let msg = format!(
        "Chummy will download Python {} for WebAssembly (about {mb:.0} MB, from GitHub) and the few packages its \
         built-in tools need (from PyPI). Everything goes in the Chummy folder.\n\nTools run in a sandbox that \
         can only see the agent's workspace folder. Each tool stays off until you switch it on.",
        chummy_tools::PYTHON_VERSION
    );
    if !dialogs::confirm(
        &app,
        window.label(),
        "Set up tools?",
        &msg,
        "Download and set up",
        false,
    )
    .await
    {
        return Ok(false);
    }
    *lock(&state.tools_installing) = true;
    changed(&app, &state);

    let app2 = app.clone();
    tauri::async_runtime::spawn(async move {
        use tauri::Manager;
        let state = app2.state::<AppState>();
        let emit = |p: ToolsProgress| {
            let _ = app2.emit(events::TOOLS_PROGRESS, p);
        };
        let result: Result<(), String> = async {
            emit(ToolsProgress::Step {
                message: "Downloading Python".into(),
            });
            let mut last = 0u64;
            chummy_tools::install_python(&state.tool_paths, &tools_http(), |done, total| {
                if done - last > 256 * 1024 || done == total {
                    last = done;
                    emit(ToolsProgress::Download { done, total });
                }
            })
            .await
            .map_err(err)?;
            emit(ToolsProgress::Step {
                message: "Preparing the sandbox (only slow the first time)".into(),
            });
            let host = state.tool_host().await?;
            chummy_tools::install_builtins(&host, &mut |m| {
                emit(ToolsProgress::Step { message: m.into() })
            })
            .await
            .map_err(err)?;
            Ok(())
        }
        .await;
        *lock(&state.tools_installing) = false;
        match result {
            Ok(()) => emit(ToolsProgress::Finished),
            Err(e) => emit(ToolsProgress::Failed {
                error: e[..1].to_uppercase() + &e[1..],
            }),
        }
        changed(&app2, &state);
    });
    Ok(true)
}

/// Switches a tool on or off for agents.
#[tauri::command]
pub fn set_tool_enabled(
    app: AppHandle,
    state: State<'_, AppState>,
    name: String,
    enabled: bool,
) -> CmdResult<()> {
    if chummy_tools::manifest(&state.tool_paths, &name).is_none() {
        return Err(format!("The tool “{name}” isn't installed."));
    }
    state.update(&app, |s| s.tools.entry(name).or_default().enabled = enabled)?;
    changed(&app, &state);
    Ok(())
}

/// Lets a tool that asks for it use the internet. Asks first.
#[tauri::command]
pub async fn set_tool_network(
    app: AppHandle,
    window: WebviewWindow,
    state: State<'_, AppState>,
    name: String,
    enabled: bool,
) -> CmdResult<bool> {
    let m = chummy_tools::manifest(&state.tool_paths, &name)
        .ok_or_else(|| format!("The tool “{name}” isn't installed."))?;
    if enabled {
        if !m.network {
            return Err(format!("“{}” doesn't use the internet.", m.title));
        }
        let msg = format!(
            "“{}” will be able to make web requests for agents: {}\n\nIt can't reach this computer or your \
             local network, and it still can't see anything outside the agent's workspace. Internet access \
             also has to be on in Permissions.",
            m.title, m.description
        );
        let title = format!("Let “{}” use the internet?", m.title);
        if !dialogs::confirm(&app, window.label(), &title, &msg, "Allow", true).await {
            return Ok(false);
        }
    }
    state.update(&app, |s| s.tools.entry(name).or_default().network = enabled)?;
    changed(&app, &state);
    Ok(enabled)
}

/// Makes every call to a tool wait for the user's OK (or stop waiting).
/// Asking is the safe side, so neither direction needs a confirmation.
#[tauri::command]
pub fn set_tool_ask(
    app: AppHandle,
    state: State<'_, AppState>,
    name: String,
    ask: bool,
) -> CmdResult<()> {
    if chummy_tools::manifest(&state.tool_paths, &name).is_none() {
        return Err(format!("The tool “{name}” isn't installed."));
    }
    state.update(&app, |s| s.tools.entry(name).or_default().ask = ask)?;
    changed(&app, &state);
    Ok(())
}
