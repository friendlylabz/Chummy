//! Where everything meets: what people send through channels (the
//! terminal, and the bundle's channels of any kind), the session's
//! updates, questions waiting, and jobs finishing. One task handles them in
//! order.
//!
//! How conversations behave is `chummy_channel::Conversations`, shared with
//! the app; what's here is chummyd's: the terminal (a channel, and where the
//! operator verifies what channels ask with `/verify`, makes invitations
//! with `/invite`, sees and removes people, and stops chummyd), and the
//! bundle's rules ([`DaemonHub`]). None of it names a kind of channel.

use std::collections::BTreeMap;
use std::io::Write;
use std::sync::Arc;

use chummy_bundle::BundleChannel;
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
/// The terminal's channel id (the bundle's channels have their own).
pub const TERMINAL: &str = "terminal";

pub enum Input {
    /// A line typed in the terminal.
    Line(String),
    /// The terminal closed.
    Eof,
    /// From a channel (by id).
    Channel(String, FromChannel),
    /// A channel asks something to verify: number `n`.
    Pending {
        channel: String,
        n: u32,
    },
    /// Number `n` doesn't wait any more.
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

    /// In a channel only if the bundle lets it take approvals; otherwise
    /// refused (chummyd has no other place to answer).
    fn approvals(&self, _agent: &str, channel: &str) -> Approvals {
        let ch = &self.p.0.bundle.channels;
        let here = if channel == TERMINAL {
            ch.terminal.approvals
        } else {
            ch.list.iter().any(|c| c.id == channel && c.approvals)
        };
        if here {
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
            .list
            .iter()
            .filter_map(|c| Some((c.id.clone(), c.reports.get(job)?.clone())))
            .collect()
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

/// The terminal's own commands for the channels, whatever their kind.
#[derive(Debug, PartialEq)]
enum Op {
    /// `/verify [n] yes|no`: what a channel asks.
    Answer(Option<u32>, bool),
    /// `/invite [channel]`: an invitation for the current agent's channel.
    Invite(String),
    /// `/people`: who may talk to the current agent, by channel.
    People,
    /// `/remove <id>`.
    Remove(String),
}

fn parse_op(text: &str) -> Option<Result<Op, String>> {
    if let Some(v) = chummy_channel::parse_verify(text) {
        return Some(v.map(|(n, yes)| Op::Answer(n, yes)));
    }
    let t = text.trim();
    let (first, rest) = t.split_once(char::is_whitespace).unwrap_or((t, ""));
    let rest = rest.trim();
    Some(match first {
        "/invite" => Ok(Op::Invite(rest.to_string())),
        "/people" if rest.is_empty() => Ok(Op::People),
        "/remove" => match rest.split_whitespace().collect::<Vec<_>>().as_slice() {
            [id] => Ok(Op::Remove(id.to_string())),
            _ => Err("Say /remove <id>, with the id /people shows.".into()),
        },
        _ => return None,
    })
}

pub struct Router {
    convs: Conversations<DaemonHub>,
    terminal: bool,
    /// The agent the terminal talks to.
    current: String,
    /// The channels running, by id.
    channels: BTreeMap<String, UnboundedSender<ToChannel>>,
    /// What each keeps (for `/people`).
    kept: Kept,
    /// What channels ask, waiting, and their channel.
    pending: BTreeMap<u32, String>,
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
        if !p.0.bundle.channels.list.is_empty() {
            help.push_str("\n/verify [n] yes  answer what a channel asks (no: refuse)\n/invite [name]   an invitation to this agent's channel\n/people          who may talk to this agent, by channel\n/remove <id>     stop someone talking to it");
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
            pending: BTreeMap::new(),
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
            person: "terminal".into(),
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
                        "chummyd has no terminal (standard input closed), so what channels ask (verifications, people to let in) can't be answered until it runs with one",
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
            Input::Pending { channel, n } => {
                self.pending.insert(n, channel);
            }
            Input::Over(n) => {
                self.pending.remove(&n);
            }
            Input::Update(u) => self.convs.update(&u),
            Input::Ask(AskEvent::Asked { pending }) => self.convs.asked(&pending),
            Input::Ask(AskEvent::Closed { id, .. }) => self.convs.closed(&id),
            Input::JobFinished(f) => self.convs.job_finished(&f.0, &f.1),
            Input::ShuttingDown => self.convs.stop_accepting(),
        }
    }

    /// A line from the terminal. The operator's `/verify`, `/invite`,
    /// `/people`, `/remove` and `/quit` work even when the terminal isn't a
    /// channel.
    fn line(&mut self, text: &str) {
        if text.is_empty() {
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
                    "The terminal only takes /verify, /invite, /people, /remove and /quit: it isn't a channel in this bundle.",
                );
            }
            return;
        }
        let to = self.terminal_conv();
        if let Some(l) = self.convs.receive(&to, text, None) {
            self.left(l);
        }
    }

    /// The current agent's channels in the bundle.
    fn agent_channels(&self) -> Vec<BundleChannel> {
        self.convs
            .hub()
            .p
            .0
            .bundle
            .channels
            .list
            .iter()
            .filter(|c| c.agents.contains(&self.current) && self.channels.contains_key(&c.id))
            .cloned()
            .collect()
    }

