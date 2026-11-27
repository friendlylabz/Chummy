//! What every kind of channel needs from the program it runs in
//! ([`ChannelHost`]), what it tells it ([`Event`], [`Status`]), and the
//! handle the program keeps of a running one ([`Handle`]). The same for
//! every kind of channel ([`crate::ChannelKind`]), in the app and chummyd.
//!
//! Verifying people is the same everywhere: a channel asks whoever runs
//! the program with an [`Event::Verify`] (emoji to compare, someone to let
//! in…), the program shows it its own way (a card in the app, a line in
//! chummyd's terminal) and answers with [`Handle::answer`]. Channels where
//! people join by invitation make one with [`Handle::invite`].

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

/// Something a channel asks whoever runs the program to check, then answer
/// yes or no ([`Handle::answer`]). Every kind of channel says it with
/// this, and every program shows it the same way.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Verify {
    /// Unique in the program ([`next_number`]).
    pub n: u32,
    /// Who it's about: "@alice:example.org (session ABCD)", "Alice
    /// (@alice), Telegram id 42".
    pub who: String,
    /// What to check, following who it is: "wants to verify the bot.
    /// Check that they see these emoji, in this order:".
    pub ask: String,
    /// What to compare (emoji and their names…), in order; may be empty.
    pub items: Vec<(String, String)>,
    /// The answers' words: "They match" / "They don't match", "Let them
    /// in" / "Refuse".
    pub yes: String,
    pub no: String,
    /// What saying yes lets happen, when it needs saying again before it's
    /// done (the app asks natively; chummyd prints it).
    pub confirm: Option<String>,
}

/// An invitation (a code) for someone to ask to talk to the agent.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Invite {
    pub code: String,
    /// How to use it, for whoever passes it on: "Ask them to send /start
    /// 123456 to @helper_bot in Telegram within 10 minutes."
    pub text: String,
    /// When it stops working (Unix ms).
    pub expires_at: u64,
}

/// Something waiting for, or worth telling, whoever runs the program.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Event {
    /// Something to check and answer: see [`Verify`].
    Verify(Verify),
    /// Number `n` doesn't wait any more (answered, cancelled, run out).
    Over { n: u32 },
    /// Something to tell whoever confirms.
    Note { text: String },
    /// The people who may talk to the agent now changed (those let in).
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

/// Numbers for what waits ([`Event::Verify`]): unique in the program, so
/// one number answers one thing whatever channel it's in.
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

    /// The answer to what waits (`n`; `None`: the only one): see
    /// [`Verify`].
    pub fn answer(&self, n: Option<u32>, yes: bool) {
        let _ = self.tx.send(ToChannel::Answer { n, yes });
    }

    /// A new invitation (channels where people join by invitation:
    /// [`crate::ChannelKind::invites`]).
    pub async fn invite(&self) -> Result<Invite, String> {
        invite(&self.tx).await
    }

    /// Stops letting someone in (channels that let people in themselves:
    /// [`crate::ChannelKind::removes_people`]).
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

/// [`Handle::invite`], with only the channel's sender.
pub async fn invite(tx: &UnboundedSender<ToChannel>) -> Result<Invite, String> {
    let (otx, orx) = tokio::sync::oneshot::channel();
    tx.send(ToChannel::Invite(otx))
        .map_err(|_| "The channel isn't running.".to_string())?;
    tokio::time::timeout(Duration::from_secs(10), orx)
        .await
        .map_err(|_| "The channel didn't answer.".to_string())?
        .map_err(|_| "The channel isn't running.".to_string())?
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
