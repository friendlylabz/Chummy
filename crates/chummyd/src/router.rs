//! Where everything meets: what people send through channels (now the
//! terminal), the session's updates, questions waiting, and jobs finishing.
//! One task handles them in order.
//!
//! A message from a person can only start a run of the agent's own loop, or
//! carry out a command. Commands come only from what people send, never
//! from what an agent says.

use std::collections::HashMap;
use std::io::Write;
use std::sync::Arc;

use chummy_channel::{Command, HELP, Line, RunView};
use chummy_flow::{Answer, AskKind};
use chummy_scheduler::{AskEvent, Job, Manager, Pending, RecordStatus, RunKind, RunRecord};
use chummy_session::{Platform, Update};

use crate::jobs::DaemonJobs;
use crate::platform::DaemonPlatform;
use crate::saver::Saver;

/// The terminal's conversation with each agent.
pub const TERMINAL_CHAT: &str = "terminal";

/// What an approval asked where approvals aren't allowed is answered with.
pub const NOT_HERE: &str = "approvals aren't allowed here";

pub enum Input {
    /// A line typed in the terminal.
    Line(String),
    /// The terminal closed.
    Eof,
    Update(Box<Update>),
    Ask(AskEvent),
    JobFinished(Box<(Job, RunRecord)>),
    /// chummyd is shutting down: take no more messages.
    ShuttingDown,
}

pub struct Router {
    p: DaemonPlatform,
    jobs: Manager<DaemonJobs>,
    saver: Arc<Saver>,
    terminal: bool,
    /// The agent the terminal talks to.
    current: String,
    /// Runs the terminal is showing, by run id.
    views: HashMap<String, RunView>,
    accepting: bool,
    /// Asked to stop (`/quit`).
    quit: Arc<tokio::sync::Notify>,
}

fn print(who: &str, text: &str) {
    let mut out = std::io::stdout().lock();
    let mut lines = text.lines();
    let first = lines.next().unwrap_or("");
    let _ = writeln!(out, "{who}: {first}");
    let pad = " ".repeat(who.chars().count() + 2);
    for l in lines {
        let _ = writeln!(out, "{pad}{l}");
    }
    let _ = out.flush();
}

fn note(text: &str) {
    print("chummyd", text);
}

impl Router {
    pub fn new(
        p: DaemonPlatform,
        jobs: Manager<DaemonJobs>,
        saver: Arc<Saver>,
        quit: Arc<tokio::sync::Notify>,
    ) -> Self {
        let terminal = p.0.bundle.channels.terminal.enabled;
        let current =
            p.0.bundle
                .agents
                .first()
                .map(|a| a.id.clone())
                .unwrap_or_default();
        Self {
            p,
            jobs,
            saver,
            terminal,
            current,
            views: HashMap::new(),
            accepting: true,
            quit,
        }
    }

    fn agent_name(&self, id: &str) -> String {
        self.p
            .0
            .bundle
            .agent(id)
            .map(|a| a.name.clone())
            .unwrap_or_else(|| id.to_string())
    }

    /// Approvals may be answered over channels for this agent.
    fn approvals_allowed(&self, agent: Option<&str>) -> bool {
        self.terminal
            && agent
                .and_then(|a| self.p.0.bundle.agent(a))
                .is_some_and(|a| a.channel_approvals)
    }

    /// Whether a question belongs to the terminal's conversation with the
    /// current agent (its runs, or its jobs).
    fn mine(&self, q: &Pending) -> bool {
        q.origin.agent_id.as_deref() == Some(self.current.as_str())
            && match q.origin.kind {
                RunKind::Chat => q.origin.chat_id.as_deref() == Some(TERMINAL_CHAT),
                RunKind::Job => true,
                RunKind::Test => false,
            }
    }

    pub fn greet(&self) {
        if self.terminal {
            note(&format!(
                "Talking to {}. Type a message, or /help.",
                self.agent_name(&self.current)
            ));
            self.say_paused();
        }
    }

    fn say_paused(&self) {
        if self.p.session().is_paused(&self.current, TERMINAL_CHAT) {
            note(
                "This conversation has a paused run: /resume carries it on, /discard throws it away.",
            );
        }
    }

