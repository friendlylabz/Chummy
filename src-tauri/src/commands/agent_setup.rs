//! Per-agent switches and Agent Loop Configuration.
//!
//! Tools and internet access are switched on at three levels, and all three
//! must be on: the app (Settings → Permissions and Tools), the agent (here),
//! and the block in the loop. Agent Loop Configuration changes block
//! settings for one agent without changing the loop file.

use std::collections::BTreeSet;

use chummy_flow::{BlockKind, LoopConfig, LoopFile, Overrides, analyze, find_block};
use serde::Serialize;
use tauri::{AppHandle, State, WebviewWindow};

use super::{CmdResult, err, review};
use crate::dialogs;
use crate::state::AppState;

/// Largest configuration file imported.
const MAX_IMPORT_BYTES: u64 = 1024 * 1024;

/// What [`set_agent_access`] saved (less than asked if a warning was declined).
#[derive(Debug, Clone, Serialize)]
pub struct AgentAccess {
    pub tools: Vec<String>,
    pub network: bool,
}

/// Sets which tools an agent may use and whether they may go online.
/// Turning internet access on asks first, and so does giving a potentially
/// unsafe tool to an agent that shares its folder. Returns what was saved.
#[tauri::command]
pub async fn set_agent_access(
    app: AppHandle,
    window: WebviewWindow,
    state: State<'_, AppState>,
    agent_id: String,
    tools: Vec<String>,
    network: bool,
) -> CmdResult<AgentAccess> {
    let s = state.settings();
    let agent = s.agent(&agent_id).ok_or("That agent no longer exists.")?;
    let installed: BTreeSet<String> = chummy_tools::installed(&state.tool_paths)
        .into_iter()
        .map(|m| m.name)
        .collect();
    let mut tools: BTreeSet<String> = tools
        .into_iter()
        .filter(|t| installed.contains(t))
        .collect();
    // A potentially unsafe tool for an agent that shares its folder: the
    // other agents' files (and whatever they leave there) reach it too.
    let others = super::agents::overlapping_agents(&s, &agent.workspace.path, Some(&agent.id));
    let risky = super::agents::risky_tools(&state);
    let new_risky: Vec<String> = tools
        .iter()
        .filter(|t| risky.contains(*t) && !agent.tools.contains(*t))
        .cloned()
        .collect();
    if !others.is_empty() && !new_risky.is_empty() {
        let names: Vec<String> = others.iter().map(|a| format!("“{}”", a.name)).collect();
        let msg = format!(
            "“{}” shares its folder with {}. A file another agent leaves there (for example from a \
             web page it read) can carry instructions this agent follows with {}.\n\nThe tool still \
             runs in the sandbox and can't see outside the folder.",
            agent.name,
            names.join(", "),
            new_risky.join(", ")
        );
        let title = format!("Give “{}” {}?", agent.name, new_risky.join(", "));
        if !dialogs::confirm(&app, window.label(), &title, &msg, "Allow", true).await {
            tools.retain(|t| !new_risky.contains(t));
        }
    }
    let mut network = network;
    if network && !agent.network {
        let mut msg = format!(
            "Tools that “{}” uses will be able to reach the internet, if internet access is also on \
             in Security and Privacy, for that tool, and in the loop's block.\n\nA web page can contain \
             instructions meant to trick the model. Tools that go online ask you first unless you \
             turned that off.",
            agent.name
        );
        if !others.is_empty() {
            let names: Vec<String> = others.iter().map(|a| format!("“{}”", a.name)).collect();
            msg.push_str(&format!(
                "\n\nThis agent shares its folder with {}. Their files will be within reach of \
                 what this agent sends online, and anything it brings back from the web, the others \
                 can read and act on.",
                names.join(", ")
            ));
        }
        let title = format!("Let “{}” use the internet?", agent.name);
        if !dialogs::confirm(&app, window.label(), &title, &msg, "Allow", true).await {
            network = false;
        }
    }
    let saved = AgentAccess {
        tools: tools.iter().cloned().collect(),
        network,
    };
    state.update(&app, |s| {
        if let Some(a) = s.agents.iter_mut().find(|a| a.id == agent_id) {
            a.tools = tools;
            a.network = network;
        }
    })?;
    Ok(saved)
}

