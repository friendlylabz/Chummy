//! Where everything meets: what people send through channels (the terminal
//! and Matrix), the session's updates, questions waiting, and jobs
//! finishing. One task handles them in order.
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
use tokio::sync::mpsc::UnboundedSender;

use crate::jobs::DaemonJobs;
use crate::link::{FromMatrix, RoomLink, ToMatrix, matrix_chat};
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
    #[cfg_attr(not(feature = "matrix"), allow(dead_code))]
    Matrix(FromMatrix),
    Update(Box<Update>),
    Ask(AskEvent),
    JobFinished(Box<(Job, RunRecord)>),
    /// chummyd is shutting down: take no more messages.
    ShuttingDown,
}

/// A conversation: someone talking to an agent through a channel.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
enum Conv {
    Terminal(String),
    Matrix(RoomLink),
}

impl Conv {
    fn agent(&self) -> &str {
        match self {
            Conv::Terminal(a) => a,
            Conv::Matrix(r) => &r.agent,
        }
    }

    fn chat(&self) -> String {
        match self {
            Conv::Terminal(_) => TERMINAL_CHAT.into(),
            Conv::Matrix(r) => matrix_chat(&r.user),
        }
    }

    fn title(&self) -> String {
        match self {
            Conv::Terminal(_) => "Terminal".into(),
            Conv::Matrix(r) => r.user.clone(),
        }
    }
}

