//! Paused chat runs.
//!
//! Pausing a chat run (or a model it couldn't reach) ends the run with its
//! whole state, which is kept here until the chat resumes or discards it:
//! sealed in the Chummy folder with a key only the OS keychain holds (see
//! [`chummy_store::paused`]), so it survives quitting and restarting, and
//! nothing Chummy didn't seal itself can ever be resumed. Without a
//! keychain, a paused run is held in memory only, until Chummy quits.
//!
//! A paused run carries on with the loop exactly as it ran (with the
//! agent's settings at the time), in the same workspace, and with the
//! access the agent and Chummy allow when it resumes.

use std::collections::HashMap;
use std::path::PathBuf;

use chummy_flow::{LoopFile, RunInputs, RunState};
use chummy_log::system::{self as syslog, Level};
use chummy_store::paused::PauseError;
use serde::{Deserialize, Serialize};
use tauri::{AppHandle, Emitter};

use crate::events;
use crate::state::{AppState, lock};

const FORMAT: &str = "chummy.paused-run";
const VERSION: u32 = 1;

/// What a paused chat run keeps, sealed.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PausedRun {
    pub format: String,
    pub version: u32,
    pub agent_id: String,
    pub chat_id: String,
    pub chat_title: String,
    /// The agent's workspace when the run started; it carries on only there.
    pub workspace: PathBuf,
    /// The loop as it ran, with the agent's block settings applied.
    pub file: LoopFile,
    pub inputs: RunInputs,
    pub state: RunState,
    pub paused_at: u64,
    /// Why it paused itself (a model it couldn't reach); `None`: asked to.
    pub reason: Option<String>,
    /// The chat's first message, not answered yet (the chat gets a name once
    /// it is).
    pub first: bool,
    /// The model's name when it paused, for its run log.
    pub model: Option<String>,
}

impl PausedRun {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        agent_id: String,
        chat_id: String,
        chat_title: String,
        workspace: PathBuf,
        file: LoopFile,
        inputs: RunInputs,
        state: RunState,
        reason: Option<String>,
        first: bool,
        model: Option<String>,
    ) -> Self {
        Self {
            format: FORMAT.into(),
            version: VERSION,
            agent_id,
            chat_id,
            chat_title,
            workspace,
            file,
            inputs,
            state,
            paused_at: crate::commands::now(),
            reason,
            first,
            model,
        }
    }
}

/// A paused run, as windows see it.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct PausedInfo {
    pub agent_id: String,
    pub chat_id: String,
    pub chat_title: String,
    pub paused_at: u64,
    pub reason: Option<String>,
    /// Kept in the Chummy folder (so it survives quitting); `false`: held
    /// in memory only, because there's no keychain to seal it with.
    pub saved: bool,
}

/// A paused run Chummy knows of.
pub struct Entry {
    pub info: PausedInfo,
    /// The run itself when it's held in memory only.
    held: Option<Box<PausedRun>>,
}

pub type Paused = HashMap<(String, String), Entry>;

fn info(run: &PausedRun, saved: bool) -> PausedInfo {
    PausedInfo {
        agent_id: run.agent_id.clone(),
        chat_id: run.chat_id.clone(),
        chat_title: run.chat_title.clone(),
        paused_at: run.paused_at,
        reason: run.reason.clone(),
        saved,
    }
}

/// Reads a sealed run, which must be for this agent and chat.
fn decode(bytes: &[u8], agent: &str, chat: &str) -> Result<PausedRun, String> {
    let run: PausedRun = serde_json::from_slice(bytes)
        .map_err(|_| "it was saved by another version of Chummy".to_string())?;
    if run.format != FORMAT || run.version != VERSION {
        return Err("it was saved by another version of Chummy".into());
    }
    if run.agent_id != agent || run.chat_id != chat {
        return Err("it belongs to another chat".into());
    }
    Ok(run)
}

