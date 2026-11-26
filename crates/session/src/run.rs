//! Starting runs and seeing them through: a chat message, a test run, a
//! paused run carrying on, a job. Progress goes out as [`Update`]s; a chat
//! run's response is saved to its chat as one response when it ends (or
//! pauses), and a resumed run carries on that same response.

use std::collections::HashMap;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use chummy_agent::{Agent, AgentConfig};
use chummy_flow::{
    Answer, ChatTurn, Event, LoopFile, RunInputs, RunOutcome, RunState, RunStatus, analyze,
    run_with,
};
use chummy_llm::ProviderConfig;
use chummy_log::runs::{RunEnd, RunHeader, RunLog};
use chummy_log::system::{self as syslog, Level};
use chummy_runner::{AgentHost, AgentSetup};
use chummy_sandbox::Jail;
use chummy_scheduler::{Origin, RunKind, RunRequest, RunResult, Snapshot};

use crate::chats::{self, ChatIndex, ChatMessage, RunSummary};
use crate::paused::{self, PausedRun};
use crate::platform::{
    ChatMessageDeleted, ChatMessageEvent, ChatRunUpdated, ChatsChanged, FirstReply, LoopProblem,
    Platform, RunEvent, RunFinished, RunPausing, RunSaid, Update,
};
use crate::runs::{PANE_LINES, RunInfo, Said};
use crate::{lock, now};

pub fn msg_id() -> String {
    chummy_store::random_id(6)
}

const SHUTTING_DOWN: &str = "Chummy is shutting down.";

fn new_run_id() -> String {
    format!("run-{}", msg_id())
}

/// Tells watchers an agent's chats changed.
pub fn emit_index<P: Platform>(p: &P, agent_id: &str, index: ChatIndex) {
    p.emit(Update::ChatsChanged(ChatsChanged {
        agent_id: agent_id.into(),
        index,
    }));
}

/// Adds a message to one of an agent's chats and tells watchers.
pub fn append<P: Platform>(
    p: &P,
    agent_id: &str,
    chat_id: &str,
    message: ChatMessage,
) -> Result<(), String> {
    let index = p.chats().append(agent_id, chat_id, &message)?;
    p.emit(Update::ChatMessage(ChatMessageEvent {
        agent_id: agent_id.into(),
        chat_id: chat_id.into(),
        message,
    }));
    emit_index(p, agent_id, index);
    Ok(())
}

/// The chat open for an agent (for questions from jobs, which have no chat
/// of their own).
pub fn active_chat<P: Platform>(p: &P, agent_id: &str) -> Result<String, String> {
    p.chats()
        .index(agent_id)?
        .active
        .ok_or_else(|| "That agent has no chat.".into())
}

/// Everything an agent needs to run: its loop (or `loop_id` instead) and
/// its opened workspace.
pub fn prepare<P: Platform>(
    p: &P,
    c: &P::Config,
    agent: &Agent,
    loop_id: Option<&str>,
) -> Result<(LoopFile, Jail), String> {
    let id = loop_id.unwrap_or(&agent.loop_id);
    // Only trusted (reviewed, built-in or user-made) loops run.
    let original = match p.runnable_loop(id) {
        Ok(f) => f,
        Err(LoopProblem::Missing) => {
            return Err(match loop_id {
                Some(_) => format!("The loop “{id}” is missing. Choose another for the job."),
                None => format!(
                    "“{}”'s loop is missing. Choose one in Settings → Agents.",
                    agent.name
                ),
            });
        }
        Err(LoopProblem::Untrusted(name)) => {
            return Err(format!(
                "The loop “{name}” hasn't been reviewed, so it can't run. Open it in the loop editor to review it."
            ));
        }
    };
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
    let jail = p.open_jail(&agent.workspace.path).map_err(|e| {
        format!(
            "“{}”'s workspace can't be used: {e}. Choose its folder again in Settings → Agents.",
            agent.name
        )
    })?;
    if c.agent_model(agent).is_none() {
        return Err("No model is connected. Add one in Settings → Models.".into());
    }
    Ok((file, jail))
}

