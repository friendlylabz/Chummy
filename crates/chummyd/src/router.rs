//! Where everything meets: what people send through channels (the
//! terminal, Matrix and Telegram), the session's updates, questions
//! waiting, and jobs finishing. One task handles them in order.
//!
//! How conversations behave is `chummy_channel::Conversations`, shared with
//! the app; what's here is chummyd's: the terminal (a channel, and where the
//! operator confirms Matrix verifications, lets people in to Telegram bots
//! and stops chummyd), and the bundle's rules ([`DaemonHub`]).

use std::collections::BTreeMap;
use std::io::Write;
use std::sync::Arc;

use chummy_channel::{
    Approvals, ChannelOutlet, Conv, Conversations, FromChannel, Hub, Left, Line, Out, Outlet,
    ToChannel,
};
use chummy_scheduler::{AskEvent, Job, JobView, Manager, RunRecord};
use chummy_session::Update;
use tokio::sync::mpsc::UnboundedSender;

use crate::jobs::DaemonJobs;
use crate::platform::DaemonPlatform;
use crate::saver::{Kept, Saver};

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
    /// From a channel (by id).
    #[cfg_attr(not(any(feature = "matrix", feature = "telegram")), allow(dead_code))]
    Channel(String, FromChannel),
    /// Someone waits to be let in to a Telegram bot: pairing `n`.
    #[cfg_attr(not(feature = "telegram"), allow(dead_code))]
    Pairing {
        channel: String,
        n: u32,
    },
    /// Pairing (or verification) `n` doesn't wait any more.
    #[cfg_attr(not(any(feature = "matrix", feature = "telegram")), allow(dead_code))]
    Over(u32),
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
    help: String,
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
        &self.help
    }

    fn changed(&self, now: bool) {
        if now {
            self.saver.save_now();
        } else {
            self.p.mark_dirty();
        }
    }
}

/// The terminal's own commands for the channels.
#[derive(Debug, PartialEq)]
enum Op {
    /// `/pair`: a code for the current agent's Telegram bot.
    Code,
    /// `/pair [n] yes|no`.
    Answer(Option<u32>, bool),
    /// `/people`: who may talk to the current agent's bot.
    People,
    /// `/unpair <id>`.
    Remove(String),
}

fn parse_op(text: &str) -> Option<Result<Op, String>> {
    let t = text.trim();
    let (first, rest) = t.split_once(char::is_whitespace).unwrap_or((t, ""));
    let words: Vec<&str> = rest.split_whitespace().collect();
    let answer = |w: &str| match w.to_lowercase().as_str() {
        "yes" | "y" => Some(true),
        "no" | "n" => Some(false),
        _ => None,
    };
    let usage = || {
        Err("Say /pair to make a code, then /pair yes or /pair no (with its number when several wait: /pair 2 yes).".to_string())
    };
    Some(match first {
        "/pair" => match words.as_slice() {
            [] => Ok(Op::Code),
            [w] => answer(w)
                .map(|a| Op::Answer(None, a))
                .map_or_else(usage, Ok),
            [n, w] => match (n.parse::<u32>(), answer(w)) {
                (Ok(n), Some(a)) => Ok(Op::Answer(Some(n), a)),
                _ => usage(),
            },
            _ => usage(),
        },
        "/people" if words.is_empty() => Ok(Op::People),
        "/unpair" => match words.as_slice() {
            [id] => Ok(Op::Remove(id.to_string())),
            _ => Err("Say /unpair <id>, with the id /people shows.".into()),
        },
        _ => return None,
    })
}

pub struct Router {
    convs: Conversations<DaemonHub>,
    terminal: bool,
    /// The agent the terminal talks to.
    current: String,
    /// The channels running (Matrix, Telegram bots), by id.
    channels: BTreeMap<String, UnboundedSender<ToChannel>>,
    /// What each keeps (for `/people`).
    #[cfg_attr(not(feature = "telegram"), allow(dead_code))]
    kept: Kept,
    /// Pairings waiting, and their channel.
    pairings: BTreeMap<u32, String>,
    /// Asked to stop (`/quit`).
    quit: Arc<tokio::sync::Notify>,
}

