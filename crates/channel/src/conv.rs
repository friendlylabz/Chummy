//! Conversations through channels, the same in the app and chummyd: what a
//! person sends (a message for the agent, an answer, or a command), how a
//! run shows as it goes, questions and approvals, and jobs' reports.
//!
//! A program keeps one [`Conversations`] for all its channels, gives it a
//! [`Hub`] (what's specific to the program) and an [`Outlet`] per channel
//! (how that channel sends), and feeds it what people send and the
//! session's updates and questions. A channel only finds conversations and
//! carries text; everything about how they behave is here.

use std::collections::HashMap;
use std::sync::Arc;

use chummy_flow::{Answer, AskKind};
use chummy_scheduler::{Job, JobView, Pending, RecordStatus, RunKind, RunRecord};
use chummy_session::{Platform, Update};

use crate::{Command, HELP, Line, RunView};

/// What an approval asked where approvals aren't allowed is answered with.
pub const NOT_HERE: &str = "approvals aren't allowed here";

/// A conversation: someone talking to an agent through a channel.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct Conv {
    /// The channel it's in (its id).
    pub channel: String,
    /// Where in the channel: a Matrix room id; empty for a console.
    pub place: String,
    pub agent: String,
    /// The agent's chat it continues.
    pub chat: String,
    /// Who's talking, as people read it: a Matrix ID, "Alice (@alice)",
    /// "Terminal". The chat's title.
    pub who: String,
    /// Who's talking, by their id in the channel (a Matrix ID, a Telegram
    /// id): what jobs report to.
    pub person: String,
    /// The operator's console (chummyd's terminal): it answers questions in
    /// turn, sees every agent's runs and jobs, and may switch agents.
    pub console: bool,
}

/// Something to send to a conversation. How it reads is up to the channel.
#[derive(Debug, Clone, PartialEq)]
pub enum Out {
    /// Chummy saying something (a notice).
    Notice(String),
    /// An agent saying something outside a run (a job's reply).
    Text { from: String, text: String },
    /// A run started: show it's working until it ends or asks something.
    Started { run_id: String },
    /// A run's response so far: what's new as lines, and the whole as one
    /// message.
    Run {
        run_id: String,
        /// The agent's name.
        from: String,
        lines: Vec<Line>,
        text: String,
        done: bool,
    },
    /// A question, or an approval, waiting for this conversation.
    Asked {
        ask_id: String,
        /// Who asks: the agent, or the job.
        from: String,
        text: String,
        /// A question (answered with a message), not an approval (`/allow`
        /// or `/deny`).
        question: bool,
    },
    /// That question doesn't wait any more.
    Closed { ask_id: String },
}

/// How a channel sends. Called from the conversations' task: it must not
/// block (queue the work).
pub trait Outlet: Send + Sync {
    fn send(&self, to: &Conv, out: Out);
}

/// How approvals (tool calls waiting for an OK) from a conversation's runs
/// are handled.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Approvals {
    /// Asked in the conversation, answered with `/allow` or `/deny`.
    Here,
    /// Refused straight away (chummyd, when not allowed over channels).
    Refuse,
    /// Left for the program's own window (the app); the conversation is
    /// told.
    Elsewhere,
}

/// What's specific to the program the conversations run in.
pub trait Hub: Send + Sync {
    type P: Platform;
    fn platform(&self) -> &Self::P;
    fn agent_name(&self, id: &str) -> String;
    /// How approvals from `agent`'s runs are handled in a channel.
    fn approvals(&self, agent: &str, channel: &str) -> Approvals;
    /// An approval no conversation shows: refused (chummyd, which has no
    /// other place to answer it), or left alone (the app).
    fn refuse_unseen_approvals(&self) -> bool;
    /// An agent's jobs, for `/jobs` and `/run`.
    fn jobs(&self, agent: &str) -> Vec<JobView>;
    fn run_job(&self, id: &str) -> Result<(), String>;
    /// Who a job reports to: (channel, who), besides consoles.
    fn job_reports(&self, job: &str) -> Vec<(String, String)>;
    /// Help lines only a console has (its own commands).
    fn console_help(&self) -> &str {
        ""
    }
    /// A run or job finished, or a chat changed: a chance to save.
    fn changed(&self, _now: bool) {}
}