#[derive(Debug, Clone, Serialize)]
pub struct AgentLoopConfig {
    pub loop_id: String,
    /// The loop as saved (the defaults).
    pub file: LoopFile,
    /// This agent's changes.
    pub overrides: Overrides,
    /// Changes for blocks the loop no longer has.
    pub missing: Vec<String>,
}

#[tauri::command]
pub fn agent_loop_config(
    state: State<'_, AppState>,
    agent_id: String,
) -> CmdResult<AgentLoopConfig> {
    let s = state.settings();
    let agent = s.agent(&agent_id).ok_or("That agent no longer exists.")?;
    let file = state.library.load(&agent.loop_id)?;
    let overrides = agent.overrides(&agent.loop_id);
    let (_, missing) = file.with_overrides(&overrides);
    Ok(AgentLoopConfig {
        loop_id: agent.loop_id.clone(),
        file,
        overrides,
        missing,
    })
}

/// Checks that `overrides` leave every block valid. Returns the reason if not.
fn check(file: &LoopFile, overrides: &Overrides) -> CmdResult<()> {
    let (applied, _) = file.with_overrides(overrides);
    for key in overrides.keys() {
        if let Some(b) = find_block(&applied, key) {
            BlockKind::parse(&b.kind, &b.params).map_err(|e| {
                format!(
                    "“{}” can't take that setting: {e}",
                    if b.label.is_empty() { &b.id } else { &b.label }
                )
            })?;
        }
    }
    if let Some(e) = analyze(&applied)
        .issues
        .iter()
        .find(|i| i.severity == chummy_flow::Severity::Error)
    {
        return Err(format!("That setting would break the loop: {}", e.message));
    }
    Ok(())
}

/// Sets one block's changed settings for an agent (`None` resets the block
/// to the loop's own settings).
#[tauri::command]
pub fn set_agent_block_config(
    app: AppHandle,
    state: State<'_, AppState>,
    agent_id: String,
    block: String,
    params: Option<serde_json::Map<String, serde_json::Value>>,
) -> CmdResult<()> {
    let s = state.settings();
    let agent = s.agent(&agent_id).ok_or("That agent no longer exists.")?;
    let file = state.library.load(&agent.loop_id)?;
    if find_block(&file, &block).is_none() {
        return Err("That block isn't in the agent's loop.".into());
    }
    let mut overrides = agent.overrides(&agent.loop_id);
    match params.filter(|p| !p.is_empty()) {
        Some(p) => {
            overrides.insert(block, p);
        }
        None => {
            overrides.remove(&block);
        }
    }
    check(&file, &overrides)?;
    let loop_id = agent.loop_id.clone();
    state.update(&app, |s| {
        if let Some(a) = s.agents.iter_mut().find(|a| a.id == agent_id) {
            if overrides.is_empty() {
                a.loop_config.remove(&loop_id);
            } else {
                a.loop_config.insert(loop_id, overrides);
            }
        }
    })
}

/// Sets an agent's changes to its model's settings (empty: inherit all).
#[tauri::command]
pub fn set_agent_model_options(
    app: AppHandle,
    state: State<'_, AppState>,
    agent_id: String,
    options: chummy_llm::ModelOverrides,
) -> CmdResult<()> {
    if let Some(p) = options.problems().into_iter().next() {
        return Err(p);
    }
    if state.settings().agent(&agent_id).is_none() {
        return Err("That agent no longer exists.".into());
    }
    state.update(&app, |s| {
        if let Some(a) = s.agents.iter_mut().find(|a| a.id == agent_id) {
            a.model_options = options;
        }
    })
}

/// Saves an agent's loop configuration to a file.
#[tauri::command]
pub async fn export_agent_loop_config(
    app: AppHandle,
    window: WebviewWindow,
    state: State<'_, AppState>,
    agent_id: String,
) -> CmdResult<bool> {
    let s = state.settings();
    let agent = s.agent(&agent_id).ok_or("That agent no longer exists.")?;
    let file = state.library.load(&agent.loop_id)?;
    let config = LoopConfig::new(&file.name, agent.overrides(&agent.loop_id));
    let suggested = format!("{}.loopconfig.json", agent.loop_id);
    let Some(path) = dialogs::save_json_file(
        &app,
        window.label(),
        "Export loop configuration",
        "Loop configuration",
        &suggested,
    )
    .await
    else {
        return Ok(false);
    };
    std::fs::write(&path, config.to_json())
        .map_err(|e| format!("Couldn't save {}: {e}", path.display()))?;
    Ok(true)
}

