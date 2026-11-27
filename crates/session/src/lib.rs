//! Chummy's conversations with agents, shared by the app and chummyd: chats,
//! runs (starting, stopping, pausing and resuming them as one response),
//! jobs, and answering the questions runs ask.
//!
//! A program implements [`Platform`] (its configuration, its stores, the
//! surroundings of a run, and where updates go) and keeps one [`Session`].
//! Everything about how a conversation behaves is here, so it behaves the
//! same in both programs.

pub mod chats;
pub mod paused;
mod platform;
mod run;
mod runs;
mod store;

use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

use chummy_runner::usage::UsageLedger;
use chummy_scheduler::AskBroker;

pub use chats::{ChatIndex, ChatMessage, ChatMeta, FileChats, MemoryChats, RunSummary};
pub use paused::{PausedInfo, PausedRun};
pub use platform::{
    ChatMessageDeleted, ChatMessageEvent, ChatRunUpdated, ChatsChanged, FirstReply, LoopProblem,
    Platform, RunEvent, RunFinished, RunPausing, RunSaid, Update,
};
pub use run::{
    active_chat, answer, append, discard_paused, emit_index, host, msg_id, pause_run, prepare,
    resume_run, run_job, send_message, snapshot, test_run,
};
pub use runs::{PANE_LINES, RunInfo, Said};
pub use store::{ChatStore, PausedOpenError, PausedStore};

/// How many finished runs are kept for each agent (see [`Session::recent`]).
pub const RECENT_PER_AGENT: usize = 3;

/// A run that finished lately.
#[derive(Debug, Clone, serde::Serialize)]
pub struct RecentRun {
    #[serde(flatten)]
    pub info: RunInfo,
    /// Unix seconds.
    pub finished_at: u64,
    /// "completed", "failed", "stopped", "limit" or "paused" ("ended" when
    /// it ended before it could say).
    pub status: String,
    /// What its Pane output blocks showed, oldest first.
    #[serde(skip)]
    pub pane: Vec<String>,
}

/// What a program's conversations share: the runs going on, the paused
/// runs, the questions waiting, and the day's model use.
pub struct Session {
    runs: Mutex<HashMap<String, RunInfo>>,
    /// Runs that finished lately, newest first: [`RECENT_PER_AGENT`] per
    /// agent.
    recent: Mutex<std::collections::VecDeque<RecentRun>>,
    paused: Mutex<paused::Paused>,
    /// Questions and tool approvals waiting for the user, from every run.
    pub asks: AskBroker,
    /// Tokens used per model connection per day, for daily limits.
    pub usage: Arc<UsageLedger>,
    /// Set when the program shuts down: pausing chat runs may then cut a
    /// model call short (it's asked again on resuming).
    hurry: AtomicBool,
}

impl Session {
    pub fn new(usage: Arc<UsageLedger>) -> Self {
        Self {
            runs: Mutex::new(HashMap::new()),
            recent: Mutex::new(Default::default()),
            paused: Mutex::new(HashMap::new()),
            asks: AskBroker::new(),
            usage,
            hurry: AtomicBool::new(false),
        }
    }

    /// For shutting down: every chat run pauses as soon as it can (cutting
    /// a model call short if nothing in its block would happen twice), and
    /// every other run (jobs, test runs) stops. Returns how many runs there
    /// were.
    pub fn pause_all_for_shutdown(&self) -> usize {
        self.hurry.store(true, Ordering::Relaxed);
        let runs = lock(&self.runs);
        for r in runs.values() {
            match &r.pause {
                Some(p) => p.store(true, Ordering::Relaxed),
                None => {
                    r.cancel.store(true, Ordering::Relaxed);
                    self.asks.withdraw_run(&r.run_id);
                }
            }
        }
        runs.len()
    }

    /// Whether the program is shutting down (see
    /// [`Session::pause_all_for_shutdown`]).
    pub fn shutting_down(&self) -> bool {
        self.hurry.load(Ordering::Relaxed)
    }

    /// The runs going on.
    pub fn runs(&self) -> Vec<RunInfo> {
        lock(&self.runs).values().cloned().collect()
    }

    pub fn run_ids(&self) -> Vec<String> {
        lock(&self.runs).keys().cloned().collect()
    }