    pub async fn handle(&mut self, input: Input) {
        match input {
            Input::Line(text) => self.line(text.trim()),
            Input::Eof => {
                if self.terminal {
                    note(
                        "The terminal closed; chummyd keeps running. Stop it with Ctrl-C (or SIGTERM).",
                    );
                }
            }
            Input::Update(u) => self.update(&u),
            Input::Ask(AskEvent::Asked { pending }) => self.asked(&pending),
            Input::Ask(AskEvent::Closed { .. }) => {}
            Input::JobFinished(f) => self.job_finished(&f.0, &f.1),
            Input::ShuttingDown => self.accepting = false,
        }
    }

    fn line(&mut self, text: &str) {
        if !self.terminal || text.is_empty() {
            return;
        }
        if !self.accepting {
            note("chummyd is shutting down.");
            return;
        }
        match chummy_channel::parse(text) {
            None => self.message(text),
            Some(Err(e)) => note(&e),
            Some(Ok(c)) => self.command(c),
        }
    }

    fn ensure_chat(&self) {
        let _ = self
            .p
            .0
            .chats
            .ensure(&self.current, TERMINAL_CHAT, "Terminal");
    }

    fn message(&mut self, text: &str) {
        // A question waiting for this conversation: this is the answer.
        if let Some(q) = self
            .p
            .session()
            .asks
            .pending()
            .into_iter()
            .find(|q| self.mine(q) && matches!(q.ask.kind, AskKind::Question))
        {
            if let Err(e) = chummy_session::answer(&self.p, &q.id, Answer::Text(text.into())) {
                note(&e);
            }
            return;
        }
        self.ensure_chat();
        match chummy_session::send_message(&self.p, &self.current, TERMINAL_CHAT, text) {
            Ok(run) => {
                self.views.insert(run.clone(), RunView::new(run));
            }
            Err(e) if e.contains("still working") => {
                note("Busy with your last message: /stop it, or wait for the reply.")
            }
            Err(e) => note(&e),
        }
    }

    /// The run going on in the terminal's conversation, if any.
    fn running(&self) -> Option<String> {
        self.p
            .session()
            .runs()
            .into_iter()
            .find(|r| {
                r.agent_id.as_deref() == Some(self.current.as_str())
                    && r.chat_id.as_deref() == Some(TERMINAL_CHAT)
            })
            .map(|r| r.run_id)
    }

    fn approval(&self) -> Option<Pending> {
        self.p
            .session()
            .asks
            .pending()
            .into_iter()
            .find(|q| self.mine(q) && !matches!(q.ask.kind, AskKind::Question))
    }

