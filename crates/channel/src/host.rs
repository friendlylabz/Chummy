//! What every kind of channel needs from the program it runs in
//! ([`ChannelHost`]), what it tells it ([`Event`], [`Status`]), and the
//! handle the program keeps of a running one ([`Handle`]). The same for
//! Matrix and Telegram, in the app and chummyd.

use std::sync::atomic::{AtomicU32, Ordering};
use std::time::Duration;

use serde::{Deserialize, Serialize};
use tokio::sync::mpsc::UnboundedSender;

use crate::link::{FromChannel, ToChannel};

/// Messages older than this when they arrive were sent while the program
/// was off: they get a note instead of a run.
pub const STALE: Duration = Duration::from_secs(10 * 60);

/// What a channel needs from the program it runs in.
pub trait ChannelHost: Send + Sync + 'static {
    fn agent_name(&self, id: &str) -> Option<String>;
    /// What the channel kept last time: see [`ChannelHost::keep`].
    fn kept(&self) -> Option<serde_json::Value>;
    /// Keeps what the channel needs next time (a login, rooms, the people
    /// paired), now. It can hold secrets: the host keeps it sealed.
    fn keep(&self, kept: serde_json::Value);
    /// A person sent something, or the conversations changed.
    fn received(&self, m: FromChannel);
    /// Something for whoever runs the program (see [`Event`]).
    fn event(&self, e: Event);
    fn status(&self, _s: Status) {}
    /// Saves a file someone sent into the agent's folder (through its jail);
    /// returns the name it got. Called off the async threads.
    fn save_upload(&self, agent: &str, name: &str, data: &[u8]) -> Result<String, String>;
}

/// Something waiting for, or worth telling, whoever runs the program.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Event {
    /// Someone's emoji (Matrix) wait to be compared: answer with
    /// [`Handle::answer`].
    Emoji {
        n: u32,
        /// "@alice:example.org (session ABCD)".
        who: String,
        /// The emoji and their names, in order.
        emoji: Vec<(String, String)>,
    },
    /// Someone sent a pairing code (Telegram) and waits to be let in:
    /// answer with [`Handle::answer`].
    Pairing {
        n: u32,
        /// "Alice (@alice, 81234567)".
        who: String,
    },
    /// Number `n` doesn't wait any more (answered, cancelled, run out).
    Over { n: u32 },
    /// Something to tell whoever confirms.
    Note { text: String },
    /// The people who may talk to the agent now (Telegram: those paired).
    People { people: Vec<Person> },
    /// The bot, as people find it ("@helper_bot").
    Account { name: String },
}

/// Someone allowed to talk to the agent.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Person {
    /// Their id in the channel (Telegram: their user id).
    pub id: String,
    /// "Alice (@alice)".
    pub name: String,
    /// When they were let in (Unix ms).
    #[serde(default)]
    pub since: u64,
}

/// The connection, as the program shows it.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(tag = "state", content = "message", rename_all = "snake_case")]
pub enum Status {
    Connecting,
    Connected,
    /// Can't connect now; trying again.
    Problem(String),
}

/// A pairing code: whoever sends it to the bot asks to be let in.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Code {
    pub code: String,
    /// When it stops working (Unix ms).
    pub expires_at: u64,
}

/// Numbers for what waits ([`Event::Emoji`], [`Event::Pairing`]): unique in
/// the program, so one number answers one thing whatever channel it's in.
pub fn next_number() -> u32 {
    static NEXT: AtomicU32 = AtomicU32::new(0);
    NEXT.fetch_add(1, Ordering::Relaxed) + 1
}

/// Now, in Unix ms.
pub fn now_ms() -> u64 {
    jiff::Timestamp::now().as_millisecond().max(0) as u64
}

/// A running channel.
pub struct Handle {
    pub(crate) tx: UnboundedSender<ToChannel>,
    pub(crate) tasks: Vec<tokio::task::JoinHandle<()>>,
}

impl Handle {
    /// Where the conversations send to it.
    pub fn sender(&self) -> UnboundedSender<ToChannel> {
        self.tx.clone()
    }

    /// The answer to what waits (`n`; `None`: the only one): emoji that
    /// match, or someone to let in.
    pub fn answer(&self, n: Option<u32>, yes: bool) {
        let _ = self.tx.send(ToChannel::Answer { n, yes });
    }

    /// A new pairing code (channels where people pair).
    pub async fn pairing_code(&self) -> Result<Code, String> {
        let (tx, rx) = tokio::sync::oneshot::channel();
        self.tx
            .send(ToChannel::NewCode(tx))
            .map_err(|_| "The channel isn't running.".to_string())?;
        tokio::time::timeout(Duration::from_secs(10), rx)
            .await
            .map_err(|_| "The channel didn't answer.".to_string())?
            .map_err(|_| "The channel isn't running.".to_string())?
    }

    /// Stops letting someone in (channels where people pair).
    pub fn remove_person(&self, id: &str) {
        let _ = self.tx.send(ToChannel::Remove { id: id.into() });
    }

    /// Sends what's still waiting (up to `wait`).
    pub async fn flush(&self, wait: Duration) {
        let (tx, rx) = tokio::sync::oneshot::channel();
        if self.tx.send(ToChannel::Flush(tx)).is_ok() {
            let _ = tokio::time::timeout(wait, rx).await;
        }
    }

    /// Stops it (what it keeps stays for next time).
    pub fn stop(&self) {
        for t in &self.tasks {
            t.abort();
        }
    }
}

impl Drop for Handle {
    fn drop(&mut self) {
        self.stop();
    }
}

/// Makes a folder only its owner can open (on Unix), if it's missing.
pub fn make_private_dir(path: &std::path::Path) -> Result<(), String> {
    if path.is_dir() {
        return Ok(());
    }
    let mut b = std::fs::DirBuilder::new();
    b.recursive(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::DirBuilderExt;
        b.mode(0o700);
    }
    b.create(path)
        .map_err(|e| format!("couldn't make {}: {e}", path.display()))
}