pub struct Router {
    p: DaemonPlatform,
    jobs: Manager<DaemonJobs>,
    saver: Arc<Saver>,
    terminal: bool,
    /// The agent the terminal talks to.
    current: String,
    /// The Matrix channel, when there is one.
    matrix: Option<UnboundedSender<ToMatrix>>,
    /// Its rooms, once they're set up.
    rooms: Vec<RoomLink>,
    /// Runs being shown, by run id, with where they're shown.
    views: HashMap<String, (Conv, RunView)>,
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

/// chummyd saying something in the terminal.
pub fn note(text: &str) {
    print("chummyd", text);
}

impl Router {
    pub fn new(
        p: DaemonPlatform,
        jobs: Manager<DaemonJobs>,
        saver: Arc<Saver>,
        matrix: Option<UnboundedSender<ToMatrix>>,
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
            matrix,
            rooms: vec![],
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

    fn to_matrix(&self, m: ToMatrix) {
        if let Some(tx) = &self.matrix {
            let _ = tx.send(m);
        }
    }

    /// chummyd saying something in a conversation.
    fn say(&self, to: &Conv, text: &str) {
        match to {
            Conv::Terminal(_) => {
                if self.terminal {
                    note(text)
                }
            }
            Conv::Matrix(r) => self.to_matrix(ToMatrix::Notice {
                room: r.room.clone(),
                text: text.into(),
            }),
        }
    }

    fn terminal_conv(&self) -> Conv {
        Conv::Terminal(self.current.clone())
    }

    /// Who a job reports to in Matrix.
    fn job_room(&self, job_id: &str, agent: &str) -> Option<&RoomLink> {
        let user = self
            .p
            .0
            .bundle
            .channels
            .matrix
            .as_ref()?
            .report_to
            .get(job_id)?;
        self.rooms
            .iter()
            .find(|r| &r.user == user && r.agent == agent)
    }

    /// The Matrix room a chat belongs to.
    fn chat_room(&self, agent: &str, chat: &str) -> Option<&RoomLink> {
        self.rooms
            .iter()
            .find(|r| r.agent == agent && matrix_chat(&r.user) == chat)
    }

    /// Where a question shows (and can be answered).
    fn ask_convs(&self, q: &Pending) -> Vec<Conv> {
        let Some(agent) = q.origin.agent_id.as_deref() else {
            return vec![];
        };
        let mut out = vec![];
        match q.origin.kind {
            RunKind::Chat => match q.origin.chat_id.as_deref() {
                Some(TERMINAL_CHAT) if self.terminal => out.push(Conv::Terminal(agent.into())),
                Some(chat) => {
                    if let Some(r) = self.chat_room(agent, chat) {
                        out.push(Conv::Matrix(r.clone()));
                    }
                }
                None => {}
            },
            RunKind::Job => {
                if self.terminal {
                    out.push(Conv::Terminal(agent.into()));
                }
                if let Some(r) = q
                    .origin
                    .job_id
                    .as_deref()
                    .and_then(|j| self.job_room(j, agent))
                {
                    out.push(Conv::Matrix(r.clone()));
                }
            }
            RunKind::Test => {}
        }
        out
    }

    /// Whether a question belongs to a conversation: its runs, or (for the
    /// terminal, and the person a job reports to) its agent's jobs.
    fn mine(&self, to: &Conv, q: &Pending) -> bool {
        q.origin.agent_id.as_deref() == Some(to.agent())
            && self.ask_convs(q).iter().any(|c| match (c, to) {
                (Conv::Terminal(_), Conv::Terminal(_)) => true,
                (Conv::Matrix(a), Conv::Matrix(b)) => a.room == b.room,
                _ => false,
            })
    }

    /// Approvals may be answered over channels for this agent.
    fn approvals_allowed(&self, agent: Option<&str>) -> bool {
        agent
            .and_then(|a| self.p.0.bundle.agent(a))
            .is_some_and(|a| a.channel_approvals)
    }

    pub fn greet(&self) {
        if self.terminal {
            note(&format!(
                "Talking to {}. Type a message, or /help.",
                self.agent_name(&self.current)
            ));
            self.say_paused(&self.terminal_conv());
        }
    }

    fn say_paused(&self, to: &Conv) {
        if self.p.session().is_paused(to.agent(), &to.chat()) {
            self.say(
                to,
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
                if self.matrix.is_some() {
                    chummy_log::system::warn(
                        "matrix",
                        "chummyd has no terminal (standard input closed), so Matrix verifications can't be confirmed until it runs with one",
                    );
                }
            }
            Input::Matrix(FromMatrix::Ready { rooms }) => {
                // Once: Matrix says it again each time it reconnects.
                let first = self.rooms.is_empty();
                self.rooms = rooms;
                if first {
                    for r in self.rooms.clone() {
                        self.say_paused(&Conv::Matrix(r));
                    }
                }
            }
            Input::Matrix(FromMatrix::Message {
                room,
                text,
                answers,
            }) => self.matrix_message(room, text.trim(), answers),
            Input::Update(u) => self.update(&u),
            Input::Ask(AskEvent::Asked { pending }) => self.asked(&pending),
            Input::Ask(AskEvent::Closed { id, .. }) => {
                self.to_matrix(ToMatrix::Closed { ask_id: id })
            }
            Input::JobFinished(f) => self.job_finished(&f.0, &f.1),
            Input::ShuttingDown => self.accepting = false,
        }
    }

    /// A line from the terminal. The operator's `/verify` and `/quit` work
    /// even when the terminal isn't a channel.
    fn line(&mut self, text: &str) {
        if text.is_empty() {
            return;
        }
        if let Some(v) = crate::link::parse_verify(text) {
            match v {
                Ok((n, matches)) if self.matrix.is_some() => {
                    self.to_matrix(ToMatrix::Verify { n, matches })
                }
                Ok(_) => note("There's no Matrix channel."),
                Err(e) => note(&e),
            }
            return;
        }
        let parsed = chummy_channel::parse(text);
        if !self.terminal {
            if matches!(parsed, Some(Ok(Command::Quit))) {
                self.command(&self.terminal_conv(), Command::Quit);
            } else {
                note(
                    "The terminal only takes /verify and /quit: it isn't a channel in this bundle.",
                );
            }
            return;
        }
        if !self.accepting {
            note("chummyd is shutting down.");
            return;
        }
        let to = self.terminal_conv();
        match parsed {
            None => self.message(&to, text, None),
            Some(Err(e)) => note(&e),
            Some(Ok(c)) => self.command(&to, c),
        }
    }

    fn matrix_message(&mut self, room: RoomLink, text: &str, answers: Option<String>) {
        if text.is_empty() {
            return;
        }
        let to = Conv::Matrix(room);
        if !self.accepting {
            self.say(&to, "chummyd is shutting down.");
            return;
        }
        match chummy_channel::parse(text) {
            None => self.message(&to, text, answers),
            Some(Err(e)) => self.say(&to, &e),
            Some(Ok(Command::Agents | Command::Agent(_) | Command::Quit)) => self.say(
                &to,
                "That's for the terminal: each room here talks to one agent.",
            ),
            Some(Ok(c)) => self.command(&to, c),
        }
    }

    fn ensure_chat(&self, to: &Conv) {
        let _ = self.p.0.chats.ensure(to.agent(), &to.chat(), &to.title());
    }

    fn message(&mut self, to: &Conv, text: &str, answers: Option<String>) {
        // A question waiting for this conversation: this is the answer.
        let waiting: Vec<Pending> = self
            .p
            .session()
            .asks
            .pending()
            .into_iter()
            .filter(|q| self.mine(to, q) && matches!(q.ask.kind, AskKind::Question))
            .collect();
        let q = match answers {
            Some(id) => waiting.iter().find(|q| q.id == id),
            // The terminal answers them in turn; in Matrix, by replying when
            // several wait.
            None if matches!(to, Conv::Terminal(_)) || waiting.len() == 1 => waiting.first(),
            None if waiting.len() > 1 => {
                self.say(
                    to,
                    "Several questions are waiting: reply to the one you're answering.",
                );
                return;
            }
            None => None,
        };
        if let Some(q) = q {
            if let Err(e) = chummy_session::answer(&self.p, &q.id, Answer::Text(text.into())) {
                self.say(to, &e);
            }
            return;
        }
        self.ensure_chat(to);
        match chummy_session::send_message(&self.p, to.agent(), &to.chat(), text) {
            Ok(run) => {
                self.views
                    .insert(run.clone(), (to.clone(), RunView::new(run)));
            }
            Err(e) if e.contains("still working") => self.say(
                to,
                "Busy with your last message: /stop it, or wait for the reply.",
            ),
            Err(e) => self.say(to, &e),
        }
    }

    /// The run going on in a conversation, if any.
    fn running(&self, to: &Conv) -> Option<String> {
        let chat = to.chat();
        self.p
            .session()
            .runs()
            .into_iter()
            .find(|r| {
                r.agent_id.as_deref() == Some(to.agent()) && r.chat_id.as_deref() == Some(&chat)
            })
            .map(|r| r.run_id)
    }

    fn approval(&self, to: &Conv) -> Option<Pending> {
        self.p
            .session()
            .asks
            .pending()
            .into_iter()
            .find(|q| self.mine(to, q) && !matches!(q.ask.kind, AskKind::Question))
    }

    fn command(&mut self, to: &Conv, c: Command) {
        let session = self.p.session();
        let (agent, chat) = (to.agent().to_string(), to.chat());
        match c {
            Command::Stop => match self.running(to) {
                Some(r) => {
                    session.stop(&r);
                    self.say(to, "Stopping.");
                }
                None => self.say(to, "Nothing is running."),
            },
            Command::Pause => match self.running(to) {
                Some(r) => match chummy_session::pause_run(&self.p, &r) {
                    Ok(()) => self.say(to, "Pausing after this step…"),
                    Err(e) => self.say(to, &e),
                },
                None => self.say(to, "Nothing is running."),
            },
            Command::Resume => match chummy_session::resume_run(&self.p, &agent, &chat) {
                Ok(run) => {
                    self.say(to, "Carrying on where it stopped.");
                    self.views
                        .insert(run.clone(), (to.clone(), RunView::new(run)));
                }
                Err(e) => self.say(to, &e),
            },
            Command::Discard => {
                if !session.is_paused(&agent, &chat) {
                    self.say(to, "There's no paused run.");
                } else {
                    match chummy_session::discard_paused(&self.p, &agent, &chat) {
                        Ok(()) => self.say(to, "The paused run was discarded."),
                        Err(e) => self.say(to, &e),
                    }
                }
            }
            Command::New => {
                if self.running(to).is_some() {
                    self.say(to, "Busy: /stop the run first.");
                } else if session.is_paused(&agent, &chat) {
                    self.say(
                        to,
                        "This conversation has a paused run: /resume or /discard it first.",
                    );
                } else {
                    self.ensure_chat(to);
                    match self.p.0.chats.clear(&agent, &chat) {
                        Ok(()) => {
                            self.p.mark_dirty();
                            self.say(to, "Started a fresh conversation.");
                        }
                        Err(e) => self.say(to, &e),
                    }
                }
            }
            Command::Status => self.status(to),
            Command::Jobs => self.list_jobs(to),
            Command::Run(name) => self.run_job(to, &name),
            Command::Allow | Command::Deny(_) => {
                let Some(q) = self.approval(to) else {
                    self.say(to, "Nothing is waiting for an approval.");
                    return;
                };
                if !self.approvals_allowed(q.origin.agent_id.as_deref()) {
                    self.say(to, "Approvals aren't allowed here.");
                    return;
                }
                let a = match c {
                    Command::Allow => Answer::Allow,
                    Command::Deny(why) => Answer::Deny(why),
                    _ => unreachable!(),
                };
                if let Err(e) = chummy_session::answer(&self.p, &q.id, a) {
                    self.say(to, &e);
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
                self.say(to, &list.join("\n"));
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
                        self.say_paused(&self.terminal_conv());
                    }
                    None => note("There's no agent by that name: /agents lists them."),
                }
            }
            Command::Quit => {
                note("Stopping chummyd…");
                self.quit.notify_one();
            }
            Command::Help => match to {
                Conv::Terminal(_) => note(&format!(
                    "{HELP}\n/agents          the agents you can talk to\n/agent <name>    talk to another agent\n/quit            stop chummyd"
                )),
                Conv::Matrix(_) => self.say(to, &format!("```\n{HELP}\n```")),
            },
        }
    }

