//! Where everything meets: what people send through channels (the terminal
//! and Matrix), the session's updates, questions waiting, and jobs
//! finishing. One task handles them in order.
//!
//! How conversations behave is `chummy_channel::Conversations`, shared with
//! the app; what's here is chummyd's: the terminal (a channel, and where the
//! operator confirms Matrix verifications and stops chummyd), and the
//! bundle's rules ([`DaemonHub`]).

use std::io::Write;
use std::sync::Arc;

use chummy_channel::{
    Approvals, Conv, Conversations, FromMatrix, Hub, Left, Line, Out, Outlet, ToMatrix,
};
use chummy_scheduler::{AskEvent, Job, JobView, Manager, RunRecord};
use chummy_session::Update;
use tokio::sync::mpsc::UnboundedSender;

use crate::jobs::DaemonJobs;
use crate::platform::DaemonPlatform;
use crate::saver::Saver;

/// The terminal's conversation with each agent.
pub const TERMINAL_CHAT: &str = "terminal";
/// The channels' ids.
pub const TERMINAL: &str = "terminal";
pub const MATRIX: &str = "matrix";

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

/// The terminal as a channel: everything printed.
struct TerminalOutlet;

impl Outlet for TerminalOutlet {
    fn send(&self, _to: &Conv, out: Out) {
        match out {
            Out::Notice(t) => note(&t),
            Out::Text { from, text } => print(&from, &text),
            Out::Run { from, lines, .. } => {
                for l in lines {
                    match l {
                        Line::Reply(t) => print(&from, &t),
                        Line::Notice(t) => print(&from, &format!("({})", t.trim())),
                        Line::Figures(f) => {
                            let mut out = std::io::stdout().lock();
                            let _ = writeln!(out, "  · {f}");
                        }
                        // Shown when asked, and typed by the person.
                        Line::Question(_) | Line::Answer(_) => {}
                    }
                }
            }
            Out::Asked {
                from,
                text,
                question,
                ..
            } => {
                if question {
                    print(&from, &format!("{text} (your next message is the answer)"));
                } else {
                    print(&from, &format!("{text}\n/allow, or /deny [reason]"));
                }
            }
            Out::Started { .. } | Out::Closed { .. } => {}
        }
    }
}

/// The bundle's rules for conversations.
pub struct DaemonHub {
    p: DaemonPlatform,
    jobs: Manager<DaemonJobs>,
    saver: Arc<Saver>,
}

impl Hub for DaemonHub {
    type P = DaemonPlatform;

    fn platform(&self) -> &DaemonPlatform {
        &self.p
    }

    fn agent_name(&self, id: &str) -> String {
        self.p
            .0
            .bundle
            .agent(id)
            .map(|a| a.name.clone())
            .unwrap_or_else(|| id.to_string())
    }

    /// Over channels only if the bundle allows it for the agent; otherwise
    /// refused (chummyd has no other place to answer).
    fn approvals(&self, agent: &str, _channel: &str) -> Approvals {
        if self
            .p
            .0
            .bundle
            .agent(agent)
            .is_some_and(|a| a.channel_approvals)
        {
            Approvals::Here
        } else {
            Approvals::Refuse
        }
    }

    fn refuse_unseen_approvals(&self) -> bool {
        true
    }

    fn jobs(&self, agent: &str) -> Vec<JobView> {
        self.jobs
            .overview()
            .jobs
            .into_iter()
            .filter(|j| j.job.agent == agent)
            .collect()
    }

    fn run_job(&self, id: &str) -> Result<(), String> {
        self.jobs.run_now(id)
    }

    fn job_reports(&self, job: &str) -> Vec<(String, String)> {
        self.p
            .0
            .bundle
            .channels
            .matrix
            .as_ref()
            .and_then(|m| m.report_to.get(job))
            .map(|user| vec![(MATRIX.to_string(), user.clone())])
            .unwrap_or_default()
    }

    fn console_help(&self) -> &str {
        "\n/agents          the agents you can talk to\n/agent <name>    talk to another agent\n/quit            stop chummyd"
    }