/// Tells a chat its paused run was removed, so the chat doesn't still say
/// it can be resumed. (Nothing happens if there's no such chat.)
fn note_removed(state: &AppState, agent: &str, chat: &str, why: &str) {
    let _ = crate::chats::append(
        &state.dirs.chats,
        agent,
        chat,
        &crate::chats::ChatMessage {
            id: chummy_store::random_id(6),
            role: "notice".into(),
            text: format!(
                "The paused run couldn't be opened ({why}), so it was removed. Send the message again to start over."
            ),
            at: crate::commands::now(),
            from: None,
            run: None,
        },
    );
}

/// Finds the paused runs kept in the Chummy folder (at startup). Files that
/// don't open, or whose agent is gone, are removed.
pub fn load(state: &AppState) {
    let settings = state.settings();
    let mut found = Vec::new();
    for (agent, chat) in state.vault.list() {
        if settings.agent(&agent).is_none() {
            let _ = state.vault.discard(state.secrets.as_ref(), &agent, &chat);
            continue;
        }
        match state.vault.open(state.secrets.as_ref(), &agent, &chat) {
            Ok(Some(bytes)) => match decode(&bytes, &agent, &chat) {
                Ok(run) => found.push(info(&run, true)),
                Err(why) => {
                    let _ = state.vault.discard(state.secrets.as_ref(), &agent, &chat);
                    note_removed(state, &agent, &chat, &why);
                    syslog::log(
                        Level::Warn,
                        "runs",
                        format!("Removed a paused run that couldn't be resumed: {why}"),
                        &[("agent", &agent), ("chat", &chat)],
                    );
                }
            },
            Ok(None) => {}
            Err(PauseError::Untrusted(why)) => {
                note_removed(state, &agent, &chat, &why);
                syslog::log(
                    Level::Warn,
                    "runs",
                    format!("Removed a paused run Chummy didn't seal: {why}"),
                    &[("agent", &agent), ("chat", &chat)],
                );
            }
            // No keychain right now: left alone, for a later start.
            Err(e) => syslog::log(
                Level::Warn,
                "runs",
                format!("A paused run can't be opened right now: {e}"),
                &[("agent", &agent), ("chat", &chat)],
            ),
        }
    }
    let mut paused = lock(&state.paused);
    for i in found {
        paused.insert(
            (i.agent_id.clone(), i.chat_id.clone()),
            Entry {
                info: i,
                held: None,
            },
        );
    }
}

/// Every paused run, oldest first.
pub fn list(state: &AppState) -> Vec<PausedInfo> {
    let mut l: Vec<PausedInfo> = lock(&state.paused)
        .values()
        .map(|e| e.info.clone())
        .collect();
    l.sort_by_key(|i| i.paused_at);
    l
}

fn changed(app: &AppHandle, state: &AppState) {
    let _ = app.emit(events::PAUSED_CHANGED, list(state));
}

pub fn is_paused(state: &AppState, agent: &str, chat: &str) -> bool {
    lock(&state.paused).contains_key(&(agent.to_string(), chat.to_string()))
}

/// Keeps a paused run: sealed in the Chummy folder, or in memory when there
/// is no keychain. Returns whether it was saved.
pub fn keep(app: &AppHandle, state: &AppState, run: PausedRun) -> bool {
    let key = (run.agent_id.clone(), run.chat_id.clone());
    let sealed = serde_json::to_vec(&run)
        .map_err(|e| e.to_string())
        .and_then(|bytes| {
            let bytes = zeroize::Zeroizing::new(bytes);
            state
                .vault
                .seal(state.secrets.as_ref(), &run.agent_id, &run.chat_id, &bytes)
                .map_err(|e| e.to_string())
        });
    let entry = match sealed {
        Ok(()) => Entry {
            info: info(&run, true),
            held: None,
        },
        Err(e) => {
            syslog::log(
                Level::Warn,
                "runs",
                format!("A paused run is kept in memory only: {e}"),
                &[("agent", &run.agent_id), ("chat", &run.chat_id)],
            );
            Entry {
                info: info(&run, false),
                held: Some(Box::new(run)),
            }
        }
    };
    let saved = entry.held.is_none();
    lock(&state.paused).insert(key, entry);
    changed(app, state);
    saved
}