    fn status(&self, to: &Conv) {
        let session = self.p.session();
        let mut lines = vec![];
        for r in session.runs() {
            if r.agent_id.as_deref() != Some(to.agent()) && matches!(to, Conv::Matrix(_)) {
                continue;
            }
            let what = match (r.source, &r.chat_id) {
                ("job", _) => format!("job {}", r.title),
                (_, Some(_)) => format!("{} (conversation)", r.title),
                _ => r.title.clone(),
            };
            let secs = chummy_session::now().saturating_sub(r.started_at);
            lines.push(format!("Running: {what}, {secs}s"));
        }
        for p in session.paused() {
            if p.agent_id != to.agent() && matches!(to, Conv::Matrix(_)) {
                continue;
            }
            lines.push(format!(
                "Paused: {} ({})",
                self.agent_name(&p.agent_id),
                p.reason.as_deref().unwrap_or("paused")
            ));
        }
        let waiting = session
            .asks
            .pending()
            .iter()
            .filter(|q| matches!(to, Conv::Terminal(_)) || self.mine(to, q))
            .count();
        if waiting > 0 {
            lines.push(format!("Waiting for an answer: {waiting}"));
        }
        if lines.is_empty() {
            lines.push("Nothing is running.".into());
        }
        if let Conv::Terminal(_) = to {
            lines.push(format!("Talking to {}.", self.agent_name(&self.current)));
        }
        self.say(to, &lines.join("\n"));
    }