/// Loads a loop configuration file into an agent. Settings for blocks the
/// agent's loop doesn't have are skipped. Returns how many blocks it set.
#[tauri::command]
pub async fn import_agent_loop_config(
    app: AppHandle,
    window: WebviewWindow,
    state: State<'_, AppState>,
    agent_id: String,
) -> CmdResult<Option<usize>> {
    let Some(path) = dialogs::pick_json_file(
        &app,
        window.label(),
        "Import loop configuration",
        "Loop configuration",
    )
    .await
    else {
        return Ok(None);
    };
    let read = (|| {
        let len = std::fs::metadata(&path).map_err(err)?.len();
        if len > MAX_IMPORT_BYTES {
            return Err("That file is too big to be a loop configuration.".to_string());
        }
        LoopConfig::from_json(&std::fs::read_to_string(&path).map_err(err)?).map_err(err)
    })();
    let config = match read {
        Ok(c) => c,
        Err(e) => {
            dialogs::error(
                &app,
                window.label(),
                "Couldn't import that configuration",
                &e,
            )
            .await;
            return Ok(None);
        }
    };
    let s = state.settings();
    let agent = s.agent(&agent_id).ok_or("That agent no longer exists.")?;
    let file = state.library.load(&agent.loop_id)?;
    let overrides: Overrides = config
        .blocks
        .into_iter()
        .filter(|(k, _)| find_block(&file, k).is_some())
        .collect();
    if let Err(e) = check(&file, &overrides) {
        dialogs::error(
            &app,
            window.label(),
            "Couldn't import that configuration",
            &e,
        )
        .await;
        return Ok(None);
    }
    let n = overrides.len();
    let loop_id = agent.loop_id.clone();
    // New settings can rewrite a loop's prompts, so unless the loop they
    // make is one Chummy already trusts, they're reviewed first.
    let (effective, _) = file.with_overrides(&overrides);
    if n > 0 && !state.trusted(&effective) {
        let changes = review::config_changes(&file, &overrides);
        review::start(
            &app,
            &state,
            effective,
            review::ReviewSource::LoopConfig {
                agent_id: agent_id.clone(),
                agent_name: agent.name.clone(),
                loop_id: loop_id.clone(),
                changes,
                base_print: chummy_flow::fingerprint(&file),
                overrides,
            },
        );
        return Ok(None);
    }
    state.update(&app, |s| {
        if let Some(a) = s.agents.iter_mut().find(|a| a.id == agent_id) {
            a.loop_config.insert(loop_id, overrides);
        }
    })?;
    Ok(Some(n))
}

#[derive(Debug, Clone, Serialize)]
pub struct UsageView {
    pub provider_id: String,
    pub name: String,
    pub used: u64,
    pub extra: u64,
    pub calls: u32,
    /// `None`: no limit.
    pub limit: Option<u64>,
}

/// Every model connection's token use today.
#[tauri::command]
pub fn usage_today(state: State<'_, AppState>) -> Vec<UsageView> {
    let s = state.settings();
    let today = state.usage.all_today();
    s.providers
        .iter()
        .map(|p| {
            let d = today.get(&p.id).copied().unwrap_or_default();
            UsageView {
                provider_id: p.id.clone(),
                name: p.name.clone(),
                used: d.used,
                extra: d.extra,
                calls: d.calls,
                limit: p.daily_tokens,
            }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn bad_settings_are_refused() {
        let file = chummy_flow::templates()
            .into_iter()
            .find(|t| t.id == "simple_chat")
            .unwrap()
            .file;
        let block = file
            .blocks
            .iter()
            .find(|b| b.kind == "llm")
            .unwrap()
            .id
            .clone();
        let mut o = Overrides::new();
        o.insert(
            block.clone(),
            json!({ "temperature": 0.3 }).as_object().unwrap().clone(),
        );
        assert!(check(&file, &o).is_ok());
        o.insert(
            block.clone(),
            json!({ "temperature": "hot" }).as_object().unwrap().clone(),
        );
        assert!(check(&file, &o).is_err());
        o.insert(
            block,
            json!({ "max_tokens": 0 }).as_object().unwrap().clone(),
        );
        assert!(check(&file, &o).unwrap_err().contains("break the loop"));
    }
}
