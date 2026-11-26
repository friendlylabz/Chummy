//! Where a session keeps what lasts: chats and paused runs. The app keeps
//! them in the Chummy folder (paused runs sealed with a key in the OS
//! keychain); chummyd keeps them in its sealed state.

use zeroize::Zeroizing;

use crate::chats::{ChatIndex, ChatMessage, RunSummary};

/// An agent's chats.
pub trait ChatStore: Send + Sync {
    /// An agent's chats (at least one) and which is open.
    fn index(&self, agent: &str) -> Result<ChatIndex, String>;
    /// A chat's messages.
    fn load(&self, agent: &str, chat: &str) -> Result<Vec<ChatMessage>, String>;
    /// Adds a message to a chat. Returns the updated index.
    fn append(&self, agent: &str, chat: &str, message: &ChatMessage) -> Result<ChatIndex, String>;
    /// Deletes one message. Returns the updated index, the message and
    /// where it was.
    fn delete_message(
        &self,
        agent: &str,
        chat: &str,
        message: &str,
    ) -> Result<(ChatIndex, ChatMessage, usize), String>;
    /// Gives every message of a run the run's latest figures. Returns
    /// whether any changed.
    fn set_run_summary(
        &self,
        agent: &str,
        chat: &str,
        summary: &RunSummary,
    ) -> Result<bool, String>;
}

/// Why a kept paused run can't be opened.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PausedOpenError {
    /// It isn't one this program sealed (or it was changed): the store has
    /// already removed it.
    Untrusted(String),
    /// It can't be checked right now (no keychain, say); it's left alone.
    Unavailable(String),
}

impl std::fmt::Display for PausedOpenError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            PausedOpenError::Untrusted(why) => {
                write!(
                    f,
                    "the paused run couldn't be opened ({why}), so it was removed"
                )
            }
            PausedOpenError::Unavailable(why) => f.write_str(why),
        }
    }
}

/// Paused chat runs, kept sealed so that only what this program kept itself
/// can ever be resumed. One per (agent, chat).
pub trait PausedStore: Send + Sync {
    /// Keeps a paused run's bytes, replacing any for that chat.
    fn seal(&self, agent: &str, chat: &str, bytes: &[u8]) -> Result<(), String>;
    /// A paused run's bytes, left where they are.
    fn open(&self, agent: &str, chat: &str) -> Result<Option<Zeroizing<Vec<u8>>>, PausedOpenError>;
    /// A paused run's bytes, removed so they can only be taken once.
    fn take(&self, agent: &str, chat: &str) -> Result<Option<Zeroizing<Vec<u8>>>, String>;
    fn discard(&self, agent: &str, chat: &str) -> Result<(), String>;
    /// Removes every paused run of an agent.
    fn discard_agent(&self, agent: &str);
    /// Every (agent, chat) with a paused run kept.
    fn list(&self) -> Vec<(String, String)>;
}