/// Every channel's conversations, and the runs shown in them.
pub struct Conversations<H: Hub> {
    hub: H,
    outlets: HashMap<String, Arc<dyn Outlet>>,
    /// The conversations each channel has now, by channel.
    convs: HashMap<String, Vec<Conv>>,
    /// Runs being shown, by run id, with where.
    views: HashMap<String, (Conv, RunView)>,
    accepting: bool,
}

/// What [`Conversations::receive`] leaves to the program: commands only a
/// console has.
#[derive(Debug, Clone, PartialEq)]
pub enum Left {
    Agents,
    Agent(String),
    Quit,
}

impl<H: Hub> Conversations<H> {
    pub fn new(hub: H) -> Self {
        Self {
            hub,
            outlets: HashMap::new(),
            convs: HashMap::new(),
            views: HashMap::new(),
            accepting: true,
        }
    }

    pub fn hub(&self) -> &H {
        &self.hub
    }

    /// Adds (or replaces) how a channel sends.
    pub fn add_channel(&mut self, channel: &str, outlet: Arc<dyn Outlet>) {
        self.outlets.insert(channel.to_string(), outlet);
    }

    /// A channel is gone (stopped, turned off or deleted): its
    /// conversations stop showing runs and questions.
    pub fn remove_channel(&mut self, channel: &str) {
        self.outlets.remove(channel);
        self.convs.remove(channel);
        self.views.retain(|_, (c, _)| c.channel != channel);
    }

    /// The conversations a channel has now (its rooms, once set up). The
    /// first time, each is told about a paused run it has.
    pub fn set_convs(&mut self, channel: &str, convs: Vec<Conv>) {
        let first = self.convs.get(channel).is_none_or(|c| c.is_empty());
        self.convs.insert(channel.to_string(), convs.clone());
        if first {
            for c in &convs {
                self.say_paused(c);
            }
        }
    }

    pub fn convs(&self, channel: &str) -> &[Conv] {
        self.convs.get(channel).map(|v| v.as_slice()).unwrap_or(&[])
    }

    /// No new messages from now on (shutting down).
    pub fn stop_accepting(&mut self) {
        self.accepting = false;
    }

    fn send(&self, to: &Conv, out: Out) {
        if let Some(o) = self.outlets.get(&to.channel) {
            o.send(to, out);
        }
    }

    /// Chummy saying something in a conversation.
    pub fn say(&self, to: &Conv, text: &str) {
        self.send(to, Out::Notice(text.into()));
    }

    pub fn say_paused(&self, to: &Conv) {
        if self.hub.platform().session().is_paused(&to.agent, &to.chat) {
            self.say(
                to,
                "This conversation has a paused run: /resume carries it on, /discard throws it away.",
            );
        }
    }

    /// Something a person sent: a message for the agent, an answer, or a
    /// command. `answers`: the question it replies to, if the channel knows.
    /// Returns the console's own commands for the program to carry out.
    pub fn receive(&mut self, to: &Conv, text: &str, answers: Option<String>) -> Option<Left> {
        let text = text.trim();
        if text.is_empty() {
            return None;
        }
        if !self.accepting {
            self.say(to, "Shutting down, so no new messages are taken.");
            return None;
        }
        match crate::parse(text) {
            None => self.message(to, text, answers),
            Some(Err(e)) => self.say(to, &e),
            Some(Ok(c)) => match c {
                Command::Agents | Command::Agent(_) | Command::Quit if !to.console => self.say(
                    to,
                    "That's for the terminal: each conversation here talks to one agent.",
                ),
                Command::Agents => return Some(Left::Agents),
                Command::Agent(a) => return Some(Left::Agent(a)),
                Command::Quit => return Some(Left::Quit),
                c => self.command(to, c),
            },
        }
        None
    }

    /// Makes sure the conversation's chat exists (telling watchers when
    /// it's made).
    fn ensure_chat(&self, to: &Conv) {
        let p = self.hub.platform();
        if let Ok(Some(index)) = p.chats().ensure(&to.agent, &to.chat, &to.who) {
            chummy_session::emit_index(p, &to.agent, index);
        }
    }