    fn command(&mut self, c: Command) {
        let session = self.p.session();
        match c {
            Command::Stop => match self.running() {
                Some(r) => {
                    session.stop(&r);
                    note("Stopping.");
                }
                None => note("Nothing is running."),
            },
            Command::Pause => match self.running() {
                Some(r) => match chummy_session::pause_run(&self.p, &r) {
                    Ok(()) => note("Pausing after this step…"),
                    Err(e) => note(&e),
                },
                None => note("Nothing is running."),
            },
            Command::Resume => {
                match chummy_session::resume_run(&self.p, &self.current, TERMINAL_CHAT) {
                    Ok(run) => {
                        note("Carrying on where it stopped.");
                        self.views.insert(run.clone(), RunView::new(run));
                    }
                    Err(e) => note(&e),
                }
            }
            Command::Discard => {
                if !session.is_paused(&self.current, TERMINAL_CHAT) {
                    note("There's no paused run.");
                } else {
                    match chummy_session::discard_paused(&self.p, &self.current, TERMINAL_CHAT) {
                        Ok(()) => note("The paused run was discarded."),
                        Err(e) => note(&e),
                    }
                }
            }
            Command::New => {
                if self.running().is_some() {
                    note("Busy: /stop the run first.");
                } else if session.is_paused(&self.current, TERMINAL_CHAT) {
                    note("This conversation has a paused run: /resume or /discard it first.");
                } else {
                    self.ensure_chat();
                    match self.p.0.chats.clear(&self.current, TERMINAL_CHAT) {
                        Ok(()) => {
                            self.p.mark_dirty();
                            note("Started a fresh conversation.");
                        }
                        Err(e) => note(&e),
                    }
                }
            }
            Command::Status => self.status(),
            Command::Jobs => self.list_jobs(),
            Command::Run(name) => self.run_job(&name),
            Command::Allow | Command::Deny(_) => {
                let Some(q) = self.approval() else {
                    note("Nothing is waiting for an approval.");
                    return;
                };
                if !self.approvals_allowed(q.origin.agent_id.as_deref()) {
                    note("Approvals aren't allowed here.");
                    return;
                }
                let a = match c {
                    Command::Allow => Answer::Allow,
                    Command::Deny(why) => Answer::Deny(why),
                    _ => unreachable!(),
                };
                if let Err(e) = chummy_session::answer(&self.p, &q.id, a) {
                    note(&e);
                }
            }
            Command::Agents => {
                let list: Vec<String> = self
                    .p
                    .0
                    .bundle
                    .agents
                    .iter()
                    .map(|a| {
                        let mark = if a.id == self.current {
                            " (talking)"
                        } else {
                            ""
                        };
                        format!("{} [{}]{mark}", a.name, a.id)
                    })
                    .collect();
                note(&list.join("\n"));
            }
            Command::Agent(name) => {
                let want = name.to_lowercase();
                match self
                    .p
                    .0
                    .bundle
                    .agents
                    .iter()
                    .find(|a| a.id.to_lowercase() == want || a.name.to_lowercase() == want)
                {
                    Some(a) => {
                        self.current = a.id.clone();
                        note(&format!("Now talking to {}.", a.name));
                        self.say_paused();
                    }
                    None => note("There's no agent by that name: /agents lists them."),
                }
            }
            Command::Quit => {
                note("Stopping chummyd…");
                self.quit.notify_one();
            }
            Command::Help => note(&format!(
                "{HELP}\n/agents          the agents you can talk to\n/agent <name>    talk to another agent\n/quit            stop chummyd"
            )),
        }
    }

    fn status(&self) {
        let session = self.p.session();
        let mut lines = vec![];
        for r in session.runs() {
            let what = match (r.source, &r.chat_id) {
                ("job", _) => format!("job {}", r.title),
                (_, Some(_)) => format!("{} (conversation)", r.title),
                _ => r.title.clone(),
            };
            let secs = chummy_session::now().saturating_sub(r.started_at);
            lines.push(format!("Running: {what}, {secs}s"));
        }
        for p in session.paused() {
            lines.push(format!(
                "Paused: {} ({})",
                self.agent_name(&p.agent_id),
                p.reason.as_deref().unwrap_or("paused")
            ));
        }
        let waiting = session.asks.pending().len();
        if waiting > 0 {
            lines.push(format!("Waiting for an answer: {waiting}"));
        }
        if lines.is_empty() {
            lines.push("Nothing is running.".into());
        }
        lines.push(format!("Talking to {}.", self.agent_name(&self.current)));
        note(&lines.join("\n"));
    }

    fn agent_jobs(&self) -> Vec<chummy_scheduler::JobView> {
        self.jobs
            .overview()
            .jobs
            .into_iter()
            .filter(|j| j.job.agent == self.current)
            .collect()
    }

    fn list_jobs(&self) {
        let jobs = self.agent_jobs();
        if jobs.is_empty() {
            note(&format!("{} has no jobs.", self.agent_name(&self.current)));
            return;
        }
        let tz = self.p.job_time_zone();
        let lines: Vec<String> = jobs
            .iter()
            .map(|j| {
                let next = j
                    .next_run
                    .and_then(|t| jiff::Timestamp::from_second(t).ok())
                    .map(|t| {
                        format!(
                            ", next {}",
                            t.to_zoned_datetime(tz.clone()).strftime("%a %d %b %H:%M")
                        )
                    })
                    .unwrap_or_default();
                let schedule = if j.schedule.is_empty() {
                    "by hand".to_string()
                } else {
                    j.schedule.join("; ")
                };
                format!(
                    "{} [{}]: {schedule}{next} ({})",
                    j.job.name, j.job.id, j.state
                )
            })
            .collect();
        note(&lines.join("\n"));
    }

