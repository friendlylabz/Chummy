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
    /// Why the chosen sandbox can't run it (shown greyed out), if it can't.
    pub unusable: Option<String>,
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
    /// The sandbox tools run in, as last checked.
    pub sandbox: chummy_tools::Health,
}

pub fn status(state: &AppState) -> ToolsStatus {
    let s = state.settings();
    let health = state.sandbox_health(&s.sandbox);
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
                    unusable: chummy_tools::unusable(&m, &health),
                    manifest: m,
                }
            })
            .collect(),
        builtin_count: chummy_tools::builtins().len(),
        sandbox: health,
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

/// Tells every window about the tools, and the sandbox (whose health
/// depends on the tool runtime too).
pub fn changed(app: &AppHandle, state: &AppState) {
    let _ = app.emit(events::TOOLS_CHANGED, status(state));
    let _ = app.emit(events::SANDBOX_CHANGED, super::sandbox::status(state));
}

/// At start-up, once tools are set up: installs built-in tools that are new
/// in this version of Chummy and reinstalls ones whose file changed, so an
/// update brings its tools without setting up again. Their switches stay as
/// they were, and new tools start off like every tool.
pub fn refresh_builtins(app: &AppHandle) {
    use tauri::Manager;
    let app = app.clone();
    tauri::async_runtime::spawn(async move {
        let state = app.state::<AppState>();
        if chummy_tools::python_status(&state.tool_paths).is_none() {
            return;
        }
        {
            let mut busy = lock(&state.tools_installing);
            if *busy {
                return;
            }
            *busy = true;
        }
        let result = async {
            let host = state.tool_host().await?;
            chummy_tools::install_builtins(&host, &mut |m| {
                let _ = app.emit(
                    events::TOOLS_PROGRESS,
                    ToolsProgress::Step { message: m.into() },
                );
            })
            .await
            .map_err(err)
        }
        .await;
        *lock(&state.tools_installing) = false;
        match result {
            Ok(_) => {
                let _ = app.emit(events::TOOLS_PROGRESS, ToolsProgress::Finished);
            }
            Err(e) => chummy_log::system::warn(
                "tools",
                format!("Couldn't update the built-in tools: {e}"),
            ),
        }
        changed(&app, &state);
    });
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
                // By character: an error can start with “ or be empty.
                error: {
                    let mut c = e.chars();
                    c.next()
                        .map(|f| f.to_uppercase().chain(c).collect())
                        .unwrap_or_else(|| "Setting up tools failed.".to_string())
                },
            }),
        }
        changed(&app2, &state);
    });
    Ok(true)
}

/// Switches a tool on or off for agents. Turning on a tool marked
/// "potentially unsafe" (the command tool) asks first, and switches on
/// "Ask me before each use" for it. Returns whether the tool is now on.
#[tauri::command]
pub async fn set_tool_enabled(
    app: AppHandle,
    window: WebviewWindow,
    state: State<'_, AppState>,
    name: String,
    enabled: bool,
) -> CmdResult<bool> {
    let m = chummy_tools::manifest(&state.tool_paths, &name)
        .ok_or_else(|| format!("The tool “{name}” isn't installed."))?;
    let was_on = state.settings().tool(&name).enabled;
    let first_risky_on = enabled && m.risky && !was_on;
    if first_risky_on {
        let msg = format!(
            "“{}” is potentially unsafe.\n\n{}\n\nIt still runs in the sandbox, so it can't see anything outside the \
             agent's workspace or go online, but inside the workspace one command can change or delete many files \
             at once. Keep a copy of anything important in that folder.\n\nChummy will ask you before each use; you \
             can turn that off below the tool.",
            m.title, m.description
        );
        let title = format!("Turn on “{}”?", m.title);
        if !dialogs::confirm(&app, window.label(), &title, &msg, "Turn on", true).await {
            return Ok(false);
        }
    }
    state.update(&app, |s| {
        let pref = s.tools.entry(name).or_default();
        pref.enabled = enabled;
        if first_risky_on {
            pref.ask = true;
        }
    })?;
    changed(&app, &state);
    Ok(enabled)
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
             also has to be on in Security and Privacy.",
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
/// Asking is the safe side: turning it on never asks. Turning it off for a
/// potentially unsafe tool asks natively first. Returns false if the user
/// said no.
#[tauri::command]
pub async fn set_tool_ask(
    app: AppHandle,
    window: WebviewWindow,
    state: State<'_, AppState>,
    name: String,
    ask: bool,
) -> CmdResult<bool> {
    let Some(m) = chummy_tools::manifest(&state.tool_paths, &name) else {
        return Err(format!("The tool “{name}” isn't installed."));
    };
    if !ask && m.risky && state.settings().tool(&name).ask {
        let msg = format!(
            "“{}” is potentially unsafe: inside the agent's workspace one call can change or delete many files at \
             once.\n\nWithout asking, agents will use it whenever the model decides to, including in scheduled \
             jobs while you're away, and a file or web page the model reads could talk it into a harmful command.",
            m.title
        );
        let title = format!("Stop asking before each use of “{}”?", m.title);
        if !dialogs::confirm(&app, window.label(), &title, &msg, "Stop asking", true).await {
            return Ok(false);
        }
    }
    state.update(&app, |s| s.tools.entry(name).or_default().ask = ask)?;
    changed(&app, &state);
    Ok(true)
}