    /// Who may talk through a channel, as its kind says.
    fn people(&self, c: &BundleChannel) -> Vec<chummy_channel::Person> {
        let Some(k) = chummy_channel::kind(&c.kind) else {
            return vec![];
        };
        let agent = crate::channels::agent_names(&self.convs.hub().p, &c.agents);
        let options = chummy_channel::kind::tidy(k, &c.options);
        let secrets = chummy_channel::kind::secrets_of(&c.secrets);
        let kept = self
            .kept
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .get(&c.id)
            .cloned();
        k.people(
            &chummy_channel::Setup {
                agent: &agent,
                options: &options,
                secrets: &secrets,
                program: "chummyd",
            },
            kept.as_ref(),
        )
    }

    /// `/verify`, `/invite`, `/people`, `/remove`: the same for every kind
    /// of channel.
    fn op(&mut self, op: Op) {
        let agent = self.agent_name(&self.current);
        match op {
            Op::Answer(n, yes) => {
                let n = match n {
                    Some(n) => n,
                    None if self.pending.len() == 1 => *self.pending.keys().next().expect("one"),
                    None if self.pending.is_empty() => {
                        return note("Nothing is waiting to be verified.");
                    }
                    None => {
                        let all: Vec<String> = self.pending.keys().map(|k| k.to_string()).collect();
                        return note(&format!(
                            "Several are waiting ({}): say which, like /verify {} yes.",
                            all.join(", "),
                            all[0]
                        ));
                    }
                };
                match self.pending.get(&n).and_then(|c| self.channels.get(c)) {
                    Some(ch) => {
                        let _ = ch.send(ToChannel::Answer { n: Some(n), yes });
                    }
                    None => note(&format!("Nothing is waiting as {n}.")),
                }
            }
            Op::Invite(name) => {
                let inviting: Vec<BundleChannel> = self
                    .agent_channels()
                    .into_iter()
                    .filter(|c| {
                        chummy_channel::kind(&c.kind).is_some_and(|k| k.invites().is_some())
                    })
                    .filter(|c| name.is_empty() || c.name.eq_ignore_ascii_case(&name))
                    .collect();
                let c = match inviting.as_slice() {
                    [c] => c.clone(),
                    [] => {
                        return note(&format!(
                            "None of {agent}'s channels{} takes invitations (/agent switches agents).",
                            if name.is_empty() {
                                String::new()
                            } else {
                                format!(" called “{name}”")
                            }
                        ));
                    }
                    several => {
                        let names: Vec<&str> = several.iter().map(|c| c.name.as_str()).collect();
                        return note(&format!(
                            "Say which channel: /invite {}",
                            names.join(", /invite ")
                        ));
                    }
                };
                let tx = self.channels.get(&c.id).cloned().expect("running");
                tokio::spawn(async move {
                    match chummy_channel::invite(&tx).await {
                        Ok(i) => note(&format!(
                            "[{}] Invitation code: {}\n{}",
                            c.name, i.code, i.text
                        )),
                        Err(e) => note(&format!("[{}] {e}", c.name)),
                    }
                });
            }
            Op::People => {
                let chans = self.agent_channels();
                if chans.is_empty() {
                    return note(&format!(
                        "{agent} has no channels in this bundle (/agent switches agents)."
                    ));
                }
                let mut out = vec![];
                for c in &chans {
                    let people = self.people(c);
                    if people.is_empty() {
                        out.push(format!("[{}] Nobody yet.", c.name));
                    } else {
                        for p in people {
                            out.push(format!("[{}] {} (id {})", c.name, p.name, p.id));
                        }
                    }
                }
                note(&format!("Who may talk to {agent}:\n{}", out.join("\n")));
            }
            Op::Remove(who) => {
                let found: Vec<BundleChannel> = self
                    .agent_channels()
                    .into_iter()
                    .filter(|c| {
                        chummy_channel::kind(&c.kind).is_some_and(|k| k.removes_people())
                            && self.people(c).iter().any(|p| p.id == who)
                    })
                    .collect();
                if found.is_empty() {
                    return note(&format!(
                        "{who} isn't someone /remove can stop talking to {agent} (/people lists them; for some channels, change who's allowed in the bundle)."
                    ));
                }
                for c in found {
                    if let Some(tx) = self.channels.get(&c.id) {
                        let _ = tx.send(ToChannel::Remove { id: who.clone() });
                    }
                    note(&format!(
                        "[{}] {who} can't talk to {agent} there any more.",
                        c.name
                    ));
                }
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
    fn channel_commands_read_right() {
        assert_eq!(parse_op("hello"), None);
        assert_eq!(parse_op("/verify yes"), Some(Ok(Op::Answer(None, true))));
        assert_eq!(
            parse_op(" /verify 3 NO "),
            Some(Ok(Op::Answer(Some(3), false)))
        );
        assert!(matches!(parse_op("/verify maybe"), Some(Err(_))));
        assert_eq!(parse_op("/invite"), Some(Ok(Op::Invite(String::new()))));
        assert_eq!(
            parse_op("/invite Phone"),
            Some(Ok(Op::Invite("Phone".into())))
        );
        assert_eq!(parse_op("/people"), Some(Ok(Op::People)));
        assert_eq!(parse_op("/remove 42"), Some(Ok(Op::Remove("42".into()))));
        assert!(matches!(parse_op("/remove"), Some(Err(_))));
        assert_eq!(parse_op("/verified"), None);
    }
}
