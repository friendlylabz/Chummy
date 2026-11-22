//! Chat and loop runs.
//!
//! Sending a message runs the agent's loop with the agent's model, inside the
//! agent's workspace. Progress streams to every window as `run:*` events; the
//! reply is saved to `chats/<agent id>.json`.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use chummy_flow::{
    ChatTurn, Event, Host, LlmCall, LoopFile, RunInputs, RunOutcome, RunStatus, analyze, run,
};
use chummy_llm::{ChatEvent, ChatRequest, Client, Message, ProviderConfig, ProviderKind};
use chummy_sandbox::Jail;
use chummy_store::JsonDoc;
use serde::{Deserialize, Serialize};
use tauri::{AppHandle, Emitter, Manager, State};

use super::{CmdResult, err, now};
use crate::commands::local::start_engine_for;
use crate::events;
use crate::settings::{Agent, Settings};
use crate::state::{AppState, RunInfo, lock};

/// Largest file a Read file block will read.
const READ_LIMIT: u64 = 1024 * 1024;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct RunSummary {
    pub status: String,
    pub steps: u32,
    pub llm_calls: u32,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ChatMessage {
    pub id: String,
    /// "user", "assistant" or "notice" (errors, stops, limits).
    pub role: String,
    pub text: String,
    pub at: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub run: Option<RunSummary>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct ChatLog {
    pub version: u32,
    pub messages: Vec<ChatMessage>,
}

#[derive(Clone, Serialize)]
pub struct ChatMessageEvent {
    pub agent_id: String,
    pub message: ChatMessage,
}

#[derive(Clone, Serialize)]
pub struct RunEvent {
    pub run_id: String,
    pub event: Event,
}

#[derive(Clone, Serialize)]
pub struct RunFinished {
    pub run_id: String,
    pub agent_id: Option<String>,
    pub outcome: RunOutcome,
}

pub fn chat_path(state: &AppState, agent_id: &str) -> CmdResult<PathBuf> {
    if !crate::library::valid_id(agent_id) {
        return Err("Invalid agent id.".into());
    }
    Ok(state.dirs.chats.join(format!("{agent_id}.json")))
}

fn chat_doc(state: &AppState, agent_id: &str) -> CmdResult<JsonDoc<ChatLog>> {
    Ok(JsonDoc::new(chat_path(state, agent_id)?))
}

fn load_chat(state: &AppState, agent_id: &str) -> CmdResult<ChatLog> {
    Ok(chat_doc(state, agent_id)?.load(|v| v).map_err(err)?.value)
}

fn append(
    app: &AppHandle,
    state: &AppState,
    agent_id: &str,
    message: ChatMessage,
) -> CmdResult<()> {
    let doc = chat_doc(state, agent_id)?;
    let mut log = doc.load(|v| v).map_err(err)?.value;
    log.version = 1;
    log.messages.push(message.clone());
    doc.save(&log).map_err(err)?;
    let _ = app.emit(
        events::CHAT_MESSAGE,
        ChatMessageEvent {
            agent_id: agent_id.into(),
            message,
        },
    );
    Ok(())
}

fn msg_id() -> String {
    let mut b = [0u8; 6];
    let _ = getrandom::fill(&mut b);
    b.iter().map(|x| format!("{x:02x}")).collect()
}

#[tauri::command]
pub fn get_chat(state: State<'_, AppState>, agent_id: String) -> CmdResult<Vec<ChatMessage>> {
    Ok(load_chat(&state, &agent_id)?.messages)
}

#[tauri::command]
pub fn clear_chat(app: AppHandle, state: State<'_, AppState>, agent_id: String) -> CmdResult<()> {
    let path = chat_path(&state, &agent_id)?;
    let _ = std::fs::remove_file(&path);
    let _ = std::fs::remove_file(path.with_extension("json.bak"));
    let _ = app.emit(
        events::CHAT_MESSAGE,
        serde_json::json!({ "agent_id": agent_id, "cleared": true }),
    );
    Ok(())
}

#[tauri::command]
pub fn list_runs(state: State<'_, AppState>) -> Vec<RunInfo> {
    lock(&state.runs).values().cloned().collect()
}

#[tauri::command]
pub fn stop_run(state: State<'_, AppState>, run_id: String) {
    if let Some(r) = lock(&state.runs).get(&run_id) {
        r.cancel.store(true, Ordering::Relaxed);
    }
}

pub fn stop_agent_runs(state: &AppState, agent_id: &str) {
    for r in lock(&state.runs).values() {
        if r.agent_id.as_deref() == Some(agent_id) {
            r.cancel.store(true, Ordering::Relaxed);
        }
    }
}

/// Everything an agent needs to run: its loop and its opened workspace.
fn prepare(state: &AppState, s: &Settings, agent: &Agent) -> CmdResult<(LoopFile, Jail)> {
    let file = state.library.load(&agent.loop_id).map_err(|_| {
        format!(
            "“{}”'s loop is missing. Choose one in Settings → Agents.",
            agent.name
        )
    })?;
    let analysis = analyze(&file);
    if let Some(e) = analysis
        .issues
        .iter()
        .find(|i| i.severity == chummy_flow::Severity::Error)
    {
        return Err(format!(
            "The loop “{}” has a problem: {} Fix it in the loop editor.",
            file.name, e.message
        ));
    }
    let jail = Jail::open(&agent.workspace.path, &state.protected).map_err(|e| {
        format!(
            "“{}”'s workspace can't be used: {e}. Choose its folder again in Settings → Agents.",
            agent.name
        )
    })?;
    if s.agent_model(agent).is_none() {
        return Err("No model is connected. Add one in Settings → Models.".into());
    }
    Ok((file, jail))
}

/// Sends a chat message to an agent and starts its loop. Returns the run id.
#[tauri::command]
pub fn send_message(
    app: AppHandle,
    state: State<'_, AppState>,
    agent_id: String,
    text: String,
) -> CmdResult<String> {
    let text = text.trim().to_string();
    if text.is_empty() {
        return Err("Type a message first.".into());
    }
    let s = state.settings();
    let agent = s
        .agent(&agent_id)
        .cloned()
        .ok_or("That agent no longer exists.")?;
    if lock(&state.runs)
        .values()
        .any(|r| r.agent_id.as_deref() == Some(agent_id.as_str()) && r.source == "chat")
    {
        return Err(format!(
            "{} is still working on the last message.",
            agent.name
        ));
    }
    let (file, jail) = prepare(&state, &s, &agent)?;

    let history: Vec<ChatTurn> = load_chat(&state, &agent_id)?
        .messages
        .into_iter()
        .filter(|m| m.role == "user" || m.role == "assistant")
        .map(|m| ChatTurn {
            role: m.role,
            text: m.text,
        })
        .collect();
    append(
        &app,
        &state,
        &agent_id,
        ChatMessage {
            id: msg_id(),
            role: "user".into(),
            text: text.clone(),
            at: now(),
            run: None,
        },
    )?;

    let inputs = RunInputs {
        message: text,
        history,
        overrides: HashMap::new(),
    };
    let host = AppHost::new(&app, s.clone(), s.agent_model(&agent).cloned(), Some(jail));
    Ok(start(&app, &state, file, host, inputs, Some(agent), "chat"))
}

/// Runs a loop from the editor without saving anything to chat. With an
/// agent, the run uses its model and workspace; without one, the default
/// model and no workspace (file blocks will report an error).
#[tauri::command]
pub fn test_run(
    app: AppHandle,
    state: State<'_, AppState>,
    file: LoopFile,
    agent_id: Option<String>,
    message: String,
    overrides: HashMap<String, String>,
) -> CmdResult<String> {
    let s = state.settings();
    let (agent, jail, model) = match agent_id.filter(|a| !a.is_empty()) {
        Some(id) => {
            let agent = s
                .agent(&id)
                .cloned()
                .ok_or("That agent no longer exists.")?;
            let jail = Jail::open(&agent.workspace.path, &state.protected).map_err(err)?;
            let model = s.agent_model(&agent).cloned();
            (Some(agent), Some(jail), model)
        }
        None => (None, None, s.default_model().cloned()),
    };
    if model.is_none() {
        return Err("No model is connected. Add one in Settings → Models.".into());
    }
    let inputs = RunInputs {
        message,
        history: vec![],
        overrides,
    };
    let host = AppHost::new(&app, s, model, jail);
    Ok(start(&app, &state, file, host, inputs, agent, "test"))
}

fn start(
    app: &AppHandle,
    state: &AppState,
    file: LoopFile,
    host: AppHost,
    inputs: RunInputs,
    agent: Option<Agent>,
    source: &'static str,
) -> String {
    let run_id = format!("run-{}", msg_id());
    let cancel = Arc::new(AtomicBool::new(false));
    let info = RunInfo {
        run_id: run_id.clone(),
        agent_id: agent.as_ref().map(|a| a.id.clone()),
        title: agent
            .as_ref()
            .map(|a| a.name.clone())
            .unwrap_or_else(|| format!("Test: {}", file.name)),
        source,
        started_at: now(),
        cancel: cancel.clone(),
    };
    lock(&state.runs).insert(run_id.clone(), info.clone());
    let _ = app.emit(events::RUN_STARTED, info);

    let app = app.clone();
    let id = run_id.clone();
    tauri::async_runtime::spawn(async move {
        let emitter = app.clone();
        let rid = id.clone();
        let outcome = run(&file, &host, &inputs, &cancel, move |event| {
            let _ = emitter.emit(
                events::RUN_EVENT,
                RunEvent {
                    run_id: rid.clone(),
                    event,
                },
            );
        })
        .await;

        let state = app.state::<AppState>();
        lock(&state.runs).remove(&id);
        if source == "chat"
            && let Some(agent) = &agent
        {
            for m in messages_for(&outcome) {
                let _ = append(&app, &state, &agent.id, m);
            }
        }
        let _ = app.emit(
            events::RUN_FINISHED,
            RunFinished {
                run_id: id,
                agent_id: agent.map(|a| a.id),
                outcome,
            },
        );
    });
    run_id
}

/// Chat messages that report a finished run.
fn messages_for(o: &RunOutcome) -> Vec<ChatMessage> {
    let (status, notice) = match &o.status {
        RunStatus::Completed if o.replies.is_empty() => (
            "completed",
            Some(
                "The loop finished without replying. Does it have a Reply block that gets input?"
                    .to_string(),
            ),
        ),
        RunStatus::Completed => ("completed", None),
        RunStatus::Stopped => ("stopped", Some("Stopped.".to_string())),
        RunStatus::LimitReached { message } => ("limit", Some(message.clone())),
        RunStatus::Failed { error, .. } => ("failed", Some(error.clone())),
    };
    let summary = RunSummary {
        status: status.into(),
        steps: o.steps,
        llm_calls: o.llm_calls,
    };
    let mut out = Vec::new();
    if !o.replies.is_empty() {
        out.push(ChatMessage {
            id: msg_id(),
            role: "assistant".into(),
            text: o.replies.join("\n\n"),
            at: now(),
            run: Some(summary.clone()),
        });
    }
    if let Some(text) = notice {
        out.push(ChatMessage {
            id: msg_id(),
            role: "notice".into(),
            text,
            at: now(),
            run: Some(summary),
        });
    }
    out
}

/// Connects the engine to models and to the agent's workspace.
pub struct AppHost {
    app: AppHandle,
    settings: Settings,
    default_model: Option<ProviderConfig>,
    jail: Option<Jail>,
    clients: tokio::sync::Mutex<HashMap<String, Client>>,
}

impl AppHost {
    fn new(
        app: &AppHandle,
        settings: Settings,
        default_model: Option<ProviderConfig>,
        jail: Option<Jail>,
    ) -> Self {
        Self {
            app: app.clone(),
            settings,
            default_model,
            jail,
            clients: Default::default(),
        }
    }

    async fn client(&self, p: &ProviderConfig) -> Result<Client, String> {
        if let Some(c) = self.clients.lock().await.get(&p.id) {
            return Ok(c.clone());
        }
        let state = self.app.state::<AppState>();
        let client = if p.kind == ProviderKind::Local {
            if chummy_local::runtime::status(&state.runtime).is_none() {
                return Err(
                    "The local engine isn't set up yet. Set it up in Settings → Local engine."
                        .into(),
                );
            }
            let ep = start_engine_for(&self.app, &state, p).await?;
            let mut cfg = p.clone();
            cfg.base_url = ep.base_url;
            Client::new(&cfg, Some(ep.token)).map_err(err)?
        } else {
            let key = state.secrets.get(&p.secret_key()).ok().flatten();
            Client::new(p, key).map_err(err)?
        };
        self.clients
            .lock()
            .await
            .insert(p.id.clone(), client.clone());
        Ok(client)
    }

    fn jail(&self) -> Result<&Jail, String> {
        self.jail
            .as_ref()
            .ok_or_else(|| "This test run has no workspace. Pick an agent to run as.".to_string())
    }
}

impl Host for AppHost {
    async fn llm(
        &self,
        call: LlmCall,
        on_text: &mut (dyn FnMut(&str) + Send),
    ) -> Result<String, String> {
        let provider = match &call.model {
            Some(id) => self.settings.provider(id).cloned().ok_or(
                "This block's model connection no longer exists. Pick another in the loop editor.",
            )?,
            None => self
                .default_model
                .clone()
                .ok_or("No model is connected. Add one in Settings → Models.")?,
        };
        let client = self.client(&provider).await?;
        let mut messages = Vec::new();
        if let Some(s) = call.system {
            messages.push(Message::system(s));
        }
        messages.push(Message::user(call.prompt));
        let req = ChatRequest {
            messages,
            tools: vec![],
            temperature: call.temperature,
            max_tokens: Some(call.max_tokens),
        };
        let resp = client
            .chat(&req, |ev| {
                if let ChatEvent::Text { text } = ev {
                    on_text(text);
                }
            })
            .await
            .map_err(|e| format!("{}: {e}", provider.name))?;
        Ok(resp.text)
    }

    fn read_file(&self, path: &str) -> Result<String, String> {
        self.jail()?.read_text(path, READ_LIMIT).map_err(err)
    }

    fn write_file(&self, path: &str, content: &str, append: bool) -> Result<(), String> {
        let jail = self.jail()?;
        let data = if append && jail.exists(path).map_err(err)? {
            let mut existing = jail.read_bytes(path, 64 * READ_LIMIT).map_err(err)?;
            existing.extend_from_slice(content.as_bytes());
            existing
        } else {
            content.as_bytes().to_vec()
        };
        jail.write(path, &data).map_err(err)
    }

    fn list_files(&self, path: &str) -> Result<Vec<String>, String> {
        Ok(self
            .jail()?
            .list(path)
            .map_err(err)?
            .into_iter()
            .map(|e| {
                if e.is_dir {
                    format!("{}/", e.name)
                } else {
                    e.name
                }
            })
            .collect())
    }
}