    fn run_job(&self, name: &str) {
        let want = name.to_lowercase();
        let Some(j) = self
            .agent_jobs()
            .into_iter()
            .find(|j| j.job.id.to_lowercase() == want || j.job.name.to_lowercase() == want)
        else {
            note("There's no job by that name for this agent: /jobs lists them.");
            return;
        };
        match self.jobs.run_now(&j.job.id) {
            Ok(()) => note(&format!("Started “{}”.", j.job.name)),
            Err(e) => note(&e),
        }
    }

    fn update(&mut self, u: &Update) {
        let mut done = None;
        for (id, v) in self.views.iter_mut() {
            let lines = v.update(u);
            if self.terminal {
                let agent = self
                    .p
                    .0
                    .bundle
                    .agent(&self.current)
                    .map(|a| a.name.clone())
                    .unwrap_or_default();
                for l in lines {
                    match l {
                        Line::Reply(t) => print(&agent, &t),
                        Line::Notice(t) => print(&agent, &format!("({})", t.trim())),
                        Line::Figures(f) => {
                            let mut out = std::io::stdout().lock();
                            let _ = writeln!(out, "  · {f}");
                        }
                        // Shown when asked, and typed by the person.
                        Line::Question(_) | Line::Answer(_) => {}
                    }
                }
            }
            if v.done() {
                done = Some(id.clone());
            }
        }
        if let Some(id) = done {
            self.views.remove(&id);
        }
        if matches!(u, Update::RunFinished(_)) {
            self.saver.save_now();
        }
    }

    fn asked(&mut self, q: &Pending) {
        let agent = q.origin.agent_id.as_deref();
        let who = match q.origin.kind {
            RunKind::Job => format!("Job “{}”", q.origin.title),
            _ => agent.map(|a| self.agent_name(a)).unwrap_or_default(),
        };
        match &q.ask.kind {
            AskKind::Question => {
                if self.terminal && self.question_shown_here(q) {
                    print(
                        &who,
                        &format!("{} (your next message is the answer)", q.ask.text),
                    );
                }
            }
            kind => {
                if !self.approvals_allowed(agent) {
                    chummy_log::system::log(
                        chummy_log::system::Level::Info,
                        "questions",
                        "Refused an approval: approvals aren't allowed over channels for this agent",
                        &[("run", &q.origin.run_id)],
                    );
                    let _ = chummy_session::answer(&self.p, &q.id, Answer::Deny(NOT_HERE.into()));
                    return;
                }
                let text = match kind {
                    AskKind::ToolApproval { tool, arguments } => {
                        format!("wants to use {tool} with {arguments}")
                    }
                    _ => q.ask.text.clone(),
                };
                print(&who, &format!("{text}\n/allow, or /deny [reason]"));
            }
        }
    }

    /// Questions show in the terminal when they're for one of its
    /// conversations, or from a job.
    fn question_shown_here(&self, q: &Pending) -> bool {
        match q.origin.kind {
            RunKind::Chat => q.origin.chat_id.as_deref() == Some(TERMINAL_CHAT),
            RunKind::Job => true,
            RunKind::Test => false,
        }
    }

    fn job_finished(&mut self, job: &Job, r: &RunRecord) {
        self.saver.save_now();
        if !self.terminal {
            return;
        }
        let how = match r.status {
            RecordStatus::Completed => "finished",
            RecordStatus::Failed => "failed",
            RecordStatus::Stopped => "was stopped",
            RecordStatus::Limit => "hit a limit",
            RecordStatus::Missed => "was missed",
            RecordStatus::Skipped => "was skipped",
        };
        let mut text = format!("“{}” ({}) {how}.", job.name, self.agent_name(&job.agent));
        if !r.note.is_empty() {
            text.push_str(&format!(" {}", r.note));
        }
        note(&text);
        if !r.reply.is_empty() {
            print(&self.agent_name(&job.agent), &r.reply);
        }
    }
}

/// Reads the terminal's lines on a thread of its own.
pub fn read_terminal(inbox: tokio::sync::mpsc::UnboundedSender<Input>) {
    std::thread::spawn(move || {
        let stdin = std::io::stdin();
        let mut line = String::new();
        loop {
            line.clear();
            match stdin.read_line(&mut line) {
                Ok(0) | Err(_) => {
                    let _ = inbox.send(Input::Eof);
                    return;
                }
                Ok(_) => {
                    if inbox.send(Input::Line(line.clone())).is_err() {
                        return;
                    }
                }
            }
        }
    });
}