    pub fn run_count(&self) -> usize {
        lock(&self.runs).len()
    }

    /// What a run's Pane output blocks have shown, newest first: a running
    /// run's so far, or a recent one's (empty for one that never used one,
    /// or finished too long ago).
    pub fn pane(&self, run_id: &str) -> Vec<String> {
        let pane = lock(&self.runs).get(run_id).map(|r| r.pane.clone());
        match pane {
            Some(p) => lock(&p).iter().rev().cloned().collect(),
            None => lock(&self.recent)
                .iter()
                .find(|r| r.info.run_id == run_id)
                .map(|r| r.pane.iter().rev().cloned().collect())
                .unwrap_or_default(),
        }
    }

    /// Runs that finished lately, newest first: [`RECENT_PER_AGENT`] for
    /// each agent (and as many test runs without one).
    pub fn recent(&self) -> Vec<RecentRun> {
        lock(&self.recent).iter().cloned().collect()
    }

    /// A run ended: it's kept among the recent ones. A paused run isn't
    /// (it's among the paused runs until it's resumed, as the same run, or
    /// discarded); a resumed run that ends replaces what was kept of it.
    fn ended(&self, info: RunInfo, status: String) {
        if status == "paused" {
            return;
        }
        let pane = lock(&info.pane).iter().cloned().collect();
        let mut recent = lock(&self.recent);
        recent.retain(|r| r.info.run_id != info.run_id);
        recent.push_front(RecentRun {
            finished_at: now(),
            status,
            pane,
            info,
        });
        let mut seen: HashMap<Option<String>, usize> = HashMap::new();
        recent.retain(|r| {
            let n = seen.entry(r.info.agent_id.clone()).or_default();
            *n += 1;
            *n <= RECENT_PER_AGENT
        });
    }

    /// Stops a run (after its current block), and withdraws its questions.
    pub fn stop(&self, run_id: &str) {
        if let Some(r) = lock(&self.runs).get(run_id) {
            r.cancel.store(true, Ordering::Relaxed);
        }
        self.asks.withdraw_run(run_id);
    }

    /// Stops every run of an agent.
    pub fn stop_agent(&self, agent_id: &str) {
        for r in lock(&self.runs).values() {
            if r.agent_id.as_deref() == Some(agent_id) {
                r.cancel.store(true, Ordering::Relaxed);
                self.asks.withdraw_run(&r.run_id);
            }
        }
    }

    /// Stops every run (when quitting).
    pub fn stop_all(&self) {
        for r in lock(&self.runs).values() {
            r.cancel.store(true, Ordering::Relaxed);
            self.asks.withdraw_run(&r.run_id);
        }
    }

    /// What, if anything, is using an agent right now, in words. With
    /// `for_chat`, only a test run counts: an agent's chats and jobs run
    /// side by side (a job is like a chat in the background), but a test run
    /// in the loop editor has the agent to itself. Without it, any run
    /// counts (to start a test run).
    pub fn agent_busy_with(&self, agent_id: &str, for_chat: bool) -> Option<String> {
        lock(&self.runs)
            .values()
            .filter(|r| !for_chat || r.source == "test")
            .find(|r| r.agent_id.as_deref() == Some(agent_id))
            .map(|r| match r.source {
                "job" => format!("the job “{}”", r.title),
                "test" => "a test run in the loop editor".to_string(),
                _ => match &r.chat_title {
                    Some(t) => format!("the chat “{t}”"),
                    None => "a chat".to_string(),
                },
            })
    }

    /// Whether a job may start for an agent now: jobs run alongside the
    /// agent's chats and other jobs; only a test run holds them back.
    pub fn job_may_start(&self, agent_id: &str) -> bool {
        !lock(&self.runs)
            .values()
            .any(|r| r.agent_id.as_deref() == Some(agent_id) && r.source == "test")
    }

    /// Whether a chat is working on a message.
    pub fn chat_busy(&self, chat_id: &str) -> bool {
        lock(&self.runs)
            .values()
            .any(|r| r.chat_id.as_deref() == Some(chat_id))
    }
}

/// Locks a mutex, recovering the data if a previous holder panicked.
pub fn lock<T>(m: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|e| e.into_inner())
}

/// Unix time in seconds.
pub fn now() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}
