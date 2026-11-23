//! Chat and loop runs.
//!
//! Sending a message runs the agent's loop with the agent's model, inside the
//! agent's workspace. Progress streams to every window as `run:*` events; the
//! reply is saved to `chats/<agent id>.json`.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use chummy_flow::{ChatTurn, Event, LoopFile, RunInputs, RunOutcome, RunStatus, analyze, run};
use chummy_log::runs::{RunEnd, RunHeader, RunLog};
use chummy_log::system::{self as syslog, Level};
use chummy_sandbox::Jail;
use chummy_scheduler::{Origin, RunKind};
use chummy_store::JsonDoc;
use serde::{Deserialize, Serialize};
use tauri::{AppHandle, Emitter, Manager, State};

use super::{CmdResult, err, now};
use crate::events;
use crate::host::{AppHost, agent_host};
use crate::settings::{Agent, Settings};
use crate::state::{AppState, RunInfo, lock};

/// Chat history kept per agent; older messages are dropped.
const MAX_CHAT_MESSAGES: usize = 2000;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct RunSummary {
    /// The run's log (Logs window, and the step view under the message).
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub run_id: String,
    pub status: String,
    pub steps: u32,
    pub llm_calls: u32,
    #[serde(default)]
    pub tool_calls: u32,
    /// Estimated tokens this turn added to context.
    #[serde(default, skip_serializing_if = "is_zero")]
    pub tokens: u64,
    /// Estimated tokens in the context at the end, and the model's context
    /// length (0: unknown).
    #[serde(default, skip_serializing_if = "is_zero")]
    pub context_tokens: u64,
    #[serde(default, skip_serializing_if = "is_zero32")]
    pub context_window: u32,
    /// The model's tokens used today when the reply came, and today's
    /// allowance (`None`: no daily limit).
    #[serde(default, skip_serializing_if = "is_zero")]
    pub daily_used: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub daily_allowed: Option<u64>,
}

fn is_zero(n: &u64) -> bool {
    *n == 0
}