/// A host for one run, with the permissions in `c`.
pub fn host<P: Platform>(
    p: &P,
    c: &P::Config,
    agent: Option<&Agent>,
    model: Option<ProviderConfig>,
    jail: Option<Jail>,
    origin: Option<Origin>,
) -> AgentHost<P::Services> {
    let setup = AgentSetup {
        installed: if c.permissions().tools {
            p.installed_tools()
        } else {
            vec![]
        },
        sandbox: p.sandbox_health(c),
        jail,
        asks: origin.map(|o| (p.session().asks.clone(), o)),
        time_zone: p.time_zone(),
        usage: Some(p.session().usage.clone()),
        ..c.setup(agent, model)
    };
    AgentHost::new(p.services(), setup)
}

/// Sends a chat message to an agent and starts its loop. Returns the run id.
pub fn send_message<P: Platform>(
    p: &P,
    agent_id: &str,
    chat_id: &str,
    text: &str,
) -> Result<String, String> {
    let text = text.trim().to_string();
    if text.is_empty() {
        return Err("Type a message first.".into());
    }
    let session = p.session();
    if session.shutting_down() {
        return Err(SHUTTING_DOWN.into());
    }
    let c = p.config();
    let agent = c
        .agent(agent_id)
        .cloned()
        .ok_or("That agent no longer exists.")?;
    let index = p.chats().index(agent_id)?;
    let Some(chat) = index.chats.iter().find(|x| x.id == chat_id).cloned() else {
        return Err("That chat no longer exists.".into());
    };
    if session.chat_busy(chat_id) {
        return Err("This chat is still working on the last message.".into());
    }
    if session.is_paused(agent_id, chat_id) {
        return Err("This chat has a paused run. Resume it or discard it first.".into());
    }
    if let Some(what) = session.agent_busy_with(agent_id, true) {
        return Err(format!("{} is busy with {what}.", agent.name));
    }
    let (file, jail) = prepare(p, &c, &agent, None)?;

    let history: Vec<ChatTurn> = p
        .chats()
        .load(agent_id, chat_id)?
        .into_iter()
        .filter(|m| m.role == "user" || m.role == "assistant")
        .map(|m| ChatTurn {
            role: m.role,
            text: m.text,
        })
        .collect();
    append(
        p,
        agent_id,
        chat_id,
        ChatMessage {
            id: msg_id(),
            role: "user".into(),
            text: text.clone(),
            at: now(),
            from: None,
            run: None,
        },
    )?;

    let title = if chat.title == chats::NEW_CHAT {
        chats::title_from(&text)
    } else {
        chat.title.clone()
    };
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
        chat_id: Some(chat_id.to_string()),
        job_id: None,
        title: format!("{} · {title}", agent.name),
    };
    let model = c.agent_model(&agent).cloned();
    let model_name = model.as_ref().map(|m| m.name.clone());
    let host = host(p, &c, Some(&agent), model, Some(jail), Some(origin));
    // A chat's first message: once answered, the chat may be named (see
    // Update::FirstReply).
    let first = inputs.history.is_empty();
    start(
        p,
        Launch {
            run_id: run_id.clone(),
            file,
            host,
            inputs,
            agent: Some(agent),
            chat: Some((chat_id.to_string(), title)),
            source: "chat",
            model: model_name,
            resume: None,
            first,
            earlier_ms: 0,
            earlier: 0,
        },
    );
    Ok(run_id)
}

/// Runs a loop from the editor without saving anything to chat. With an
/// agent, the run uses its model and workspace; without one, the default
/// model and no workspace (file blocks will report an error).
pub fn test_run<P: Platform>(
    p: &P,
    file: LoopFile,
    agent_id: Option<&str>,
    message: String,
    overrides: HashMap<String, String>,
) -> Result<String, String> {
    if p.session().shutting_down() {
        return Err(SHUTTING_DOWN.into());
    }
    let c = p.config();
    let (agent, jail, model) = match agent_id.filter(|a| !a.is_empty()) {
        Some(id) => {
            let agent = c.agent(id).cloned().ok_or("That agent no longer exists.")?;
            if let Some(what) = p.session().agent_busy_with(id, false) {
                return Err(format!(
                    "{} is busy with {what}; test without an agent, or wait.",
                    agent.name
                ));
            }
            let jail = p.open_jail(&agent.workspace.path)?;
            let model = c.agent_model(&agent).cloned();
            (Some(agent), Some(jail), model)
        }
        None => (None, None, c.default_model().cloned()),
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
        chat_id: None,
        job_id: None,
        title: format!("Test: {}", file.name),
    };
    let model_name = model.as_ref().map(|m| m.name.clone());
    let host = host(p, &c, agent.as_ref(), model, jail, Some(origin));
    start(
        p,
        Launch {
            run_id: run_id.clone(),
            file,
            host,
            inputs,
            agent,
            chat: None,
            source: "test",
            model: model_name,
            resume: None,
            first: false,
            earlier_ms: 0,
            earlier: 0,
        },
    );
    Ok(run_id)
}

