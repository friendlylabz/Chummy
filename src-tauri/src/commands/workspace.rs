//! Choosing the workspace and changing permissions.
//!
//! The UI never sends a path: Rust shows the folder picker, validates the
//! choice and asks for confirmation itself.

use serde::Deserialize;
use tauri::{AppHandle, State, WebviewWindow};

use super::{CmdResult, now};
use crate::dialogs;
use crate::settings::Workspace;
use crate::state::AppState;

/// Lets the user pick, then confirm, the workspace folder. Returns `false` if
/// they cancelled.
#[tauri::command]
pub async fn choose_workspace(
    app: AppHandle,
    window: WebviewWindow,
    state: State<'_, AppState>,
) -> CmdResult<bool> {
    let label = window.label().to_string();
    let Some(picked) = dialogs::pick_folder(&app, &label, "Choose the folder agents may use").await
    else {
        return Ok(false);
    };

    let path = match chummy_sandbox::validate_workspace(&picked, &state.protected) {
        Ok(p) => p,
        Err(e) => {
            let reason = e.to_string();
            let reason = reason[..1].to_uppercase() + &reason[1..];
            let msg = format!(
                "{}\n\n{reason}.\n\nFor safety, agents can never use a whole drive, your home folder, system \
                 folders, hidden settings folders, or Chummy's own folder. Pick or create a folder just \
                 for your agents' work, such as Documents/Agent workspace.",
                picked.display()
            );
            dialogs::error(&app, &label, "That folder can't be used", &msg).await;
            return Ok(false);
        }
    };

    let msg = format!(
        "{}\n\nAgents will be able to read, create, change and delete files in this folder and its \
         subfolders. They won't be able to see anything outside it.",
        path.display()
    );
    if !dialogs::confirm(
        &app,
        &label,
        "Use this folder for agents?",
        &msg,
        "Use this folder",
        true,
    )
    .await
    {
        return Ok(false);
    }

    state.update(&app, |s| {
        s.workspace = Some(Workspace {
            path,
            confirmed_at: now(),
        })
    })?;
    Ok(true)
}

#[tauri::command]
pub fn clear_workspace(app: AppHandle, state: State<'_, AppState>) -> CmdResult<()> {
    state.update(&app, |s| s.workspace = None)
}

#[derive(Debug, Clone, Copy, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PermissionKind {
    Network,
    Tools,
}

/// Turns a permission on (after a native confirmation) or off (immediately).
/// Returns the new value.
#[tauri::command]
pub async fn set_permission(
    app: AppHandle,
    window: WebviewWindow,
    state: State<'_, AppState>,
    kind: PermissionKind,
    enabled: bool,
) -> CmdResult<bool> {
    if enabled {
        let (title, msg, ok) = match kind {
            PermissionKind::Network => (
                "Allow internet access?",
                "Agents and their tools will be able to reach the internet. They could send content \
                 from your workspace to websites.\n\nThis doesn't affect the model connections you set \
                 up in Models; those are separate.",
                "Allow internet access",
            ),
            PermissionKind::Tools => (
                "Allow tools?",
                "Agents will be able to run the tools you install. Tools stay locked to your workspace \
                 folder, and still need internet access turned on to go online.",
                "Allow tools",
            ),
        };
        if !dialogs::confirm(&app, window.label(), title, msg, ok, true).await {
            return Ok(false);
        }
    }
    state.update(&app, |s| match kind {
        PermissionKind::Network => s.permissions.network = enabled,
        PermissionKind::Tools => s.permissions.tools = enabled,
    })?;
    Ok(enabled)
}
