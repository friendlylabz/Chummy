//! The sandbox tools run in (Security and Privacy → Sandbox).
//!
//! One sandbox for the whole app, Native by default. Everything that
//! changes what runs tools asks natively, from here:
//!
//! - switching to Container or OpenShell;
//! - the image a container runs: Chummy runs exactly the image ID the user
//!   approved, never whatever a tag points to later;
//! - the program Chummy starts (`docker`, `podman`, `openshell`): chosen
//!   only with a native file picker, never typed in by a window, and never
//!   from a folder an agent can write to.
//!
//! Switching back to Native never asks: it's the built-in sandbox.

use std::path::Path;

use chummy_tools::sandbox::container::{short_id, valid_image};
use chummy_tools::sandbox::{ContainerEngine, ContainerOptions};
use chummy_tools::{Health, SandboxKind, SandboxSettings};
use serde::{Deserialize, Serialize};
use std::sync::atomic::Ordering;
use tauri::{AppHandle, Manager, State, WebviewWindow};

use super::CmdResult;
use crate::dialogs;
use crate::state::AppState;

#[derive(Debug, Clone, Serialize)]
pub struct KindInfo {
    pub kind: SandboxKind,
    pub label: &'static str,
    /// Tools can start other programs in it.
    pub programs: bool,
}

#[derive(Debug, Clone, Serialize)]
pub struct SandboxStatus {
    pub settings: SandboxSettings,
    pub health: Health,
    /// A check is running.
    pub checking: bool,
    pub kinds: Vec<KindInfo>,
    pub default_image: &'static str,
}

pub fn status(state: &AppState) -> SandboxStatus {
    let s = state.settings().sandbox;
    SandboxStatus {
        health: state.sandbox_health(&s),
        settings: s,
        checking: state.sandbox_checking.load(Ordering::Relaxed),
        kinds: SandboxKind::ALL
            .into_iter()
            .map(|k| KindInfo {
                kind: k,
                label: k.label(),
                programs: k.programs(),
            })
            .collect(),
        default_image: chummy_tools::sandbox::container::DEFAULT_IMAGE,
    }
}

/// Tells every window, and refreshes the tool list (what's usable).
pub fn changed(app: &AppHandle) {
    let state = app.state::<AppState>();
    super::tools::changed(app, &state);
}

/// Checks the sandbox in the background (again, if the settings change
/// while it runs). Native needs no check.
pub fn check_soon(app: &AppHandle) {
    let state = app.state::<AppState>();
    if state.settings().sandbox.kind == SandboxKind::Native {
        changed(app);
        return;
    }
    if state.sandbox_checking.swap(true, Ordering::SeqCst) {
        return;
    }
    changed(app);
    let app = app.clone();
    tauri::async_runtime::spawn(async move {
        let state = app.state::<AppState>();
        loop {
            let before = state.settings().sandbox;
            let h = state.check_sandbox().await;
            log_health(&h);
            if state.settings().sandbox == before {
                break;
            }
        }
        state.sandbox_checking.store(false, Ordering::SeqCst);
        changed(&app);
    });
}

fn log_health(h: &Health) {
    use chummy_log::system::{Level, log};
    let python = h.python.clone().unwrap_or_default();
    log(
        if h.ready { Level::Info } else { Level::Warn },
        "tools",
        format!(
            "Sandbox {} {}",
            h.kind.label(),
            if h.ready { "is ready" } else { "isn't ready" }
        ),
        &[("message", h.message.as_str()), ("python", python.as_str())],
    );
}

#[tauri::command]
pub fn sandbox_status(state: State<'_, AppState>) -> SandboxStatus {
    status(&state)
}

/// Checks the sandbox now and returns what it found.
#[tauri::command]
pub async fn check_sandbox(app: AppHandle, state: State<'_, AppState>) -> CmdResult<Health> {
    state.sandbox_checking.store(true, Ordering::SeqCst);
    changed(&app);
    let h = state.check_sandbox().await;
    log_health(&h);
    state.sandbox_checking.store(false, Ordering::SeqCst);
    changed(&app);
    Ok(h)
}

