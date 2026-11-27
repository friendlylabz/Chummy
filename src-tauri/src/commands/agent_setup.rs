//! Per-agent switches and Agent Loop Configuration.
//!
//! Each agent has its own tool switchboard: which tools it may use, and for
//! each, whether every use asks first and whether it may go online. The app
//! keeps only the master switches (Security and Privacy), and a loop's block
//! can only switch a tool off for itself. Agent Loop Configuration changes
//! block settings for one agent without changing the loop file.

use std::collections::BTreeMap;

use chummy_flow::{BlockKind, LoopConfig, LoopFile, Overrides, analyze, find_block};
use serde::Serialize;
use tauri::{AppHandle, State, WebviewWindow};

use super::{CmdResult, err, review};
use crate::dialogs;
use crate::settings::AgentTool;
use crate::state::AppState;

/// Largest configuration file imported.
const MAX_IMPORT_BYTES: u64 = 1024 * 1024;

/// Sets an agent's tool switchboard: the tools it may use, each with "ask
/// first" and "internet". Returns what was saved, which is less than asked
/// where the user declined a confirmation:
/// - switching on a potentially unsafe tool asks, says so if the folder is
///   shared with other agents, and switches "ask first" on for it;
/// - letting a tool go online asks;
/// - no longer asking before a potentially unsafe tool asks.
///
/// Tools that aren't installed are dropped, and so is internet for tools
/// that don't use it.
#[tauri::command]
pub async fn set_agent_access(
    app: AppHandle,
    window: WebviewWindow,
    state: State<'_, AppState>,
    agent_id: String,
    tools: BTreeMap<String, AgentTool>,
) -> CmdResult<BTreeMap<String, AgentTool>> {
    let s = state.settings();
    let agent = s.agent(&agent_id).ok_or("That agent no longer exists.")?;
    let installed: BTreeMap<String, chummy_tools::ToolManifest> =
        chummy_tools::installed(&state.tool_paths)
            .into_iter()
            .map(|m| (m.name.clone(), m))
            .collect();
    let others = super::agents::overlapping_agents(&s, &agent.workspace.path, Some(&agent.id));
    let shared_with = (!others.is_empty()).then(|| {
        others
            .iter()
            .map(|a| format!("“{}”", a.name))
            .collect::<Vec<_>>()
            .join(", ")
    });

    let mut saved = BTreeMap::new();
    for (name, mut want) in tools {
        let Some(m) = installed.get(&name) else {
            continue;
        };
        let had = agent.tools.get(&name).copied();
        if !m.network {
            want.network = false;
        }
        if m.risky && had.is_none() {
            let mut msg = format!(
                "“{}” is potentially unsafe.\n\n{}\n\nIt still runs in the sandbox, so it can't see anything \
                 outside the agent's workspace, but inside the workspace one command can change or delete many \
                 files at once. Keep a copy of anything important in that folder.\n\nChummy will ask you \
                 before each use; you can turn that off in the agent's tools.",
                m.title, m.description
            );
            if let Some(names) = &shared_with {
                msg.push_str(&format!(
                    "\n\n“{}” shares its folder with {names}. A file another agent leaves there (for \
                     example from a web page it read) can carry instructions this agent follows with it.",
                    agent.name
                ));
            }
            let title = format!("Give “{}” {}?", agent.name, m.title);
            if !dialogs::confirm(&app, window.label(), &title, &msg, "Turn on", true).await {
                continue;
            }
            want.ask = true;
        }
        if want.network && !had.is_some_and(|t| t.network) {
            let direct = m.needs.iter().any(|n| n == "direct-network");
            let mut msg = if direct {
                format!(
                    "The programs “{}” starts for “{}” will get an ordinary network: git, pip, npm, curl \
                     and anything else can connect anywhere. Chummy can't check those connections, so \
                     unlike other tools they can also reach this computer and your local network \
                     (a router, a printer, services on this machine).\n\nThey still can't see files \
                     outside the agent's workspace, but they could send what's in it anywhere, and a web \
                     page or a downloaded script can contain instructions meant to trick the model. This \
                     works in the Container sandbox; OpenShell keeps the programs offline. Internet \
                     access also has to be on in Security and Privacy.",
                    m.title, agent.name
                )
            } else {
                format!(
                    "“{}” will be able to make web requests for “{}”: {}\n\nIt can't reach this computer or \
                     your local network, and it still can't see anything outside the agent's workspace. A web \
                     page can contain instructions meant to trick the model. Internet access also has to be on \
                     in Security and Privacy.",
                    m.title, agent.name, m.description
                )
            };
            if let Some(names) = &shared_with {
                msg.push_str(&format!(
                    "\n\nThis agent shares its folder with {names}. Their files will be within reach of \
                     what this tool sends online, and anything it brings back from the web, the others \
                     can read and act on."
                ));
            }
            let title = format!("Let “{}” use the internet for “{}”?", m.title, agent.name);
            if !dialogs::confirm(&app, window.label(), &title, &msg, "Allow", true).await {
                want.network = false;
            }
        }
        if !want.ask && m.risky && had.is_some_and(|t| t.ask) {
            let msg = format!(
                "“{}” is potentially unsafe: inside the agent's workspace one call can change or delete \
                 many files at once.\n\nWithout asking, “{}” will use it whenever the model decides to, \
                 including in scheduled jobs while you're away, and a file or web page the model reads \
                 could talk it into a harmful command.",
                m.title, agent.name
            );
            let title = format!("Stop asking before each use of “{}”?", m.title);
            if !dialogs::confirm(&app, window.label(), &title, &msg, "Stop asking", true).await {
                want.ask = true;
            }
        }
        saved.insert(name, want);
    }
    let out = saved.clone();
    state.update(&app, |s| {
        if let Some(a) = s.agents.iter_mut().find(|a| a.id == agent_id) {
            a.tools = saved;
        }
    })?;
    // Settings → Tools shows which agents have each tool on.
    super::tools::changed(&app, &state);
    Ok(out)
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
    let today = state.session.usage.all_today();
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
