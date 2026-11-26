//! What a program gives a session: its configuration, its stores, the
//! surroundings of a run, and somewhere to send updates. The app and chummyd
//! each implement [`Platform`]; everything else about running agents is in
//! this crate, so both behave the same.

use std::future::Future;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use chummy_agent::AgentConfig;
use chummy_flow::{Event, LoopFile, RunOutcome};
use chummy_log::runs::LogOptions;
use chummy_runner::Services;
use chummy_sandbox::Jail;
use chummy_tools::{Health, ToolManifest};
use serde::Serialize;

use crate::chats::{ChatIndex, ChatMessage, RunSummary};
use crate::paused::PausedInfo;
use crate::runs::RunInfo;
use crate::{ChatStore, PausedStore, Session};

/// Why a loop can't run.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LoopProblem {
    /// There's no loop with that id (or it can't be read).
    Missing,
    /// It hasn't been reviewed and trusted; its name.
    Untrusted(String),
}

pub trait Platform: Clone + Send + Sync + 'static {
    type Services: Services;
    type Config: AgentConfig + Send + Sync + 'static;

    /// The session's shared state (runs, paused runs, questions, usage).
    fn session(&self) -> &Session;
    /// The configuration now. Each run takes it once, when it starts.
    fn config(&self) -> Self::Config;
    fn chats(&self) -> &dyn ChatStore;
    fn paused_store(&self) -> &dyn PausedStore;
    /// A loop that may run, by id.
    fn runnable_loop(&self, id: &str) -> Result<LoopFile, LoopProblem>;
    /// Opens an agent's folder (checked afresh every time).
    fn open_jail(&self, path: &Path) -> Result<Jail, String>;
    /// What gets models, secrets and the tool sandbox for runs.
    fn services(&self) -> Arc<Self::Services>;
    /// The tools installed now.
    fn installed_tools(&self) -> Vec<ToolManifest>;
    /// The tool sandbox's last known health under `config`; `None`: unchecked.
    fn sandbox_health(&self, config: &Self::Config) -> Option<Health>;
    /// The IANA time zone handed to tools as `TZ`.
    fn time_zone(&self) -> Option<String>;
    /// The time zone job schedules and run notes use.
    fn job_time_zone(&self) -> jiff::tz::TimeZone {
        jiff::tz::TimeZone::system()
    }
    /// Where run logs go and how they're written; `None`: no run logs.
    fn run_logs(&self) -> Option<(PathBuf, LogOptions)>;
    /// Runs a task in the background.
    fn spawn(&self, task: impl Future<Output = ()> + Send + 'static);
    /// Tells whoever is watching (the app's windows, a channel).
    fn emit(&self, update: Update);
}

/// Something that changed, for whoever is watching.
#[derive(Clone, Debug)]
pub enum Update {
    RunStarted(RunInfo),
    RunEvent(RunEvent),
    RunFinished(RunFinished),
    RunPausing(RunPausing),
    /// A part added to a running chat run's response (a question and its
    /// answer).
    RunSaid(RunSaid),
    PausedChanged(Vec<PausedInfo>),
    ChatMessage(ChatMessageEvent),
    ChatMessageDeleted(ChatMessageDeleted),
    ChatRunUpdated(ChatRunUpdated),
    ChatsChanged(ChatsChanged),
    /// A chat's first message was answered (the app may name the chat).
    FirstReply(FirstReply),
}

#[derive(Clone, Debug, Serialize)]
pub struct RunEvent {
    pub run_id: String,
    pub event: Event,
}

#[derive(Clone, Debug, Serialize)]
pub struct RunFinished {
    pub run_id: String,
    pub agent_id: Option<String>,
    pub chat_id: Option<String>,
    pub outcome: RunOutcome,
}

#[derive(Clone, Debug, Serialize)]
pub struct RunPausing {
    pub run_id: String,
}

#[derive(Clone, Debug, Serialize)]
pub struct RunSaid {
    pub run_id: String,
    pub role: String,
    pub text: String,
}

#[derive(Clone, Debug, Serialize)]
pub struct ChatMessageEvent {
    pub agent_id: String,
    pub chat_id: String,
    pub message: ChatMessage,
}

#[derive(Clone, Debug, Serialize)]
pub struct ChatMessageDeleted {
    pub agent_id: String,
    pub chat_id: String,
    pub message_id: String,
}

#[derive(Clone, Debug, Serialize)]
pub struct ChatRunUpdated {
    pub agent_id: String,
    pub chat_id: String,
    pub summary: RunSummary,
}

#[derive(Clone, Debug, Serialize)]
pub struct ChatsChanged {
    pub agent_id: String,
    pub index: ChatIndex,
}

#[derive(Clone, Debug)]
pub struct FirstReply {
    pub agent_id: String,
    pub chat_id: String,
    pub message: String,
    pub reply: String,
}
