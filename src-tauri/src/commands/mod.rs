//! IPC commands. Every command must also be listed in `build.rs` and allowed in
//! the capability file of each window that may call it.
//!
//! Errors cross to the UI as plain, user-readable strings.

pub mod agents;
pub mod asks;
pub mod chat;
pub mod general;
pub mod jobs;
pub mod local;
pub mod loops;
pub mod providers;
pub mod tools;

pub type CmdResult<T> = Result<T, String>;

pub(crate) fn err(e: impl std::fmt::Display) -> String {
    e.to_string()
}

pub(crate) fn now() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}
