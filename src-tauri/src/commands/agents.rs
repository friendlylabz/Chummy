//! Agents (each with a workspace folder) and permissions.
//!
//! Agents may share a folder, or have one inside another's, but only after a
//! native warning: a shared folder lets one agent leave files or
//! instructions that another, with different tools, later acts on.
//!
//! The UI never sends a filesystem path: Rust shows the folder picker,
//! validates the choice, asks for confirmation, and hands the UI a one-time
//! token that stands for the folder.

use std::collections::BTreeSet;
use std::path::Path;

use serde::{Deserialize, Serialize};
use tauri::{AppHandle, State, WebviewWindow};

use super::{CmdResult, err, now};
use crate::dialogs;
use crate::settings::{Agent, Settings, Workspace};
use crate::state::{AppState, lock};

#[derive(Serialize)]
pub struct PickedWorkspace {
    /// Pass this to `save_agent`.
    pub token: String,
    /// For display only.
    pub path: String,
}

/// Other agents whose workspace is `path`, inside it, or contains it.
pub(crate) fn overlapping_agents<'a>(
    s: &'a Settings,
    path: &Path,
    except: Option<&str>,
) -> Vec<&'a Agent> {
    s.agents
        .iter()
        .filter(|a| Some(a.id.as_str()) != except)
        .filter(|a| chummy_sandbox::paths_overlap(&a.workspace.path, path))
        .collect()
}

/// What an agent can do beyond reading and writing its folder, for the
/// sharing warnings ("can go online, can use Run shell command").
fn notable_access(s: &Settings, a: &Agent, risky: &BTreeSet<String>) -> Vec<String> {
    let mut out = Vec::new();
    if a.network && s.permissions.network {
        out.push("can go online".to_string());
    }
    let unsafe_tools: Vec<&str> = a
        .tools
        .iter()
        .filter(|t| risky.contains(*t))
        .map(|t| t.as_str())
        .collect();
    if !unsafe_tools.is_empty() && s.permissions.tools {
        out.push(format!("can use {}", unsafe_tools.join(", ")));
    }
    out
}

/// Names of installed tools marked potentially unsafe.
pub(crate) fn risky_tools(state: &AppState) -> BTreeSet<String> {
    chummy_tools::installed(&state.tool_paths)
        .into_iter()
        .filter(|m| m.risky)
        .map(|m| m.name)
        .collect()
}

/// The paragraph about sharing for the folder confirmation: which agents
/// share it and how, what they can do, and why that matters.
pub(crate) fn sharing_warning(
    s: &Settings,
    path: &Path,
    others: &[&Agent],
    risky: &BTreeSet<String>,
) -> String {
    let mut lines = Vec::new();
    let mut any_access = false;
    for a in others {
        let how = if a.workspace.path == path {
            "uses this same folder"
        } else if a.workspace.path.starts_with(path) {
            "has a folder inside it"
        } else {
            "has a folder that contains it"
        };
        let access = notable_access(s, a, risky);
        any_access |= !access.is_empty();
        let access = if access.is_empty() {
            String::new()
        } else {
            format!(" (it {})", access.join(" and "))
        };
        lines.push(format!(
            "• “{}” {how}: {}{access}",
            a.name,
            a.workspace.path.display()
        ));
    }
    let mut msg = format!(
        "Shared with other agents:\n{}\n\nAgents that share files can affect each other. Whatever one \
         agent reads or writes there, the others can read, change or delete, and a file one agent \
         leaves (for example from a web page it read) can carry instructions another agent follows \
         with its own tools. Two agents working at the same time can also overwrite each other's \
         changes.",
        lines.join("\n")
    );
    if any_access {
        msg.push_str(
            "\n\nCareful: an agent here can do more than read and write files. Anything that reaches \
             this folder can end up in what it sends online or the commands it runs. Share only \
             between agents you'd trust with each other's tools.",
        );
    }
    msg
}

