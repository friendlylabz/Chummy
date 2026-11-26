//! Paused chat runs.
//!
//! Pausing a chat run (or a model it couldn't reach) ends the run with its
//! whole state, which is kept here until the chat resumes or discards it:
//! sealed by the program's [`PausedStore`] (the app: in the Chummy folder
//! with a key only the OS keychain holds, see [`chummy_store::paused`]), so
//! it survives quitting and restarting, and nothing the program didn't seal
//! itself can ever be resumed. If it can't be sealed (no keychain), it's
//! held in memory only, until the program quits.
//!
//! A paused run carries on with the loop exactly as it ran (with the
//! agent's settings at the time), in the same workspace, and with the
//! access the agent and Chummy allow when it resumes.

use std::collections::HashMap;
use std::path::PathBuf;

use chummy_agent::AgentConfig;
use chummy_flow::{LoopFile, RunInputs, RunState};
use chummy_log::system::{self as syslog, Level};
use serde::{Deserialize, Serialize};

use crate::chats::ChatMessage;
use crate::platform::{Platform, Update};
use crate::store::PausedOpenError;
use crate::{Session, lock, now};

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
    /// The run's id: it carries on as the same run (one response, one log).
    #[serde(default)]
    pub run_id: String,
    /// How long it had worked before pausing, in ms.
    #[serde(default)]
    pub elapsed_ms: u64,
    /// The chat's "Paused" notes, removed when it carries on.
    #[serde(default)]
    pub notices: Vec<String>,
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
            paused_at: now(),
            reason,
            first,
            model,
            run_id: String::new(),
            elapsed_ms: 0,
            notices: vec![],
        }
    }
}

/// A paused run, as windows see it.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct PausedInfo {
    pub agent_id: String,
    pub chat_id: String,
    /// The run it carries on as (its response in the chat).
    pub run_id: String,
    pub chat_title: String,
    pub paused_at: u64,
    pub reason: Option<String>,
    /// Kept in the Chummy folder (so it survives quitting); `false`: held
    /// in memory only, because there's no keychain to seal it with.
    pub saved: bool,
}

/// A paused run the session knows of.
pub(crate) struct Entry {
    pub info: PausedInfo,
    /// The run itself when it's held in memory only.
    held: Option<Box<PausedRun>>,
}

pub(crate) type Paused = HashMap<(String, String), Entry>;

