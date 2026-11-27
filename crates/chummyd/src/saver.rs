//! Saving the state: when something changed (at most every 30 seconds),
//! after a run or job finishes, and on shutdown.

use std::collections::BTreeMap;
use std::sync::atomic::Ordering;
use std::sync::{Arc, Mutex};
use std::time::Duration;

/// What each channel keeps in the state, by channel id (a login, rooms,
/// the people let in).
pub type Kept = Arc<Mutex<BTreeMap<String, serde_json::Value>>>;

use chummy_scheduler::JobStore;

use crate::platform::DaemonPlatform;
use crate::state::StateDir;

/// How often changes are saved at most.
pub const EVERY: Duration = Duration::from_secs(30);

pub struct Saver {
    p: DaemonPlatform,
    state: StateDir,
    jobs: JobStore,
    kept: Kept,
    /// One save at a time.
    busy: Mutex<()>,
}

impl Saver {
    pub fn new(p: DaemonPlatform, state: StateDir, jobs: JobStore, kept: Kept) -> Self {
        Self {
            p,
            state,
            jobs,
            kept,
            busy: Mutex::new(()),
        }
    }

    /// Saves everything now.
    pub fn save_now(&self) {
        let _g = self.busy.lock().unwrap_or_else(|e| e.into_inner());
        let inner = &self.p.0;
        inner.dirty.store(false, Ordering::Relaxed);
        let mut s = self.state.blank();
        s.saved_at = chummy_session::now();
        s.chats = inner.chats.snapshot();
        s.paused = inner.paused.snapshot();
        s.jobs = self.jobs.snapshot();
        s.usage = inner.session.usage.snapshot();
        s.channels = self.kept.lock().unwrap_or_else(|e| e.into_inner()).clone();
        if let Err(e) = self.state.save(&s) {
            inner.dirty.store(true, Ordering::Relaxed);
            chummy_log::system::error("state", format!("Couldn't save the state: {e}"));
        }
    }

    /// Saves now if something changed.
    pub fn save_if_changed(&self) {
        if self.p.0.dirty.load(Ordering::Relaxed) {
            self.save_now();
        }
    }

    /// Saves changes every [`EVERY`] until dropped.
    pub fn every(self: &Arc<Self>) -> tokio::task::JoinHandle<()> {
        let me = self.clone();
        tokio::spawn(async move {
            let mut t = tokio::time::interval(EVERY);
            t.tick().await;
            loop {
                t.tick().await;
                me.save_if_changed();
            }
        })
    }
}