    fn message(&mut self, to: &Conv, text: &str, answers: Option<String>) {
        let p = self.hub.platform();
        // A question waiting for this conversation: this is the answer.
        let waiting: Vec<Pending> = p
            .session()
            .asks
            .pending()
            .into_iter()
            .filter(|q| self.mine(to, q) && matches!(q.ask.kind, AskKind::Question))
            .collect();
        let q = match answers {
            Some(id) => waiting.iter().find(|q| q.id == id),
            // A console answers them in turn; elsewhere, by replying when
            // several wait.
            None if to.console || waiting.len() == 1 => waiting.first(),
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
            if let Err(e) = chummy_session::answer(p, &q.id, Answer::Text(text.into())) {
                self.say(to, &e);
            }
            return;
        }
        self.ensure_chat(to);
        match chummy_session::send_message(p, &to.agent, &to.chat, text) {
            Ok(run) => self.track(to, run),
            Err(e) if e.contains("still working") => self.say(
                to,
                "Busy with your last message: /stop it, or wait for the reply.",
            ),
            Err(e) => self.say(to, &e),
        }
    }

    /// Shows a run in its conversation as it goes.
    fn track(&mut self, to: &Conv, run: String) {
        self.send(
            to,
            Out::Started {
                run_id: run.clone(),
            },
        );
        self.views
            .insert(run.clone(), (to.clone(), RunView::new(run)));
    }

    /// The run going on in a conversation, if any.
    fn running(&self, to: &Conv) -> Option<String> {
        self.hub
            .platform()
            .session()
            .runs()
            .into_iter()
            .find(|r| {
                r.agent_id.as_deref() == Some(to.agent.as_str())
                    && r.chat_id.as_deref() == Some(to.chat.as_str())
            })
            .map(|r| r.run_id)
    }

    /// Where a question shows (and can be answered).
    fn ask_convs(&self, q: &Pending) -> Vec<Conv> {
        let Some(agent) = q.origin.agent_id.as_deref() else {
            return vec![];
        };
        let all = || self.convs.values().flatten();
        // A console shows the agent the question is about.
        let as_agent = |c: &Conv| Conv {
            agent: agent.to_string(),
            ..c.clone()
        };
        let mut out: Vec<Conv> = vec![];
        match q.origin.kind {
            RunKind::Chat => {
                if let Some(chat) = q.origin.chat_id.as_deref() {
                    for c in all() {
                        if c.chat == chat && (c.console || c.agent == agent) {
                            out.push(as_agent(c));
                        }
                    }
                }
            }
            RunKind::Job => {
                out.extend(all().filter(|c| c.console).map(as_agent));
                if let Some(job) = q.origin.job_id.as_deref() {
                    for (channel, who) in self.hub.job_reports(job) {
                        out.extend(
                            all()
                                .filter(|c| {
                                    c.channel == channel && c.person == who && c.agent == agent
                                })
                                .cloned(),
                        );
                    }
                }
            }
            RunKind::Test => {}
        }
        out.dedup();
        out
    }

    /// Whether a question belongs to a conversation.
    fn mine(&self, to: &Conv, q: &Pending) -> bool {
        q.origin.agent_id.as_deref() == Some(to.agent.as_str())
            && self
                .ask_convs(q)
                .iter()
                .any(|c| c.channel == to.channel && c.place == to.place)
    }

    fn approval(&self, to: &Conv) -> Option<Pending> {
        self.hub
            .platform()
            .session()
            .asks
            .pending()
            .into_iter()
            .find(|q| self.mine(to, q) && !matches!(q.ask.kind, AskKind::Question))
    }

