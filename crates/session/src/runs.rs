//! Runs in progress.

use std::collections::VecDeque;
use std::sync::atomic::AtomicBool;
use std::sync::{Arc, Mutex};

use serde::Serialize;

/// A loop run in progress.
#[derive(Debug, Clone, Serialize)]
pub struct RunInfo {
    pub run_id: String,
    pub agent_id: Option<String>,
    /// The chat a chat run belongs to, and its title.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub chat_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub chat_title: Option<String>,
    pub title: String,
    /// "chat", "test" or "job".
    pub source: &'static str,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub job_id: Option<String>,
    pub started_at: u64,
    #[serde(skip)]
    pub cancel: Arc<AtomicBool>,
    /// Set to pause the run after its current block; only chat runs can
    /// pause.
    #[serde(skip)]
    pub pause: Option<Arc<AtomicBool>>,
    /// Whether it can pause (chat runs).
    pub pausable: bool,
    /// For a resumed run: how long it had already worked before it paused,
    /// in ms (the timer goes on from there).
    #[serde(default)]
    pub resumed_ms: u64,
    /// What the run's Pane output blocks have shown, oldest first (at most
    /// [`PANE_LINES`]).
    #[serde(skip)]
    pub pane: Arc<Mutex<VecDeque<String>>>,
    /// What a chat run has said and been told, in order: its replies (one
    /// part per stretch of replies), and questions with their answers.
    /// Saved to the chat as one response when the run ends.
    #[serde(skip)]
    pub said: Arc<Mutex<Vec<Said>>>,
}

/// One part of a chat run's response.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Said {
    /// "assistant", "question" or "answer" (as chat message roles).
    pub role: String,
    pub text: String,
}

impl Said {
    /// Adds a part, joining it to the last one when both are replies.
    pub fn push(parts: &mut Vec<Said>, role: &str, text: &str) {
        match parts.last_mut() {
            Some(last) if role == "assistant" && last.role == "assistant" => {
                last.text.push_str("\n\n");
                last.text.push_str(text);
            }
            _ => parts.push(Said {
                role: role.into(),
                text: text.into(),
            }),
        }
    }
}

/// Lines a run's pane keeps; older ones are dropped.
pub const PANE_LINES: usize = 500;