/// A chat's paused run, left where it is (to check it can carry on).
pub fn peek(state: &AppState, agent: &str, chat: &str) -> Result<PausedRun, String> {
    let key = (agent.to_string(), chat.to_string());
    if let Some(run) = lock(&state.paused)
        .get(&key)
        .and_then(|e| e.held.as_deref())
    {
        return Ok(run.clone());
    }
    if !lock(&state.paused).contains_key(&key) {
        return Err("This chat has no paused run.".into());
    }
    match state.vault.open(state.secrets.as_ref(), agent, chat) {
        Ok(Some(bytes)) => decode(&bytes, agent, chat).map_err(|why| {
            format!(
                "The paused run can't be resumed: {why}. Discard it and send the message again."
            )
        }),
        Ok(None) => Err("The paused run is gone. Send the message again.".into()),
        Err(e) => Err(format!("The paused run can't be opened: {e}.")),
    }
}

/// Takes a chat's paused run to resume it: it's removed (its key first), so
/// it can only be resumed this once.
pub fn take(
    app: &AppHandle,
    state: &AppState,
    agent: &str,
    chat: &str,
) -> Result<PausedRun, String> {
    let key = (agent.to_string(), chat.to_string());
    let held = {
        let mut paused = lock(&state.paused);
        match paused.get(&key) {
            None => return Err("This chat has no paused run.".into()),
            Some(e) if e.held.is_some() => paused.remove(&key).and_then(|e| e.held),
            Some(_) => None,
        }
    };
    let run = match held {
        Some(run) => *run,
        None => {
            let bytes = state
                .vault
                .take(state.secrets.as_ref(), agent, chat)
                .map_err(|e| format!("The paused run can't be opened: {e}."))?
                .ok_or("The paused run is gone. Send the message again.")?;
            lock(&state.paused).remove(&key);
            decode(&bytes, agent, chat)?
        }
    };
    changed(app, state);
    Ok(run)
}

/// Removes a chat's paused run, if it has one.
pub fn discard(app: &AppHandle, state: &AppState, agent: &str, chat: &str) -> Result<(), String> {
    let key = (agent.to_string(), chat.to_string());
    let Some(entry) = lock(&state.paused).remove(&key) else {
        return Ok(());
    };
    if entry.held.is_none()
        && let Err(e) = state.vault.discard(state.secrets.as_ref(), agent, chat)
    {
        // Still listed, so it can be discarded again.
        lock(&state.paused).insert(key, entry);
        changed(app, state);
        return Err(format!("The paused run couldn't be removed: {e}"));
    }
    changed(app, state);
    Ok(())
}

/// Removes every paused run of an agent (when it's deleted).
pub fn discard_agent(app: &AppHandle, state: &AppState, agent: &str) {
    lock(&state.paused).retain(|(a, _), _| a != agent);
    state.vault.discard_agent(state.secrets.as_ref(), agent);
    changed(app, state);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_paused_run_is_only_read_back_for_its_own_chat() {
        let run = PausedRun::new(
            "agent1".into(),
            "chat1".into(),
            "Notes".into(),
            PathBuf::from("/work"),
            LoopFile::new("loop"),
            RunInputs::default(),
            serde_json::from_value(serde_json::json!({
                "version": 1, "frames": [], "replies": [], "steps": 0, "llm_calls": 0,
                "tool_calls": 0, "noticed": [], "memory": {}, "outlines": {},
                "usage": { "tokens_added": 0, "context_tokens": 0, "context_window": 0, "model": null }
            }))
            .unwrap(),
            None,
            true,
            None,
        );
        let bytes = serde_json::to_vec(&run).unwrap();
        assert_eq!(decode(&bytes, "agent1", "chat1").unwrap(), run);
        assert!(decode(&bytes, "agent1", "chat2").is_err());
        assert!(decode(&bytes, "agent2", "chat1").is_err());
        let mut v: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
        v["version"] = 2.into();
        assert!(decode(&serde_json::to_vec(&v).unwrap(), "agent1", "chat1").is_err());
        v["version"] = 1.into();
        v["extra"] = true.into();
        assert!(decode(&serde_json::to_vec(&v).unwrap(), "agent1", "chat1").is_err());
    }
}