/// Chooses where tools run. Returns false if the user said no.
#[tauri::command]
pub async fn set_sandbox_kind(
    app: AppHandle,
    window: WebviewWindow,
    state: State<'_, AppState>,
    kind: SandboxKind,
) -> CmdResult<bool> {
    if state.settings().sandbox.kind == kind {
        return Ok(true);
    }
    let ask = match kind {
        SandboxKind::Native => None,
        SandboxKind::Container => Some((
            "Run tools in containers?",
            "Each tool call will run in a fresh Docker or Podman container that sees only the agent's \
             workspace and has no network (tools still go online only through Chummy, when you allow it). \
             Tools there can run real programs.\n\n\
             Chummy uses the container engine already on this computer; it doesn't install one. A container \
             engine can do a lot on this computer, so only use one you trust. You'll choose the image next.",
            "Use containers",
        )),
        SandboxKind::Openshell => Some((
            "Run tools in OpenShell?",
            "Each tool call will run in an NVIDIA OpenShell sandbox with no network, through the openshell \
             program and your OpenShell gateway. Tools there can run real programs.\n\n\
             OpenShell can't open a folder directly, so Chummy copies the agent's workspace in for each call \
             and copies the changes back (keeping your version of any file you change meanwhile). \
             Chummy doesn't install OpenShell or set up a gateway.",
            "Use OpenShell",
        )),
    };
    if let Some((title, msg, ok)) = ask
        && !dialogs::confirm(&app, window.label(), title, msg, ok, true).await
    {
        return Ok(false);
    }
    state.update(&app, |s| s.sandbox.kind = kind)?;
    check_soon(&app);
    Ok(true)
}

/// What the page may change about containers: the engine, the image's name
/// and the limits. The program and the approved image ID are never taken
/// from a window.
#[derive(Debug, Clone, Deserialize)]
pub struct ContainerChoice {
    pub engine: ContainerEngine,
    pub image: String,
    pub memory_mb: u32,
    pub cpus: f64,
}

#[tauri::command]
pub async fn set_container_options(
    app: AppHandle,
    state: State<'_, AppState>,
    choice: ContainerChoice,
) -> CmdResult<()> {
    let image = choice.image.trim().to_string();
    if !valid_image(&image) {
        return Err(format!(
            "“{image}” isn't an image name Chummy can use (like python:3.14-slim)."
        ));
    }
    state.update(&app, |s| {
        let c = &mut s.sandbox.container;
        // A different image (or engine, which keeps its own images) needs
        // approving again.
        if c.image != image || c.engine != choice.engine {
            c.approved_id = None;
        }
        let next = ContainerOptions {
            engine: choice.engine,
            image,
            memory_mb: choice.memory_mb,
            cpus: choice.cpus,
            program: c.program.clone(),
            approved_id: c.approved_id.clone(),
        }
        .normalized();
        *c = next;
    })?;
    check_soon(&app);
    Ok(())
}

/// Gets the container image: downloads it (`pull`) or takes the copy
/// already on this computer. Asks first. Returns false if the user said no.
#[tauri::command]
pub async fn get_container_image(
    app: AppHandle,
    window: WebviewWindow,
    state: State<'_, AppState>,
    pull: bool,
) -> CmdResult<bool> {
    let opts = state.settings().sandbox.container;
    let image = opts.image.clone();
    let engine = opts.engine.label();
    if pull
        && !dialogs::confirm(
            &app,
            window.label(),
            &format!("Download {image}?"),
            &format!(
                "{engine} will download the image “{image}” from its registry (usually Docker Hub). \
                 Tools will then run inside it, so use an image you trust. The official Python images \
                 are a good choice."
            ),
            "Download",
            true,
        )
        .await
    {
        return Ok(false);
    }
    let id = state.container(opts.clone())?.get_image(pull).await?;
    if !pull
        && !dialogs::confirm(
            &app,
            window.label(),
            &format!("Use {image}?"),
            &format!(
                "Tools will run inside the copy of “{image}” already on this computer ({}). \
                 Use an image you trust.",
                short_id(&id)
            ),
            "Use this image",
            true,
        )
        .await
    {
        return Ok(false);
    }
    state.update(&app, |s| {
        let c = &mut s.sandbox.container;
        if c.image == image && c.engine == opts.engine {
            c.approved_id = Some(id.clone());
        }
    })?;
    check_soon(&app);
    Ok(true)
}