    fn command(&mut self, to: &Conv, c: Command) {
        let p = self.hub.platform();
        let session = p.session();
        let (agent, chat) = (to.agent.as_str(), to.chat.as_str());
        match c {
            Command::Stop => match self.running(to) {
                Some(r) => {
                    session.stop(&r);
                    self.say(to, "Stopping.");
                }
                None => self.say(to, "Nothing is running."),
            },
            Command::Pause => match self.running(to) {
                Some(r) => match chummy_session::pause_run(p, &r) {
                    Ok(()) => self.say(to, "Pausing after this step…"),
                    Err(e) => self.say(to, &e),
                },
                None => self.say(to, "Nothing is running."),
            },
            Command::Resume => match chummy_session::resume_run(p, agent, chat) {
                Ok(run) => {
                    self.say(to, "Carrying on where it stopped.");
                    self.track(to, run);
                }
                Err(e) => self.say(to, &e),
            },
            Command::Discard => {
                if !session.is_paused(agent, chat) {
                    self.say(to, "There's no paused run.");
                } else {
                    match chummy_session::discard_paused(p, agent, chat) {
                        Ok(()) => self.say(to, "The paused run was discarded."),
                        Err(e) => self.say(to, &e),
                    }
                }
            }
            Command::New => {
                if self.running(to).is_some() {
                    self.say(to, "Busy: /stop the run first.");
                } else if session.is_paused(agent, chat) {
                    self.say(
                        to,
                        "This conversation has a paused run: /resume or /discard it first.",
                    );
                } else {
                    self.ensure_chat(to);
                    match p.chats().clear(agent, chat) {
                        Ok(index) => {
                            chummy_session::emit_index(p, agent, index);
                            self.hub.changed(false);
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
                let agent = q.origin.agent_id.clone().unwrap_or_default();
                if self.hub.approvals(&agent, &to.channel) != Approvals::Here {
                    self.say(to, "Approvals aren't allowed here.");
                    return;
                }
                let a = match c {
                    Command::Allow => Answer::Allow,
                    Command::Deny(why) => Answer::Deny(why),
                    _ => unreachable!(),
                };
                if let Err(e) = chummy_session::answer(p, &q.id, a) {
                    self.say(to, &e);
                }
            }
            Command::Help => {
                if to.console {
                    self.say(to, &format!("{HELP}{}", self.hub.console_help()));
                } else {
                    self.say(to, &format!("```\n{HELP}\n```"));
                }
            }
            // Handled in `receive`.
            Command::Agents | Command::Agent(_) | Command::Quit => {}
        }
    }

    fn status(&self, to: &Conv) {
        let session = self.hub.platform().session();
        let mut lines = vec![];
        for r in session.runs() {
            if !to.console && r.agent_id.as_deref() != Some(to.agent.as_str()) {
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
            if !to.console && p.agent_id != to.agent {
                continue;
            }
            lines.push(format!(
                "Paused: {} ({})",
                self.hub.agent_name(&p.agent_id),
                p.reason.as_deref().unwrap_or("paused")
            ));
        }
        let waiting = session
            .asks
            .pending()
            .iter()
            .filter(|q| to.console || self.mine(to, q))
            .count();
        if waiting > 0 {
            lines.push(format!("Waiting for an answer: {waiting}"));
        }
        if lines.is_empty() {
            lines.push("Nothing is running.".into());
        }
        if to.console {
            lines.push(format!("Talking to {}.", self.hub.agent_name(&to.agent)));
        }
        self.say(to, &lines.join("\n"));
    }

    fn list_jobs(&self, to: &Conv) {
        let jobs = self.hub.jobs(&to.agent);
        if jobs.is_empty() {
            self.say(
                to,
                &format!("{} has no jobs.", self.hub.agent_name(&to.agent)),
            );
            return;
        }
        let tz = self.hub.platform().job_time_zone();
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
            .hub
            .jobs(&to.agent)
            .into_iter()
            .find(|j| j.job.id.to_lowercase() == want || j.job.name.to_lowercase() == want)
        else {
            self.say(
                to,
                "There's no job by that name for this agent: /jobs lists them.",
            );
            return;
        };
        match self.hub.run_job(&j.job.id) {
            Ok(()) => self.say(to, &format!("Started “{}”.", j.job.name)),
            Err(e) => self.say(to, &e),
        }
    }

    /// An update from the session: runs being shown move on.
    pub fn update(&mut self, u: &Update) {
        let mut done = vec![];
        for (id, (to, v)) in self.views.iter_mut() {
            let lines = v.update(u);
            if lines.is_empty() && !v.done() {
                continue;
            }
            let out = Out::Run {
                run_id: id.clone(),
                from: self.hub.agent_name(&to.agent),
                lines,
                text: v.text(),
                done: v.done(),
            };
            if let Some(o) = self.outlets.get(&to.channel) {
                o.send(to, out);
            }
            if v.done() {
                done.push(id.clone());
            }
        }
        for id in done {
            self.views.remove(&id);
        }
        if matches!(u, Update::RunFinished(_)) {
            self.hub.changed(true);
        }
    }

    /// A question (or approval) was asked.
    pub fn asked(&mut self, q: &Pending) {
        let p = self.hub.platform();
        let agent = q.origin.agent_id.clone().unwrap_or_default();
        let from = match q.origin.kind {
            RunKind::Job => format!("Job “{}”", q.origin.title),
            _ => self.hub.agent_name(&agent),
        };
        let convs = self.ask_convs(q);
        let question = matches!(q.ask.kind, AskKind::Question);
        let (text, here) = if question {
            (q.ask.text.clone(), convs)
        } else {
            let (here, rest): (Vec<Conv>, Vec<Conv>) = convs
                .into_iter()
                .partition(|c| self.hub.approvals(&agent, &c.channel) == Approvals::Here);
            let elsewhere: Vec<&Conv> = rest
                .iter()
                .filter(|c| self.hub.approvals(&agent, &c.channel) == Approvals::Elsewhere)
                .collect();
            for c in &elsewhere {
                self.say(c, &format!("{from} is waiting for an approval in Chummy."));
            }
            if here.is_empty() && elsewhere.is_empty() {
                let shown_nowhere = rest.is_empty();
                if !shown_nowhere || self.hub.refuse_unseen_approvals() {
                    chummy_log::system::log(
                        chummy_log::system::Level::Info,
                        "questions",
                        "Refused an approval: approvals aren't allowed over channels for this agent",
                        &[("run", &q.origin.run_id)],
                    );
                    let _ = chummy_session::answer(p, &q.id, Answer::Deny(NOT_HERE.into()));
                }
                return;
            }
            let text = match &q.ask.kind {
                AskKind::ToolApproval { tool, arguments } => {
                    format!("wants to use {tool} with {arguments}")
                }
                _ => q.ask.text.clone(),
            };
            (text, here)
        };
        for c in here {
            self.send(
                &c,
                Out::Asked {
                    ask_id: q.id.clone(),
                    from: from.clone(),
                    text: text.clone(),
                    question,
                },
            );
        }
    }

    /// A question stopped waiting (answered anywhere, or withdrawn).
    pub fn closed(&self, ask_id: &str) {
        for o in self.outlets.values() {
            // Outlets that track questions forget it; the conversation
            // doesn't matter.
            o.send(
                &Conv {
                    channel: String::new(),
                    place: String::new(),
                    agent: String::new(),
                    chat: String::new(),
                    who: String::new(),
                    person: String::new(),
                    console: false,
                },
                Out::Closed {
                    ask_id: ask_id.into(),
                },
            );
        }
    }

    /// A job finished: consoles, and whoever it reports to, are told.
    pub fn job_finished(&self, job: &Job, r: &RunRecord) {
        self.hub.changed(true);
        let how = match r.status {
            RecordStatus::Completed => "finished",
            RecordStatus::Failed => "failed",
            RecordStatus::Stopped => "was stopped",
            RecordStatus::Limit => "hit a limit",
            RecordStatus::Missed => "was missed",
            RecordStatus::Skipped => "was skipped",
        };
        let name = self.hub.agent_name(&job.agent);
        let mut text = format!("“{}” ({name}) {how}.", job.name);
        if !r.note.is_empty() {
            text.push_str(&format!(" {}", r.note));
        }
        let reports = self.hub.job_reports(&job.id);
        for to in self.convs.values().flatten() {
            let reported = to.agent == job.agent
                && reports
                    .iter()
                    .any(|(ch, who)| *ch == to.channel && *who == to.person);
            if !to.console && !reported {
                continue;
            }
            self.say(to, &text);
            if !r.reply.is_empty() {
                self.send(
                    to,
                    Out::Text {
                        from: name.clone(),
                        text: r.reply.clone(),
                    },
                );
            }
        }
    }
}

/// The chat a person's conversation with an agent continues in a channel:
/// one per person (and `scope`, when one program has several channels of a
/// kind), the same across restarts. Starts with `m-`, which marks a
/// channel's chat.
pub fn channel_chat(scope: &str, user: &str) -> String {
    use sha2::{Digest, Sha256};
    let mut h = Sha256::new();
    if !scope.is_empty() {
        h.update(scope.as_bytes());
        h.update([0]);
    }
    h.update(user.as_bytes());
    let hex: String = h
        .finalize()
        .iter()
        .take(8)
        .map(|b| format!("{b:02x}"))
        .collect();
    format!("m-{hex}")
}

/// Whether a chat is a channel's (see [`channel_chat`]).
pub fn is_channel_chat(chat: &str) -> bool {
    chat.starts_with("m-")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn each_person_has_their_own_conversation() {
        let a = channel_chat("", "@alice:example.org");
        assert_eq!(a, channel_chat("", "@alice:example.org"));
        assert_ne!(a, channel_chat("", "@bob:example.org"));
        assert_ne!(a, channel_chat("ch-1", "@alice:example.org"));
        assert_eq!(a.len(), 18);
        assert!(is_channel_chat(&a));
    }
}