fn is_zero32(n: &u32) -> bool {
    *n == 0
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ChatMessage {
    pub id: String,
    /// "user", "assistant", "notice" (errors, stops, limits), or
    /// "question" / "answer" (something a run asked, and the reply). Only
    /// "user" and "assistant" messages are the conversation the loop sees.
    pub role: String,
    pub text: String,
    pub at: u64,
    /// Who asked a question when it wasn't this chat (a job's name).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub from: Option<String>,
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

pub fn append(
    app: &AppHandle,
    state: &AppState,
    agent_id: &str,
    message: ChatMessage,
) -> CmdResult<()> {
    let doc = chat_doc(state, agent_id)?;
    let mut log = doc.load(|v| v).map_err(err)?.value;
    log.version = 1;
    log.messages.push(message.clone());
    if log.messages.len() > MAX_CHAT_MESSAGES {
        let extra = log.messages.len() - MAX_CHAT_MESSAGES;
        log.messages.drain(..extra);
    }
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

pub fn msg_id() -> String {
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
    state.asks.withdraw_run(&run_id);
}

pub fn stop_agent_runs(state: &AppState, agent_id: &str) {
    for r in lock(&state.runs).values() {
        if r.agent_id.as_deref() == Some(agent_id) {
            r.cancel.store(true, Ordering::Relaxed);
            state.asks.withdraw_run(&r.run_id);
        }
    }
}

/// Stops every run (when quitting).
pub fn stop_all_runs(state: &AppState) {
    for r in lock(&state.runs).values() {
        r.cancel.store(true, Ordering::Relaxed);
        state.asks.withdraw_run(&r.run_id);
    }
}

/// What, if anything, is using an agent right now, in words.
pub fn agent_busy_with(state: &AppState, agent_id: &str) -> Option<String> {
    lock(&state.runs)
        .values()
        .find(|r| r.agent_id.as_deref() == Some(agent_id))
        .map(|r| match r.source {
            "job" => format!("the job “{}”", r.title),
            "test" => "a test run in the loop editor".to_string(),
            _ => "the last message".to_string(),
        })
}

fn new_run_id() -> String {
    format!("run-{}", msg_id())
}

/// Everything an agent needs to run: its loop (or `loop_id` instead) and
/// its opened workspace.
pub fn prepare(
    state: &AppState,
    s: &Settings,
    agent: &Agent,
    loop_id: Option<&str>,
) -> CmdResult<(LoopFile, Jail)> {
    let id = loop_id.unwrap_or(&agent.loop_id);
    let original = state.library.load(id).map_err(|_| match loop_id {
        Some(_) => format!("The loop “{id}” is missing. Choose another for the job."),
        None => format!(
            "“{}”'s loop is missing. Choose one in Settings → Agents.",
            agent.name
        ),
    })?;
    // The agent's own block settings (Agent Loop Configuration).
    let (file, _missing) = original.with_overrides(&agent.overrides(id));
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
    if let Some(what) = agent_busy_with(&state, &agent_id) {
        return Err(format!("{} is busy with {what}.", agent.name));
    }
    let (file, jail) = prepare(&state, &s, &agent, None)?;

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
            from: None,
            run: None,
        },
    )?;

    let inputs = RunInputs {
        message: text,
        history,
        overrides: HashMap::new(),
    };
    let run_id = new_run_id();
    let origin = Origin {
        run_id: run_id.clone(),
        kind: RunKind::Chat,
        agent_id: Some(agent.id.clone()),
        job_id: None,
        title: agent.name.clone(),
    };
    let model = s.agent_model(&agent).cloned();
    let model_name = model.as_ref().map(|m| m.name.clone());
    let host = agent_host(&app, &s, Some(&agent), model, Some(jail), Some(origin));
    start(
        &app,
        &state,
        run_id.clone(),
        file,
        host,
        inputs,
        Some(agent),
        "chat",
        model_name,
    );
    Ok(run_id)
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
            if let Some(what) = agent_busy_with(&state, &id) {
                return Err(format!(
                    "{} is busy with {what}; test without an agent, or wait.",
                    agent.name
                ));
            }
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
    let run_id = new_run_id();
    let origin = Origin {
        run_id: run_id.clone(),
        kind: RunKind::Test,
        agent_id: agent.as_ref().map(|a| a.id.clone()),
        job_id: None,
        title: format!("Test: {}", file.name),
    };
    let model_name = model.as_ref().map(|m| m.name.clone());
    let host = agent_host(&app, &s, agent.as_ref(), model, jail, Some(origin));
    start(
        &app,
        &state,
        run_id.clone(),
        file,
        host,
        inputs,
        agent,
        "test",
        model_name,
    );
    Ok(run_id)
}

/// What a run log says about a run, beyond its [`RunInfo`].
pub struct RunMeta {
    pub agent_name: Option<String>,
    pub loop_name: String,
    pub model: Option<String>,
    pub message: String,
}

/// Adds a run to the run list, starts its log and tells every window. The
/// guard takes it off again however the run ends.
pub fn track(app: &AppHandle, state: &AppState, info: RunInfo, meta: RunMeta) -> RunGuard {
    lock(&state.runs).insert(info.run_id.clone(), info.clone());
    let started_at = chummy_log::now_ms();
    let opts = state.run_log_options();
    let log = opts
        .and_then(|opts| {
            RunLog::create(
                &state.run_logs,
                RunHeader {
                    run_id: info.run_id.clone(),
                    title: info.title.clone(),
                    source: info.source.into(),
                    agent_id: info.agent_id.clone(),
                    agent_name: meta.agent_name,
                    job_id: info.job_id.clone(),
                    loop_name: meta.loop_name.clone(),
                    model: meta.model.clone(),
                    message: meta.message,
                    started_at,
                },
                opts,
            )
        })
        .map(Arc::new);
    syslog::log(
        Level::Info,
        "runs",
        format!("Started a {} run: {}", info.source, info.title),
        &[
            ("run", &info.run_id),
            ("loop", &meta.loop_name),
            ("model", meta.model.as_deref().unwrap_or("none")),
        ],
    );
    let guard = RunGuard {
        app: app.clone(),
        run_id: info.run_id.clone(),
        log,
        started_at,
        title: info.title.clone(),
        agent_id: info.agent_id.clone(),
    };
    let _ = app.emit(events::RUN_STARTED, info);
    guard
}

/// Sends a run's progress to every window, and to its log.
pub fn emitter(app: &AppHandle, guard: &RunGuard) -> impl FnMut(Event) + Send + use<> {
    let app = app.clone();
    let run_id = guard.run_id.clone();
    let log = guard.log.clone();
    move |event| {
        if let Some(l) = &log {
            l.event(&event);
        }
        let _ = app.emit(
            events::RUN_EVENT,
            RunEvent {
                run_id: run_id.clone(),
                event,
            },
        );
    }
}

#[allow(clippy::too_many_arguments)]
fn start(
    app: &AppHandle,
    state: &AppState,
    run_id: String,
    file: LoopFile,
    host: AppHost,
    inputs: RunInputs,
    agent: Option<Agent>,
    source: &'static str,
    model: Option<String>,
) {
    let cancel = Arc::new(AtomicBool::new(false));
    let info = RunInfo {
        run_id: run_id.clone(),
        agent_id: agent.as_ref().map(|a| a.id.clone()),
        title: agent
            .as_ref()
            .map(|a| a.name.clone())
            .unwrap_or_else(|| format!("Test: {}", file.name)),
        source,
        job_id: None,
        started_at: now(),
        cancel: cancel.clone(),
    };
    // However the run ends (even by a panic), it leaves the run list, so
    // the agent never stays "still working".
    let meta = RunMeta {
        agent_name: agent.as_ref().map(|a| a.name.clone()),
        loop_name: file.name.clone(),
        model,
        message: inputs.message.clone(),
    };
    let guard = track(app, state, info, meta);
    let app = app.clone();
    let id = run_id;
    tauri::async_runtime::spawn(async move {
        let outcome = run(&file, &host, &inputs, &cancel, emitter(&app, &guard)).await;
        let end = guard.finish(&outcome);

        let state = app.state::<AppState>();
        lock(&state.runs).remove(&id);
        if source == "chat"
            && let Some(agent) = &agent
        {
            for m in messages_for(&outcome, &id, &end) {
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
        // A job may have been waiting for this agent.
        if let Some(jobs) = state.jobs.get() {
            jobs.wake();
        }
    });
}

pub struct RunGuard {
    app: AppHandle,
    run_id: String,
    log: Option<Arc<RunLog>>,
    started_at: u64,
    title: String,
    agent_id: Option<String>,
}

impl RunGuard {
    /// Records how the run ended, in its log and the system log, and
    /// returns that record (with today's use of the model).
    pub fn finish(&self, o: &RunOutcome) -> RunEnd {
        let mut end = RunEnd::from_outcome(o);
        let state = self.app.state::<AppState>();
        let settings = state.settings();
        // The model the turn ended on: the block's, else the agent's, else the default.
        let model = o
            .usage
            .model
            .clone()
            .or_else(|| {
                self.agent_id
                    .as_deref()
                    .and_then(|a| settings.agent(a))
                    .and_then(|a| a.model.clone())
            })
            .or_else(|| settings.default_provider.clone());
        if let Some(p) = model.as_deref().and_then(|m| settings.provider(m)) {
            (end.daily_used, end.daily_allowed) = state.usage.daily(p);
        }
        let secs = format!(
            "{:.1}",
            end.ended_at.saturating_sub(self.started_at) as f64 / 1000.0
        );
        let level = match end.status.as_str() {
            "failed" => Level::Warn,
            _ => Level::Info,
        };
        let mut fields = vec![
            ("run", self.run_id.as_str()),
            ("status", end.status.as_str()),
            ("seconds", secs.as_str()),
        ];
        if let Some(e) = &end.error {
            fields.push(("error", e.as_str()));
        }
        let steps = end.steps.to_string();
        let calls = end.llm_calls.to_string();
        fields.push(("steps", &steps));
        fields.push(("model calls", &calls));
        syslog::log(
            level,
            "runs",
            format!(
                "{} {}",
                self.title,
                match end.status.as_str() {
                    "completed" => "finished",
                    "stopped" => "was stopped",
                    "failed" => "failed",
                    "limit" => "hit a limit",
                    _ => "ended",
                }
            ),
            &fields,
        );
        if let Some(l) = &self.log {
            l.end(end.clone());
        }
        end
    }
}

impl Drop for RunGuard {
    fn drop(&mut self) {
        let state = self.app.state::<AppState>();
        lock(&state.runs).remove(&self.run_id);
        state.asks.withdraw_run(&self.run_id);
    }
}

/// Chat messages that report a finished run.
fn messages_for(o: &RunOutcome, run_id: &str, end: &RunEnd) -> Vec<ChatMessage> {
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
        run_id: run_id.into(),
        status: status.into(),
        steps: o.steps,
        llm_calls: o.llm_calls,
        tool_calls: o.tool_calls,
        tokens: end.tokens_added,
        context_tokens: end.context_tokens,
        context_window: end.context_window,
        daily_used: end.daily_used,
        daily_allowed: end.daily_allowed,
    };
    let mut out = Vec::new();
    if !o.replies.is_empty() {
        out.push(ChatMessage {
            id: msg_id(),
            role: "assistant".into(),
            text: o.replies.join("\n\n"),
            at: now(),
            from: None,
            run: Some(summary.clone()),
        });
    }
    if let Some(text) = notice {
        out.push(ChatMessage {
            id: msg_id(),
            role: "notice".into(),
            text,
            at: now(),
            from: None,
            run: Some(summary),
        });
    }
    out
}