#[tauri::command]
pub async fn set_openshell_image(
    app: AppHandle,
    state: State<'_, AppState>,
    image: String,
) -> CmdResult<()> {
    let image = image.trim().to_string();
    if !image.is_empty() && !valid_image(&image) {
        return Err(format!("“{image}” isn't an image name Chummy can use."));
    }
    state.update(&app, |s| s.sandbox.openshell.image = image)?;
    check_soon(&app);
    Ok(())
}

#[derive(Debug, Clone, Copy, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ProgramFor {
    Container,
    Openshell,
}

/// Lets the user point Chummy at the container engine or `openshell`
/// program with a native file picker. Returns false if they cancelled.
#[tauri::command]
pub async fn choose_sandbox_program(
    app: AppHandle,
    window: WebviewWindow,
    state: State<'_, AppState>,
    which: ProgramFor,
) -> CmdResult<bool> {
    let what = match which {
        ProgramFor::Container => state.settings().sandbox.container.engine.label(),
        ProgramFor::Openshell => "OpenShell",
    };
    let Some(path) = dialogs::pick_program(
        &app,
        window.label(),
        &format!("Where is the {what} program?"),
    )
    .await
    else {
        return Ok(false);
    };
    let path = std::fs::canonicalize(&path).map_err(|e| format!("Can't use that file: {e}"))?;
    if !path.is_file() {
        return Err("That isn't a program file.".into());
    }
    if let Some(why) = forbidden_program(&state, &path) {
        return Err(why);
    }
    let shown = path.display().to_string();
    if !dialogs::confirm(
        &app,
        window.label(),
        &format!("Use this {what} program?"),
        &format!(
            "Chummy will start {shown} for every tool call, outside any sandbox, to create the sandbox. \
             Only choose the real {what} program."
        ),
        "Use it",
        true,
    )
    .await
    {
        return Ok(false);
    }
    state.update(&app, |s| match which {
        ProgramFor::Container => {
            s.sandbox.container.program = shown.clone();
            s.sandbox.container.approved_id = None;
        }
        ProgramFor::Openshell => s.sandbox.openshell.program = shown.clone(),
    })?;
    check_soon(&app);
    Ok(true)
}

/// Goes back to finding the program by itself.
#[tauri::command]
pub async fn clear_sandbox_program(
    app: AppHandle,
    state: State<'_, AppState>,
    which: ProgramFor,
) -> CmdResult<()> {
    state.update(&app, |s| match which {
        ProgramFor::Container => s.sandbox.container.program.clear(),
        ProgramFor::Openshell => s.sandbox.openshell.program.clear(),
    })?;
    check_soon(&app);
    Ok(())
}

/// A program in a folder an agent can write to, or in the Chummy folder,
/// could be replaced by a tool, and Chummy would then run it unsandboxed.
fn forbidden_program(state: &AppState, path: &Path) -> Option<String> {
    let s = state.settings();
    for a in &s.agents {
        let ws = std::fs::canonicalize(&a.workspace.path).unwrap_or(a.workspace.path.clone());
        if path.starts_with(&ws) {
            return Some(format!(
                "That file is in {}'s workspace, where tools can change it. Choose the program where it's installed.",
                a.name
            ));
        }
    }
    let root = std::fs::canonicalize(&state.dirs.root).unwrap_or(state.dirs.root.clone());
    if path.starts_with(&root) {
        return Some(
            "That file is in the Chummy folder. Choose the program where it's installed.".into(),
        );
    }
    None
}

/// Deletes the OpenShell sandboxes Chummy made. Asks first.
#[tauri::command]
pub async fn remove_openshell_sandboxes(
    app: AppHandle,
    window: WebviewWindow,
    state: State<'_, AppState>,
) -> CmdResult<Option<usize>> {
    if !dialogs::confirm(
        &app,
        window.label(),
        "Remove Chummy's OpenShell sandboxes?",
        "Chummy keeps one OpenShell sandbox per agent workspace between tool calls. Removing them frees \
         their space; Chummy makes new ones the next time a tool runs. Your workspaces aren't touched.",
        "Remove",
        false,
    )
    .await
    {
        return Ok(None);
    }
    let n = state.openshell()?.remove_all().await?;
    state.forget_sandbox();
    chummy_log::system::info("tools", format!("Removed {n} OpenShell sandboxes"));
    check_soon(&app);
    Ok(Some(n))
}