fn info(run: &PausedRun, saved: bool) -> PausedInfo {
    PausedInfo {
        agent_id: run.agent_id.clone(),
        chat_id: run.chat_id.clone(),
        run_id: run.run_id.clone(),
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
fn note_removed<P: Platform>(p: &P, agent: &str, chat: &str, why: &str) {
    let _ = p.chats().append(
        agent,
        chat,
        &ChatMessage {
            id: chummy_store::random_id(6),
            role: "notice".into(),
            text: format!(
                "The paused run couldn't be opened ({why}), so it was removed. Send the message again to start over."
            ),
            at: now(),
            from: None,
            run: None,
        },
    );
}

/// Finds the paused runs kept by the store (at startup). Ones that don't
/// open, or whose agent is gone, are removed.
pub fn load<P: Platform>(p: &P) {
    let config = p.config();
    let store = p.paused_store();
    let mut found = Vec::new();
    for (agent, chat) in store.list() {
        if config.agent(&agent).is_none() {
            let _ = store.discard(&agent, &chat);
            continue;
        }
        match store.open(&agent, &chat) {
            Ok(Some(bytes)) => match decode(&bytes, &agent, &chat) {
                Ok(run) => found.push(info(&run, true)),
                Err(why) => {
                    let _ = store.discard(&agent, &chat);
                    note_removed(p, &agent, &chat, &why);
                    syslog::log(
                        Level::Warn,
                        "runs",
                        format!("Removed a paused run that couldn't be resumed: {why}"),
                        &[("agent", &agent), ("chat", &chat)],
                    );
                }
            },
            Ok(None) => {}
            Err(PausedOpenError::Untrusted(why)) => {
                note_removed(p, &agent, &chat, &why);
                syslog::log(
                    Level::Warn,
                    "runs",
                    format!("Removed a paused run Chummy didn't seal: {why}"),
                    &[("agent", &agent), ("chat", &chat)],
                );
            }
            // Can't be checked right now: left alone, for a later start.
            Err(e) => syslog::log(
                Level::Warn,
                "runs",
                format!("A paused run can't be opened right now: {e}"),
                &[("agent", &agent), ("chat", &chat)],
            ),
        }
    }
    let mut paused = lock(&p.session().paused);
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

impl Session {
    /// Every paused run, oldest first.
    pub fn paused(&self) -> Vec<PausedInfo> {
        let mut l: Vec<PausedInfo> = lock(&self.paused)
            .values()
            .map(|e| e.info.clone())
            .collect();
        l.sort_by_key(|i| i.paused_at);
        l
    }

    pub fn is_paused(&self, agent: &str, chat: &str) -> bool {
        lock(&self.paused).contains_key(&(agent.to_string(), chat.to_string()))
    }
}

fn changed<P: Platform>(p: &P) {
    p.emit(Update::PausedChanged(p.session().paused()));
}

/// Keeps a paused run: sealed by the store, or in memory if it can't be.
/// Returns whether it was saved.
pub fn keep<P: Platform>(p: &P, run: PausedRun) -> bool {
    let key = (run.agent_id.clone(), run.chat_id.clone());
    let sealed = serde_json::to_vec(&run)
        .map_err(|e| e.to_string())
        .and_then(|bytes| {
            let bytes = zeroize::Zeroizing::new(bytes);
            p.paused_store().seal(&run.agent_id, &run.chat_id, &bytes)
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
    lock(&p.session().paused).insert(key, entry);
    changed(p);
    saved
}

/// A chat's paused run, left where it is (to check it can carry on).
pub fn peek<P: Platform>(p: &P, agent: &str, chat: &str) -> Result<PausedRun, String> {
    let key = (agent.to_string(), chat.to_string());
    {
        let paused = lock(&p.session().paused);
        match paused.get(&key) {
            None => return Err("This chat has no paused run.".into()),
            Some(e) => {
                if let Some(run) = e.held.as_deref() {
                    return Ok(run.clone());
                }
            }
        }
    }
    match p.paused_store().open(agent, chat) {
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
pub fn take<P: Platform>(p: &P, agent: &str, chat: &str) -> Result<PausedRun, String> {
    let key = (agent.to_string(), chat.to_string());
    let held = {
        let mut paused = lock(&p.session().paused);
        match paused.get(&key) {
            None => return Err("This chat has no paused run.".into()),
            Some(e) if e.held.is_some() => paused.remove(&key).and_then(|e| e.held),
            Some(_) => None,
        }
    };
    let run = match held {
        Some(run) => *run,
        None => {
            let bytes = p
                .paused_store()
                .take(agent, chat)
                .map_err(|e| format!("The paused run can't be opened: {e}."))?
                .ok_or("The paused run is gone. Send the message again.")?;
            lock(&p.session().paused).remove(&key);
            decode(&bytes, agent, chat)?
        }
    };
    changed(p);
    Ok(run)
}

/// Removes a chat's paused run, if it has one.
pub fn discard<P: Platform>(p: &P, agent: &str, chat: &str) -> Result<(), String> {
    let key = (agent.to_string(), chat.to_string());
    let Some(entry) = lock(&p.session().paused).remove(&key) else {
        return Ok(());
    };
    if entry.held.is_none()
        && let Err(e) = p.paused_store().discard(agent, chat)
    {
        // Still listed, so it can be discarded again.
        lock(&p.session().paused).insert(key, entry);
        changed(p);
        return Err(format!("The paused run couldn't be removed: {e}"));
    }
    changed(p);
    Ok(())
}

/// Removes every paused run of an agent (when it's deleted).
pub fn discard_agent<P: Platform>(p: &P, agent: &str) {
    lock(&p.session().paused).retain(|(a, _), _| a != agent);
    p.paused_store().discard_agent(agent);
    changed(p);
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