/// A one-time token standing for a picked folder: refused if the system
/// can't give secure random numbers (an all-zero token could be guessed).
fn token() -> Result<String, String> {
    let mut b = [0u8; 16];
    getrandom::fill(&mut b).map_err(|e| format!("No secure random numbers: {e}"))?;
    Ok(b.iter().map(|x| format!("{x:02x}")).collect())
}

/// Lets the user pick and confirm a workspace folder for an agent. Returns
/// `None` if they cancelled or the folder can't be used (after explaining why).
#[tauri::command]
pub async fn pick_workspace(
    app: AppHandle,
    window: WebviewWindow,
    state: State<'_, AppState>,
    agent_id: Option<String>,
) -> CmdResult<Option<PickedWorkspace>> {
    let label = window.label().to_string();
    let Some(picked) =
        dialogs::pick_folder(&app, &label, "Choose this agent's workspace folder").await
    else {
        return Ok(None);
    };

    let path = match chummy_sandbox::validate_workspace(&picked, &state.protected) {
        Ok(p) => p,
        Err(e) => {
            let reason = e.to_string();
            let mut c = reason.chars();
            let reason: String = c
                .next()
                .map(|f| f.to_uppercase().chain(c).collect())
                .unwrap_or_default();
            let msg = format!(
                "{}\n\n{reason}.\n\nFor safety, agents can never use a whole drive, your home folder, system \
                 folders, hidden settings folders, or Chummy's own folder. Pick or create a folder just \
                 for this agent's work, such as Documents/Agents/Researcher.",
                picked.display()
            );
            dialogs::error(&app, &label, "That folder can't be used", &msg).await;
            return Ok(None);
        }
    };

    let settings = state.settings();
    let others = overlapping_agents(&settings, &path, agent_id.as_deref());

    let advice = chummy_sandbox::workspace_advice(&path);
    let mut msg = format!(
        "{}\n\nThis agent will be able to read, create, change and delete files in this folder and its \
         subfolders. It won't be able to see anything outside it.\n\nFiles the agent writes come from a model. \
         Check them before you open, run or share them, especially scripts, programs, shortcuts and project \
         settings (like a .git or .vscode folder): opening those outside Chummy isn't sandboxed.",
        path.display()
    );
    if !advice.is_empty() {
        msg.push_str(&format!(
            "\n\nCareful: {}\n\nA new, empty folder just for this agent is safer. Copy in only the files it needs.",
            advice.join(" ")
        ));
    }
    if !others.is_empty() {
        msg.push_str("\n\n");
        msg.push_str(&sharing_warning(
            &settings,
            &path,
            &others,
            &risky_tools(&state),
        ));
    }
    let (title, ok) = if !others.is_empty() {
        ("Share this folder with other agents?", "Share this folder")
    } else if advice.is_empty() {
        ("Give this agent this folder?", "Use this folder")
    } else {
        ("Give this agent this whole folder?", "Use it anyway")
    };
    if !dialogs::confirm(&app, &label, title, &msg, ok, true).await {
        return Ok(None);
    }

    let token = token()?;
    let display = path.display().to_string();
    lock(&state.picked_workspaces).insert(token.clone(), path);
    Ok(Some(PickedWorkspace {
        token,
        path: display,
    }))
}

#[derive(Debug, Deserialize)]
pub struct AgentDraft {
    /// Empty for a new agent.
    #[serde(default)]
    pub id: String,
    pub name: String,
    pub loop_id: String,
    #[serde(default)]
    pub model: Option<String>,
}