/// Pauses a chat run after its current block (or right away while it
/// waits on the user, if it can run that block again). Its state is kept
/// until the chat resumes or discards it.
pub fn pause_run<P: Platform>(p: &P, run_id: &str) -> Result<(), String> {
    let runs = lock(&p.session().runs);
    let run = runs.get(run_id).ok_or("That run has already finished.")?;
    let flag = run.pause.as_ref().ok_or("Only chat runs can be paused.")?;
    flag.store(true, Ordering::Relaxed);
    p.emit(Update::RunPausing(RunPausing {
        run_id: run_id.to_string(),
    }));
    Ok(())
}

/// Carries on a chat's paused run, where it stopped, as the same run.
/// Returns its id.
pub fn resume_run<P: Platform>(p: &P, agent_id: &str, chat_id: &str) -> Result<String, String> {
    let session = p.session();
    if session.shutting_down() {
        return Err(SHUTTING_DOWN.into());
    }
    let c = p.config();
    let agent = c
        .agent(agent_id)
        .cloned()
        .ok_or("That agent no longer exists.")?;
    if session.chat_busy(chat_id) {
        return Err("This chat is already working.".into());
    }
    if let Some(what) = session.agent_busy_with(agent_id, true) {
        return Err(format!("{} is busy with {what}.", agent.name));
    }
    // Check everything before taking the run, so a run that can't carry on
    // yet stays paused.
    let run = paused::peek(p, agent_id, chat_id)?;
    if run.workspace != agent.workspace.path {
        return Err(format!(
            "“{}”'s workspace has changed since this run paused, so it can't carry on. Discard it and send the message again.",
            agent.name
        ));
    }
    let jail = p.open_jail(&agent.workspace.path).map_err(|e| {
        format!(
            "“{}”'s workspace can't be used: {e}. Choose its folder again in Settings → Agents.",
            agent.name
        )
    })?;
    let model = c.agent_model(&agent).cloned();
    if model.is_none() {
        return Err("No model is connected. Add one in Settings → Models.".into());
    }
    // From here it's this run's alone: taken once, never again.
    let run = paused::take(p, agent_id, chat_id)?;
    // It carries on as if it never stopped: the "Paused" note goes, and it
    // stays the same run (one response, one log).
    for note in &run.notices {
        if let Ok((index, _, _)) = p.chats().delete_message(agent_id, chat_id, note) {
            p.emit(Update::ChatMessageDeleted(ChatMessageDeleted {
                agent_id: agent_id.to_string(),
                chat_id: chat_id.to_string(),
                message_id: note.clone(),
            }));
            emit_index(p, agent_id, index);
        }
    }
    let title = p
        .chats()
        .index(agent_id)
        .ok()
        .and_then(|ix| ix.chats.into_iter().find(|x| x.id == chat_id))
        .map(|x| x.title)
        .unwrap_or(run.chat_title.clone());
    let run_id = if run.run_id.is_empty() {
        new_run_id()
    } else {
        run.run_id.clone()
    };
    let earlier = run.state.replies().len();
    let origin = Origin {
        run_id: run_id.clone(),
        kind: RunKind::Chat,
        agent_id: Some(agent.id.clone()),
        chat_id: Some(chat_id.to_string()),
        job_id: None,
        title: format!("{} · {title}", agent.name),
    };
    let model_name = model.as_ref().map(|m| m.name.clone());
    let host = host(p, &c, Some(&agent), model, Some(jail), Some(origin));
    syslog::log(
        Level::Info,
        "runs",
        format!("Resuming a paused run: {} · {title}", agent.name),
        &[("run", &run_id)],
    );
    start(
        p,
        Launch {
            run_id: run_id.clone(),
            file: run.file,
            host,
            inputs: run.inputs,
            agent: Some(agent),
            chat: Some((chat_id.to_string(), title)),
            source: "chat",
            model: model_name,
            resume: Some(run.state),
            first: run.first,
            earlier_ms: run.elapsed_ms,
            earlier,
        },
    );
    Ok(run_id)
}