impl Router {
    pub fn new(
        p: DaemonPlatform,
        jobs: Manager<DaemonJobs>,
        saver: Arc<Saver>,
        channels: BTreeMap<String, UnboundedSender<ToChannel>>,
        kept: Kept,
        quit: Arc<tokio::sync::Notify>,
    ) -> Self {
        let terminal = p.0.bundle.channels.terminal.enabled;
        let current =
            p.0.bundle
                .agents
                .first()
                .map(|a| a.id.clone())
                .unwrap_or_default();
        let mut help =
            "\n/agents          the agents you can talk to\n/agent <name>    talk to another agent"
                .to_string();
        if !p.0.bundle.channels.telegram.is_empty() {
            help.push_str("\n/pair            a code to let someone talk to this agent's Telegram bot\n/people          who may talk to it\n/unpair <id>     stop someone talking to it");
        }
        help.push_str("\n/quit            stop chummyd");
        let mut convs = Conversations::new(DaemonHub {
            p,
            jobs,
            saver,
            help,
        });
        if terminal {
            convs.add_channel(TERMINAL, Arc::new(TerminalOutlet));
        }
        for (id, tx) in &channels {
            convs.add_channel(id, Arc::new(ChannelOutlet(tx.clone())));
        }
        Self {
            convs,
            terminal,
            current,
            channels,
            kept,
            pairings: BTreeMap::new(),
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
                if !self.channels.is_empty() {
                    chummy_log::system::warn(
                        "app",
                        "chummyd has no terminal (standard input closed), so Matrix verifications can't be confirmed, nor people let in to Telegram bots, until it runs with one",
                    );
                }
            }
            Input::Channel(id, FromChannel::Ready { convs }) => self.convs.set_convs(&id, convs),
            Input::Channel(
                _,
                FromChannel::Message {
                    conv,
                    text,
                    answers,
                },
            ) => {
                self.convs.receive(&conv, &text, answers);
            }
            Input::Pairing { channel, n } => {
                self.pairings.insert(n, channel);
            }
            Input::Over(n) => {
                self.pairings.remove(&n);
            }
            Input::Update(u) => self.convs.update(&u),
            Input::Ask(AskEvent::Asked { pending }) => self.convs.asked(&pending),
            Input::Ask(AskEvent::Closed { id, .. }) => self.convs.closed(&id),
            Input::JobFinished(f) => self.convs.job_finished(&f.0, &f.1),
            Input::ShuttingDown => self.convs.stop_accepting(),
        }
    }

    /// A line from the terminal. The operator's `/verify`, `/pair`,
    /// `/people`, `/unpair` and `/quit` work even when the terminal isn't a
    /// channel.
    fn line(&mut self, text: &str) {
        if text.is_empty() {
            return;
        }
        if let Some(v) = chummy_channel::parse_verify(text) {
            match v {
                Ok((n, yes)) => match self.channels.get(MATRIX) {
                    Some(m) => {
                        let _ = m.send(ToChannel::Answer { n, yes });
                    }
                    None => note("There's no Matrix channel."),
                },
                Err(e) => note(&e),
            }
            return;
        }
        if let Some(op) = parse_op(text) {
            match op {
                Ok(op) => self.op(op),
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
                    "The terminal only takes /verify, /pair, /people, /unpair and /quit: it isn't a channel in this bundle.",
                );
            }
            return;
        }
        let to = self.terminal_conv();
        if let Some(l) = self.convs.receive(&to, text, None) {
            self.left(l);
        }
    }

    /// `/pair`, `/people`, `/unpair`: for the current agent's Telegram bot
    /// (answers go to whichever bot the pairing is for).
    fn op(&mut self, op: Op) {
        let agent = self.agent_name(&self.current);
        let id = crate::channels::telegram_id(&self.current);
        let bot = self.channels.get(&id).cloned();
        let no_bot = || {
            note(&format!(
                "{agent} has no Telegram bot in this bundle (/agent switches agents)."
            ))
        };
        match op {
            Op::Code => {
                let Some(bot) = bot else { return no_bot() };
                let (tx, rx) = tokio::sync::oneshot::channel();
                let _ = bot.send(ToChannel::NewCode(tx));
                tokio::spawn(async move {
                    match rx.await {
                        Ok(Ok(c)) => note(&format!(
                            "Pairing code for {agent}'s Telegram bot: {}\nAsk the person to send /start {} to the bot within 10 minutes; then you'll be asked here whether to let them in.",
                            c.code, c.code
                        )),
                        Ok(Err(e)) => note(&e),
                        Err(_) => note("The Telegram bot isn't running."),
                    }
                });
            }
            Op::Answer(n, yes) => {
                let n = match n {
                    Some(n) => n,
                    None if self.pairings.len() == 1 => *self.pairings.keys().next().expect("one"),
                    None if self.pairings.is_empty() => return note("No pairing is waiting."),
                    None => {
                        return note(&format!(
                            "Several pairings are waiting ({}): say which, like /pair {} yes.",
                            self.pairings
                                .keys()
                                .map(|k| k.to_string())
                                .collect::<Vec<_>>()
                                .join(", "),
                            self.pairings.keys().next().expect("some")
                        ));
                    }
                };
                match self.pairings.get(&n).and_then(|c| self.channels.get(c)) {
                    Some(ch) => {
                        let _ = ch.send(ToChannel::Answer { n: Some(n), yes });
                    }
                    None => note(&format!("There's no pairing {n} waiting.")),
                }
            }
            Op::People => {
                if bot.is_none() {
                    return no_bot();
                }
                #[cfg(feature = "telegram")]
                let people = self
                    .kept
                    .lock()
                    .unwrap_or_else(|e| e.into_inner())
                    .get(&id)
                    .map(chummy_channel::telegram::people)
                    .unwrap_or_default();
                #[cfg(not(feature = "telegram"))]
                let people: Vec<chummy_channel::Person> = vec![];
                if people.is_empty() {
                    note(&format!(
                        "Nobody may talk to {agent}'s Telegram bot yet: /pair makes a code."
                    ));
                } else {
                    let list: Vec<String> = people
                        .iter()
                        .map(|p| format!("{} (id {})", p.name, p.id))
                        .collect();
                    note(&format!(
                        "Who may talk to {agent}'s Telegram bot:\n{}\n/unpair <id> stops someone.",
                        list.join("\n")
                    ));
                }
            }
            Op::Remove(who) => {
                let Some(bot) = bot else { return no_bot() };
                let _ = bot.send(ToChannel::Remove { id: who.clone() });
                note(&format!(
                    "{who} can't talk to {agent}'s Telegram bot any more."
                ));
            }
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pairing_commands_read_right() {
        assert_eq!(parse_op("hello"), None);
        assert_eq!(parse_op("/pair"), Some(Ok(Op::Code)));
        assert_eq!(parse_op("/pair yes"), Some(Ok(Op::Answer(None, true))));
        assert_eq!(
            parse_op(" /pair 3 NO "),
            Some(Ok(Op::Answer(Some(3), false)))
        );
        assert!(matches!(parse_op("/pair maybe"), Some(Err(_))));
        assert_eq!(parse_op("/people"), Some(Ok(Op::People)));
        assert_eq!(parse_op("/unpair 42"), Some(Ok(Op::Remove("42".into()))));
        assert!(matches!(parse_op("/unpair"), Some(Err(_))));
        assert_eq!(parse_op("/pairs"), None);
    }
}