/// Creates or updates an agent. New agents need a `workspace_token` from
/// `pick_workspace`; existing agents keep their folder unless one is given.
#[tauri::command]
pub fn save_agent(
    app: AppHandle,
    state: State<'_, AppState>,
    agent: AgentDraft,
    workspace_token: Option<String>,
) -> CmdResult<String> {
    let name = agent.name.trim().to_string();
    if name.is_empty() {
        return Err("Give the agent a name.".into());
    }
    if state.library.load(&agent.loop_id).is_err() {
        return Err("Choose a loop for this agent.".into());
    }
    let s = state.settings();
    let model = agent.model.filter(|m| !m.is_empty());
    if let Some(m) = &model
        && s.provider(m).is_none()
    {
        return Err("That model connection no longer exists.".into());
    }
    let existing = (!agent.id.is_empty())
        .then(|| s.agent(&agent.id).cloned())
        .flatten();
    if !agent.id.is_empty() && existing.is_none() {
        return Err("That agent no longer exists.".into());
    }

    let workspace = match workspace_token {
        Some(t) => {
            let path = lock(&state.picked_workspaces)
                .remove(&t)
                .ok_or("Choose the workspace folder again.")?;
            // Re-check: the folder could have changed since it was picked.
            // (Sharing with other agents was confirmed when it was picked.)
            let path = chummy_sandbox::validate_workspace(&path, &state.protected).map_err(err)?;
            Workspace {
                path,
                confirmed_at: now(),
            }
        }
        None => existing
            .as_ref()
            .map(|a| a.workspace.clone())
            .ok_or("Choose a workspace folder for this agent.")?,
    };

    let id = existing
        .as_ref()
        .map(|a| a.id.clone())
        .unwrap_or_else(|| format!("agent-{}", chummy_store::random_id(4)));
    let created_at = existing.as_ref().map(|a| a.created_at).unwrap_or_else(now);
    // New agents start with no tools and no internet; existing ones keep theirs.
    let (tools, network, loop_config) = existing
        .as_ref()
        .map(|a| (a.tools.clone(), a.network, a.loop_config.clone()))
        .unwrap_or_default();
    let new = Agent {
        id: id.clone(),
        name,
        loop_id: agent.loop_id,
        model,
        workspace,
        created_at,
        tools,
        network,
        loop_config,
    };
    state.update(&app, |s| {
        match s.agents.iter_mut().find(|a| a.id == new.id) {
            Some(a) => *a = new,
            None => s.agents.push(new),
        }
        if s.active_agent.is_none() {
            s.active_agent = Some(id.clone());
        }
    })?;
    // Every agent starts with a chat, ready to type into.
    crate::chats::index(&state.dirs.chats, &id)?;
    Ok(id)
}

/// Deletes an agent and its chat history after a native confirmation. Files in
/// its workspace are left alone.
#[tauri::command]
pub async fn delete_agent(
    app: AppHandle,
    window: WebviewWindow,
    state: State<'_, AppState>,
    id: String,
) -> CmdResult<bool> {
    let Some(agent) = state.settings().agent(&id).cloned() else {
        return Ok(false);
    };
    let msg = format!(
        "“{}” and its chat history will be deleted. The files in its workspace ({}) are not touched.",
        agent.name,
        agent.workspace.path.display()
    );
    if !dialogs::confirm(
        &app,
        window.label(),
        "Delete this agent?",
        &msg,
        "Delete agent",
        true,
    )
    .await
    {
        return Ok(false);
    }
    crate::commands::chat::stop_agent_runs(&state, &id);
    crate::chats::delete_agent(&state.dirs.chats, &id)?;
    state.update(&app, |s| {
        s.agents.retain(|a| a.id != id);
        if s.active_agent.as_deref() == Some(id.as_str()) {
            s.active_agent = s.agents.first().map(|a| a.id.clone());
        }
    })?;
    Ok(true)
}

#[tauri::command]
pub fn set_active_agent(app: AppHandle, state: State<'_, AppState>, id: String) -> CmdResult<()> {
    if state.settings().agent(&id).is_none() {
        return Err("That agent no longer exists.".into());
    }
    state.update(&app, |s| s.active_agent = Some(id))
}

#[derive(Debug, Clone, Copy, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PermissionKind {
    Network,
    Tools,
    /// Asking before tools go online (on by default).
    ConfirmOnlineTools,
}