/// Throws away a chat's paused run (the caller has asked the user), and
/// says so in the chat.
pub fn discard_paused<P: Platform>(p: &P, agent_id: &str, chat_id: &str) -> Result<(), String> {
    paused::discard(p, agent_id, chat_id)?;
    append(
        p,
        agent_id,
        chat_id,
        ChatMessage {
            id: msg_id(),
            role: "notice".into(),
            text: "The paused run was discarded.".into(),
            at: now(),
            from: None,
            run: None,
        },
    )
}

/// What a run log says about a run, beyond its [`RunInfo`].
struct RunMeta {
    agent_name: Option<String>,
    loop_name: String,
    model: Option<String>,
    message: String,
    /// A paused run carrying on: its log continues.
    resumed: bool,
}

/// Adds a run to the run list, starts its log and tells watchers. The guard
/// takes it off again however the run ends.
fn track<P: Platform>(p: &P, info: RunInfo, meta: RunMeta) -> RunGuard<P> {
    lock(&p.session().runs).insert(info.run_id.clone(), info.clone());
    let started_at = chummy_log::now_ms();
    let log = p
        .run_logs()
        .and_then(|(dir, opts)| {
            if meta.resumed
                && let Some(log) = RunLog::resume(&dir, &info.run_id, opts.clone())
            {
                return Some(log);
            }
            RunLog::create(
                &dir,
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
        p: p.clone(),
        run_id: info.run_id.clone(),
        log,
        started_at,
        title: info.title.clone(),
        agent_id: info.agent_id.clone(),
    };
    p.emit(Update::RunStarted(info));
    guard
}

/// Sends a run's progress to watchers, and to its log.
fn emitter<P: Platform>(guard: &RunGuard<P>) -> impl FnMut(Event) + Send + use<P> {
    let p = guard.p.clone();
    let run_id = guard.run_id.clone();
    let log = guard.log.clone();
    let (pane, said) = match lock(&p.session().runs).get(&run_id) {
        Some(r) => (Some(r.pane.clone()), Some(r.said.clone())),
        None => (None, None),
    };
    move |event| {
        if let Some(l) = &log {
            l.event(&event);
        }
        if let (Event::Pane { text, .. }, Some(pane)) = (&event, &pane) {
            let mut lines = lock(pane);
            if lines.len() >= PANE_LINES {
                lines.pop_front();
            }
            lines.push_back(text.clone());
        }
        if let (Event::Reply { text, .. }, Some(said)) = (&event, &said) {
            Said::push(&mut lock(said), "assistant", text);
        }
        p.emit(Update::RunEvent(RunEvent {
            run_id: run_id.clone(),
            event,
        }));
    }
}

/// A run to start.
struct Launch<S: chummy_runner::Services> {
    run_id: String,
    file: LoopFile,
    host: AgentHost<S>,
    inputs: RunInputs,
    agent: Option<Agent>,
    /// The chat a chat run belongs to, and its title. Only chat runs can
    /// pause.
    chat: Option<(String, String)>,
    source: &'static str,
    model: Option<String>,
    /// A paused run to carry on (as the same run: `run_id` is its id).
    resume: Option<RunState>,
    /// For a resumed run: how long it worked before, in ms, and how many
    /// replies it had already shown.
    earlier_ms: u64,
    earlier: usize,
    /// The chat's first message: once answered, the chat may be named.
    first: bool,
}

fn start<P: Platform>(p: &P, launch: Launch<P::Services>) {
    let Launch {
        run_id,
        file,
        host,
        inputs,
        agent,
        chat,
        source,
        model,
        resume,
        first,
        earlier_ms,
        earlier,
    } = launch;
    let cancel = Arc::new(AtomicBool::new(false));
    let resumed = resume.is_some();
    let pause = chat.is_some().then(|| Arc::new(AtomicBool::new(false)));
    let (chat_id, chat_title) = chat.unzip();
    let info = RunInfo {
        run_id: run_id.clone(),
        agent_id: agent.as_ref().map(|a| a.id.clone()),
        chat_id: chat_id.clone(),
        chat_title: chat_title.clone(),
        title: agent
            .as_ref()
            .map(|a| a.name.clone())
            .unwrap_or_else(|| format!("Test: {}", file.name)),
        source,
        job_id: None,
        started_at: now(),
        cancel: cancel.clone(),
        pausable: pause.is_some(),
        pause: pause.clone(),
        resumed_ms: earlier_ms,
        pane: Default::default(),
        said: Default::default(),
    };
    // However the run ends (even by a panic), it leaves the run list, so
    // the agent never stays "still working".
    let meta = RunMeta {
        agent_name: agent.as_ref().map(|a| a.name.clone()),
        loop_name: file.name.clone(),
        model: model.clone(),
        message: if resumed {
            format!("(Resumed) {}", inputs.message)
        } else {
            inputs.message.clone()
        },
        resumed,
    };
    let first_message = inputs.message.clone();
    let guard = track(p, info, meta);
    let p = p.clone();
    let id = run_id;
    p.clone().spawn(async move {
        let mut outcome = run_with(
            &file,
            &host,
            &inputs,
            resume,
            &cancel,
            pause.as_deref(),
            Some(&p.session().hurry),
            emitter(&guard),
        )
        .await;
        let end = guard.finish(&outcome);

        let said = lock(&p.session().runs)
            .remove(&id)
            .map(|r| lock(&r.said).clone())
            .unwrap_or_default();
        if let (Some(agent), Some(chat)) = (&agent, &chat_id) {
            let replied = !outcome.replies.is_empty();
            // The whole run's time, across pauses.
            let ms = earlier_ms + end.ended_at.saturating_sub(guard.started_at);
            let paused_state = outcome.paused.take();
            let mut msgs = messages_for(&outcome, &id, &end, ms, said, earlier, Some(true));
            // A resumed run is still one response: what it said before
            // pausing gets the whole run's figures too.
            if resumed {
                let summary = summary_for(&outcome, &id, &end, ms);
                set_run_summary(&p, &agent.id, chat, summary);
            }
            // A paused run is kept for the chat to resume; what it said so
            // far goes to the chat now, with a "Paused" note that goes away
            // when it carries on.
            if let (RunStatus::Paused { reason }, Some(st)) = (&outcome.status, paused_state) {
                let mut run = PausedRun::new(
                    agent.id.clone(),
                    chat.clone(),
                    chat_title.clone().unwrap_or_default(),
                    agent.workspace.path.clone(),
                    file.clone(),
                    inputs.clone(),
                    *st,
                    reason.clone(),
                    first && !replied,
                    model.clone(),
                );
                run.run_id = id.clone();
                run.elapsed_ms = ms;
                run.notices = msgs
                    .iter()
                    .filter(|m| m.role == "notice")
                    .map(|m| m.id.clone())
                    .collect();
                if !paused::keep(&p, run) {
                    for m in msgs.iter_mut().filter(|m| m.role == "notice") {
                        m.text.push_str(NOT_SAVED);
                    }
                }
            }
            for m in msgs {
                let _ = append(&p, &agent.id, chat, m);
            }
            if first && replied {
                p.emit(Update::FirstReply(FirstReply {
                    agent_id: agent.id.clone(),
                    chat_id: chat.clone(),
                    message: first_message,
                    reply: outcome.replies.join("\n\n"),
                }));
            }
        }
        p.emit(Update::RunFinished(RunFinished {
            run_id: id,
            agent_id: agent.map(|a| a.id),
            chat_id,
            outcome,
        }));
        drop(guard);
    });
}

/// Runs a job as its agent (same folder, model and permissions as chatting
/// with it), shown like any other run. Its questions go to the agent's chat.
pub async fn run_job<P: Platform>(p: &P, req: &RunRequest) -> RunResult {
    let c = p.config();
    let job = &req.job;
    let Some(agent) = c.agent(&job.agent).cloned() else {
        return RunResult::failed(format!(
            "The agent this job runs as no longer exists. Choose another for “{}”.",
            job.name
        ));
    };
    let (file, jail) = match prepare(p, &c, &agent, job.loop_id.as_deref()) {
        Ok(x) => x,
        Err(e) => return RunResult::failed(e),
    };
    let origin = Origin {
        run_id: req.run_id.clone(),
        kind: RunKind::Job,
        agent_id: Some(agent.id.clone()),
        chat_id: None,
        job_id: Some(job.id.clone()),
        title: job.name.clone(),
    };
    let model = c.agent_model(&agent).cloned();
    let model_name = model.as_ref().map(|m| m.name.clone());
    let host = host(p, &c, Some(&agent), model, Some(jail), Some(origin));
    let guard = track(
        p,
        RunInfo {
            run_id: req.run_id.clone(),
            agent_id: Some(agent.id.clone()),
            chat_id: None,
            chat_title: None,
            title: job.name.clone(),
            source: "job",
            job_id: Some(job.id.clone()),
            started_at: now(),
            cancel: req.cancel.clone(),
            pause: None,
            pausable: false,
            resumed_ms: 0,
            pane: Default::default(),
            said: Default::default(),
        },
        RunMeta {
            agent_name: Some(agent.name.clone()),
            loop_name: file.name.clone(),
            model: model_name,
            message: job.message.clone(),
            resumed: false,
        },
    );
    let tz = p.job_time_zone();
    let (result, outcome) = chummy_runner::run_job(req, &file, &host, &tz, emitter(&guard)).await;
    if let Some(outcome) = outcome {
        guard.finish(&outcome);
        p.emit(Update::RunFinished(RunFinished {
            run_id: req.run_id.clone(),
            agent_id: Some(agent.id.clone()),
            chat_id: None,
            outcome,
        }));
    }
    result
}

/// What's in a folder of an agent's (for jobs that watch folders). The
/// folder is opened, and checked, afresh every time.
pub fn snapshot<P: Platform>(p: &P, agent: &str, folder: &str) -> Result<Snapshot, String> {
    let c = p.config();
    let agent = c.agent(agent).ok_or("the agent no longer exists")?;
    let jail = p.open_jail(&agent.workspace.path)?;
    chummy_runner::snapshot(&jail, folder)
}

/// What an answer looks like in the chat.
fn answer_text(a: &Answer) -> String {
    match a {
        Answer::Text(t) => t.clone(),
        Answer::Allow => "Allowed.".into(),
        Answer::Deny(why) if why.trim().is_empty() => "Not allowed.".into(),
        Answer::Deny(why) => format!("Not allowed: {}", why.trim()),
    }
}

/// Answers a question or tool approval. Answers only ever come from the
/// user, through the program's own interface; nothing a loop or a model
/// does can answer for them.
pub fn answer<P: Platform>(p: &P, id: &str, answer: Answer) -> Result<(), String> {
    let session = p.session();
    let pending = session
        .asks
        .pending()
        .into_iter()
        .find(|q| q.id == id)
        .ok_or("That question was already answered, or its run stopped.")?;
    session.asks.answer(id, answer.clone()).map_err(|e| {
        let e = e.to_string();
        let mut c = e.chars();
        c.next()
            .map(|f| f.to_uppercase().chain(c).collect())
            .unwrap_or(e)
    })?;
    let question = pending.ask.text.clone();
    let answer_said = answer_text(&answer);
    // A chat run keeps them in its response, in order, saved when it ends.
    if pending.origin.kind == RunKind::Chat {
        let said = lock(&session.runs)
            .get(&pending.origin.run_id)
            .map(|r| r.said.clone());
        if let Some(said) = said {
            let mut parts = lock(&said);
            for (role, text) in [("question", &question), ("answer", &answer_said)] {
                Said::push(&mut parts, role, text);
                p.emit(Update::RunSaid(RunSaid {
                    run_id: pending.origin.run_id.clone(),
                    role: role.into(),
                    text: text.clone(),
                }));
            }
            return Ok(());
        }
    }
    // Otherwise keep a record in the chat (a job's questions go to the
    // agent's open chat). Test runs aren't part of a chat.
    if pending.origin.kind != RunKind::Test
        && let Some(agent) = &pending.origin.agent_id
        && let Some(chat) = pending
            .origin
            .chat_id
            .clone()
            .or_else(|| active_chat(p, agent).ok())
    {
        let from = (pending.origin.kind == RunKind::Job).then(|| pending.origin.title.clone());
        for (role, text) in [("question", question), ("answer", answer_said)] {
            let _ = append(
                p,
                agent,
                &chat,
                ChatMessage {
                    id: msg_id(),
                    role: role.into(),
                    text,
                    at: now(),
                    from: from.clone(),
                    run: None,
                },
            );
        }
    }
    Ok(())
}

struct RunGuard<P: Platform> {
    p: P,
    run_id: String,
    log: Option<Arc<RunLog>>,
    started_at: u64,
    title: String,
    agent_id: Option<String>,
}

impl<P: Platform> RunGuard<P> {
    /// Records how the run ended, in its log and the system log, and
    /// returns that record (with today's use of the model).
    fn finish(&self, o: &RunOutcome) -> RunEnd {
        let mut end = RunEnd::from_outcome(o);
        let c = self.p.config();
        // The model the turn ended on: the block's, else the agent's, else the default.
        let model = o
            .usage
            .model
            .clone()
            .or_else(|| {
                self.agent_id
                    .as_deref()
                    .and_then(|a| c.agent(a))
                    .and_then(|a| a.model.clone())
            })
            .or_else(|| c.default_provider().map(str::to_string));
        if let Some(pc) = model.as_deref().and_then(|m| c.provider(m)) {
            (end.daily_used, end.daily_allowed) = self.p.session().usage.daily(pc);
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
                    "paused" => "was paused",
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

impl<P: Platform> Drop for RunGuard<P> {
    fn drop(&mut self) {
        let session = self.p.session();
        lock(&session.runs).remove(&self.run_id);
        session.asks.withdraw_run(&self.run_id);
    }
}

/// Added to a "Paused" note when the run couldn't be sealed (no keychain).
const NOT_SAVED: &str =
    " There's no system keychain to keep it safely, so it's only kept until Chummy quits.";

/// Gives a run's earlier messages its latest figures, and tells watchers.
fn set_run_summary<P: Platform>(p: &P, agent: &str, chat: &str, summary: RunSummary) {
    if let Ok(true) = p.chats().set_run_summary(agent, chat, &summary) {
        p.emit(Update::ChatRunUpdated(ChatRunUpdated {
            agent_id: agent.to_string(),
            chat_id: chat.to_string(),
            summary,
        }));
    }
}

/// A run's figures, as its chat messages carry them.
fn summary_for(o: &RunOutcome, run_id: &str, end: &RunEnd, ms: u64) -> RunSummary {
    let status = match &o.status {
        RunStatus::Paused { .. } => "paused",
        RunStatus::Completed => "completed",
        RunStatus::Stopped => "stopped",
        RunStatus::LimitReached { .. } => "limit",
        RunStatus::Failed { .. } => "failed",
    };
    RunSummary {
        run_id: run_id.into(),
        status: status.into(),
        steps: o.steps,
        llm_calls: o.llm_calls,
        tool_calls: o.tool_calls,
        ms,
        tokens: end.tokens_added,
        context_tokens: end.context_tokens,
        context_window: end.context_window,
        daily_used: end.daily_used,
        daily_allowed: end.daily_allowed,
    }
}

/// The chat messages a run (or the part of it since it last paused) adds:
/// its response in order (replies, questions, the user's answers), then a
/// notice of how it ended if that needs saying, all carrying the run's
/// figures so a chat shows them as one response. `earlier` replies were
/// already shown before a pause.
fn messages_for(
    o: &RunOutcome,
    run_id: &str,
    end: &RunEnd,
    ms: u64,
    said: Vec<Said>,
    earlier: usize,
    // For a paused run: whether it was saved (or is held in memory only).
    saved: Option<bool>,
) -> Vec<ChatMessage> {
    let (status, notice) = match &o.status {
        RunStatus::Paused { reason } => {
            let mut text = match reason {
                Some(why) => format!(
                    "Paused: {} Resume once the model is back.",
                    chummy_flow::sentence(why)
                ),
                None => "Paused. Resume to carry on where it stopped.".to_string(),
            };
            if saved == Some(false) {
                text.push_str(NOT_SAVED);
            }
            ("paused", Some(text))
        }
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
        // A loop that shows its own reason (like the Chummy assistant's
        // "I couldn't finish") doesn't need it said twice.
        RunStatus::Failed { error, .. }
            if o.replies
                .last()
                .is_some_and(|r| !r.trim().is_empty() && error.contains(r.trim())) =>
        {
            ("failed", Some("The run ended with this error.".to_string()))
        }
        RunStatus::Failed { error, .. } => ("failed", Some(error.clone())),
    };
    let summary = summary_for(o, run_id, end, ms);
    debug_assert_eq!(summary.status, status);
    let mut parts = said;
    let new_replies = o.replies.get(earlier..).unwrap_or_default();
    if !parts.iter().any(|p| p.role == "assistant") && !new_replies.is_empty() {
        Said::push(&mut parts, "assistant", &new_replies.join("\n\n"));
    }
    let mut out: Vec<ChatMessage> = parts
        .into_iter()
        .map(|p| ChatMessage {
            id: msg_id(),
            role: p.role,
            text: p.text,
            at: now(),
            from: None,
            run: Some(summary.clone()),
        })
        .collect();
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

#[cfg(test)]
mod tests {
    use super::*;
    use chummy_flow::RunStatus;

    fn outcome(status: RunStatus, replies: &[&str]) -> RunOutcome {
        RunOutcome {
            status,
            replies: replies.iter().map(|s| s.to_string()).collect(),
            steps: 3,
            llm_calls: 1,
            tool_calls: 0,
            waiting: vec![],
            usage: Default::default(),
            paused: None,
        }
    }

    #[test]
    fn a_run_is_one_response_in_order() {
        let mut said = Vec::new();
        Said::push(&mut said, "assistant", "Looking.");
        Said::push(&mut said, "assistant", "Found it.");
        Said::push(&mut said, "question", "Which one?");
        Said::push(&mut said, "answer", "The first");
        Said::push(&mut said, "assistant", "Done.");
        let o = outcome(
            RunStatus::Failed {
                error: "the model server failed".into(),
                block: None,
            },
            &["Looking.", "Found it.", "Done."],
        );
        let m = messages_for(&o, "run1", &RunEnd::default(), 1500, said, 0, None);
        let roles: Vec<&str> = m.iter().map(|m| m.role.as_str()).collect();
        assert_eq!(
            roles,
            ["assistant", "question", "answer", "assistant", "notice"]
        );
        assert_eq!(m[0].text, "Looking.\n\nFound it.");
        // Every part carries the run, so the chat shows one response with
        // one set of figures and one delete.
        assert!(m.iter().all(|m| m.run.as_ref().unwrap().run_id == "run1"));
        assert_eq!(m[4].run.as_ref().unwrap().ms, 1500);
    }

    #[test]
    fn a_resumed_run_carries_on_its_response() {
        // Paused after one reply: what it said, then a "Paused" note (which
        // goes when it carries on), all as the same run.
        let mut said = Vec::new();
        Said::push(&mut said, "assistant", "Looking.");
        let o = outcome(RunStatus::Paused { reason: None }, &["Looking."]);
        let m = messages_for(&o, "run1", &RunEnd::default(), 800, said, 0, Some(true));
        assert_eq!(m.len(), 2);
        assert_eq!(
            (
                m[1].role.as_str(),
                m[1].run.as_ref().unwrap().status.as_str()
            ),
            ("notice", "paused")
        );

        // Resumed and done, without saying more: the reply from before the
        // pause isn't repeated, and it doesn't count as "no reply".
        let o = outcome(RunStatus::Completed, &["Looking."]);
        let m = messages_for(&o, "run1", &RunEnd::default(), 2000, vec![], 1, None);
        assert!(m.is_empty(), "{m:?}");
        let summary = summary_for(&o, "run1", &RunEnd::default(), 2000);
        assert_eq!((summary.status.as_str(), summary.ms), ("completed", 2000));

        // Resumed and says more: only the new reply.
        let o = outcome(RunStatus::Completed, &["Looking.", "Found it."]);
        let m = messages_for(&o, "run1", &RunEnd::default(), 2000, vec![], 1, None);
        assert_eq!(m.len(), 1);
        assert_eq!(m[0].text, "Found it.");
        assert_eq!(m[0].run.as_ref().unwrap().run_id, "run1");
    }

    #[test]
    fn a_reason_already_shown_isnt_repeated() {
        let why = "**I couldn't finish:** it failed 3 times";
        let o = outcome(
            RunStatus::Failed {
                error: format!("Stop with error: {why}"),
                block: None,
            },
            &[why],
        );
        let m = messages_for(&o, "r", &RunEnd::default(), 0, vec![], 0, None);
        assert_eq!(m[1].text, "The run ended with this error.");
        // Otherwise the error is the notice.
        let o = outcome(
            RunStatus::Failed {
                error: "Work: the server is down".into(),
                block: None,
            },
            &["Looking."],
        );
        let m = messages_for(&o, "r", &RunEnd::default(), 0, vec![], 0, None);
        assert_eq!(m[1].text, "Work: the server is down");
    }

    #[test]
    fn answers_read_well() {
        assert_eq!(answer_text(&Answer::Allow), "Allowed.");
        assert_eq!(answer_text(&Answer::Deny(" ".into())), "Not allowed.");
        assert_eq!(answer_text(&Answer::Deny("no".into())), "Not allowed: no");
        assert_eq!(answer_text(&Answer::Text("x".into())), "x");
    }

    #[test]
    fn replies_are_kept_without_a_record() {
        let o = outcome(RunStatus::Completed, &["a", "b"]);
        let m = messages_for(&o, "r", &RunEnd::default(), 0, vec![], 0, None);
        assert_eq!(m.len(), 1);
        assert_eq!(m[0].text, "a\n\nb");
    }
}