    fn agent_jobs(&self, agent: &str) -> Vec<chummy_scheduler::JobView> {
        self.jobs
            .overview()
            .jobs
            .into_iter()
            .filter(|j| j.job.agent == agent)
            .collect()
    }

    fn list_jobs(&self, to: &Conv) {
        let jobs = self.agent_jobs(to.agent());
        if jobs.is_empty() {
            self.say(to, &format!("{} has no jobs.", self.agent_name(to.agent())));
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
        self.say(to, &lines.join("\n"));
    }

    fn run_job(&self, to: &Conv, name: &str) {
        let want = name.to_lowercase();
        let Some(j) = self
            .agent_jobs(to.agent())
            .into_iter()
            .find(|j| j.job.id.to_lowercase() == want || j.job.name.to_lowercase() == want)
        else {
            self.say(
                to,
                "There's no job by that name for this agent: /jobs lists them.",
            );
            return;
        };
        match self.jobs.run_now(&j.job.id) {
            Ok(()) => self.say(to, &format!("Started “{}”.", j.job.name)),
            Err(e) => self.say(to, &e),
        }
    }

    fn update(&mut self, u: &Update) {
        let mut done = vec![];
        for (id, (to, v)) in self.views.iter_mut() {
            let lines = v.update(u);
            match to {
                Conv::Terminal(agent) => {
                    if !self.terminal {
                        continue;
                    }
                    let agent = self
                        .p
                        .0
                        .bundle
                        .agent(agent)
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
                Conv::Matrix(r) => {
                    if (!lines.is_empty() || v.done())
                        && let Some(tx) = &self.matrix
                    {
                        let _ = tx.send(ToMatrix::Run {
                            run_id: id.clone(),
                            room: r.room.clone(),
                            text: v.text(),
                            done: v.done(),
                        });
                    }
                }
            }
            if v.done() {
                done.push(id.clone());
            }
        }
        for id in done {
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
        let convs = self.ask_convs(q);
        let text = match &q.ask.kind {
            AskKind::Question => q.ask.text.clone(),
            kind => {
                if !self.approvals_allowed(agent) || convs.is_empty() {
                    chummy_log::system::log(
                        chummy_log::system::Level::Info,
                        "questions",
                        "Refused an approval: approvals aren't allowed over channels for this agent",
                        &[("run", &q.origin.run_id)],
                    );
                    let _ = chummy_session::answer(&self.p, &q.id, Answer::Deny(NOT_HERE.into()));
                    return;
                }
                match kind {
                    AskKind::ToolApproval { tool, arguments } => {
                        format!("wants to use {tool} with {arguments}")
                    }
                    _ => q.ask.text.clone(),
                }
            }
        };
        let question = matches!(q.ask.kind, AskKind::Question);
        for c in convs {
            match c {
                Conv::Terminal(_) => {
                    if question {
                        print(&who, &format!("{text} (your next message is the answer)"));
                    } else {
                        print(&who, &format!("{text}\n/allow, or /deny [reason]"));
                    }
                }
                Conv::Matrix(r) => {
                    let how = if question {
                        "Reply to this message to answer."
                    } else {
                        "Send /allow, or /deny [reason]."
                    };
                    self.to_matrix(ToMatrix::Question {
                        room: r.room.clone(),
                        ask_id: q.id.clone(),
                        text: format!("**{who}:** {text}\n\n{how}"),
                    });
                }
            }
        }
    }

    fn job_finished(&mut self, job: &Job, r: &RunRecord) {
        self.saver.save_now();
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
        if self.terminal {
            note(&text);
            if !r.reply.is_empty() {
                print(&self.agent_name(&job.agent), &r.reply);
            }
        }
        if let Some(room) = self.job_room(&job.id, &job.agent).map(|r| r.room.clone()) {
            self.to_matrix(ToMatrix::Notice {
                room: room.clone(),
                text,
            });
            if !r.reply.is_empty() {
                self.to_matrix(ToMatrix::Text {
                    room,
                    text: r.reply.clone(),
                });
            }
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