/// Changes a permission. Widening access (turning access on, or a
/// safeguard off) needs a native confirmation; narrowing it is immediate.
/// Returns the new value.
#[tauri::command]
pub async fn set_permission(
    app: AppHandle,
    window: WebviewWindow,
    state: State<'_, AppState>,
    kind: PermissionKind,
    enabled: bool,
) -> CmdResult<bool> {
    // Turning a safeguard off widens access just like turning access on.
    let widens = match kind {
        PermissionKind::ConfirmOnlineTools => !enabled,
        _ => enabled,
    };
    if widens {
        let (title, msg, ok) = match kind {
            PermissionKind::ConfirmOnlineTools => (
                "Stop asking before tools go online?",
                "Tools that can use the internet will run without waiting for your OK, including in \
                 scheduled jobs while you're away. A web page can contain instructions that trick a \
                 model into sending your files somewhere; asking first is how you'd catch that.\n\n\
                 Only turn this off if you trust every page your agents might read.",
                "Stop asking",
            ),
            PermissionKind::Network => (
                "Allow internet access?",
                "Tools you allow in Tools will be able to reach the internet (never this computer or \
                 your local network). They could send content from an agent's workspace to websites.\n\n\
                 This doesn't affect the model connections you set up in Models; those are separate.",
                "Allow internet access",
            ),
            PermissionKind::Tools => (
                "Allow tools?",
                "Agents will be able to run the tools you switch on in Tools. Each tool runs in a sandbox \
                 that can only see the agent's workspace folder, and can only go online if you also \
                 allow that tool and turn on internet access.",
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
        PermissionKind::ConfirmOnlineTools => s.permissions.confirm_online_tools = enabled,
    })?;
    Ok(enabled)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    fn agent(id: &str, path: &str, network: bool, tools: &[&str]) -> Agent {
        Agent {
            id: id.into(),
            name: id.to_uppercase(),
            loop_id: "assistant".into(),
            model: None,
            workspace: Workspace {
                path: PathBuf::from(path),
                confirmed_at: 0,
            },
            created_at: 0,
            tools: tools.iter().map(|t| t.to_string()).collect(),
            network,
            loop_config: Default::default(),
        }
    }

    #[test]
    fn sharing_is_found_and_explained() {
        let mut s = Settings::default();
        s.permissions.network = true;
        s.permissions.tools = true;
        s.agents = vec![
            agent("a", "/w/shared", false, &[]),
            agent("b", "/w/shared/sub", true, &["run_shell", "read_file"]),
            agent("c", "/w/other", true, &[]),
        ];
        let risky: BTreeSet<String> = ["run_shell".to_string()].into();
        let path = Path::new("/w/shared");

        let others = overlapping_agents(&s, path, Some("a"));
        assert_eq!(
            others.iter().map(|a| a.id.as_str()).collect::<Vec<_>>(),
            ["b"]
        );
        let msg = sharing_warning(&s, path, &others, &risky);
        assert!(msg.contains("“B” has a folder inside it"), "{msg}");
        assert!(msg.contains("can go online and can use run_shell"), "{msg}");
        assert!(msg.contains("Careful:"), "{msg}");

        // A new agent in the same folder as A, which can do nothing more.
        let others = overlapping_agents(&s, Path::new("/w/shared/sub/x"), None);
        let ids: Vec<&str> = others.iter().map(|a| a.id.as_str()).collect();
        assert_eq!(ids, ["a", "b"]);
        let only_a = overlapping_agents(&s, path, Some("b"));
        let msg = sharing_warning(&s, path, &only_a, &risky);
        assert!(msg.contains("“A” uses this same folder"), "{msg}");
        assert!(!msg.contains("Careful:"), "{msg}");

        // With internet off app-wide, B's online switch doesn't count.
        s.permissions.network = false;
        let msg = sharing_warning(&s, path, &overlapping_agents(&s, path, Some("a")), &risky);
        assert!(!msg.contains("can go online"), "{msg}");
    }
}