    fn changed(&self, now: bool) {
        if now {
            self.saver.save_now();
        } else {
            self.p.mark_dirty();
        }
    }
}

pub struct Router {
    convs: Conversations<DaemonHub>,
    terminal: bool,
    /// The agent the terminal talks to.
    current: String,
    /// The Matrix channel, when there is one (for verifications).
    matrix: Option<UnboundedSender<ToMatrix>>,
    /// Asked to stop (`/quit`).
    quit: Arc<tokio::sync::Notify>,
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
        let mut convs = Conversations::new(DaemonHub { p, jobs, saver });
        if terminal {
            convs.add_channel(TERMINAL, Arc::new(TerminalOutlet));
        }
        if let Some(tx) = &matrix {
            convs.add_channel(MATRIX, Arc::new(chummy_channel::MatrixOutlet(tx.clone())));
        }
        Self {
            convs,
            terminal,
            current,
            matrix,
            quit,
        }
    }

    fn agent_name(&self, id: &str) -> String {
        self.convs.hub().agent_name(id)
    }

    fn terminal_conv(&self) -> Conv {
        Conv {
            channel: TERMINAL.into(),
            place: String::new(),
            agent: self.current.clone(),
            chat: TERMINAL_CHAT.into(),
            who: "Terminal".into(),
            console: true,
        }
    }

    pub fn greet(&mut self) {
        if self.terminal {
            note(&format!(
                "Talking to {}. Type a message, or /help.",
                self.agent_name(&self.current)
            ));
            let c = self.terminal_conv();
            self.convs.set_convs(TERMINAL, vec![c]);
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
            Input::Matrix(FromMatrix::Ready { convs }) => self.convs.set_convs(MATRIX, convs),
            Input::Matrix(FromMatrix::Message {
                conv,
                text,
                answers,
            }) => {
                self.convs.receive(&conv, &text, answers);
            }
            Input::Update(u) => self.convs.update(&u),
            Input::Ask(AskEvent::Asked { pending }) => self.convs.asked(&pending),
            Input::Ask(AskEvent::Closed { id, .. }) => self.convs.closed(&id),
            Input::JobFinished(f) => self.convs.job_finished(&f.0, &f.1),
            Input::ShuttingDown => self.convs.stop_accepting(),
        }
    }

    /// A line from the terminal. The operator's `/verify` and `/quit` work
    /// even when the terminal isn't a channel.
    fn line(&mut self, text: &str) {
        if text.is_empty() {
            return;
        }
        if let Some(v) = chummy_channel::parse_verify(text) {
            match v {
                Ok((n, matches)) => match &self.matrix {
                    Some(m) => {
                        let _ = m.send(ToMatrix::Verify { n, matches });
                    }
                    None => note("There's no Matrix channel."),
                },
                Err(e) => note(&e),
            }
            return;
        }
        if !self.terminal {
            if matches!(
                chummy_channel::parse(text),
                Some(Ok(chummy_channel::Command::Quit))
            ) {
                self.left(Left::Quit);
            } else {
                note(
                    "The terminal only takes /verify and /quit: it isn't a channel in this bundle.",
                );
            }
            return;
        }
        let to = self.terminal_conv();
        if let Some(l) = self.convs.receive(&to, text, None) {
            self.left(l);
        }
    }

    /// The terminal's own commands.
    fn left(&mut self, l: Left) {
        let bundle = self.convs.hub().p.0.bundle.clone();
        match l {
            Left::Agents => {
                let list: Vec<String> = bundle
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
            Left::Agent(name) => {
                let want = name.to_lowercase();
                match bundle
                    .agents
                    .iter()
                    .find(|a| a.id.to_lowercase() == want || a.name.to_lowercase() == want)
                {
                    Some(a) => {
                        self.current = a.id.clone();
                        note(&format!("Now talking to {}.", a.name));
                        let c = self.terminal_conv();
                        self.convs.set_convs(TERMINAL, vec![c.clone()]);
                        self.convs.say_paused(&c);
                    }
                    None => note("There's no agent by that name: /agents lists them."),
                }
            }
            Left::Quit => {
                note("Stopping chummyd…");
                self.quit.notify_one();
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
