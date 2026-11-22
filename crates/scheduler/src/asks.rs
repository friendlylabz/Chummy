//! Questions for the user: the one way any run (a chat, a test run in the
//! editor, a scheduled job) asks a question or asks to run a tool.
//!
//! A run calls [`AskBroker::ask`] and waits. Whoever shows questions to the
//! user (the app's windows today; chummyd's connections and its notification
//! bridges later) lists [`AskBroker::pending`], hears about changes from
//! [`AskBroker::subscribe`], and answers with [`AskBroker::answer`]. Answers
//! are checked against the question: a tool approval can only be allowed or
//! refused, and a question only answered with text.
//!
//! Ids are random 128-bit values, so an id alone can't be guessed by
//! anything that wasn't shown the question.

use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};

use chummy_flow::{Answer, Ask, AskKind};
use serde::Serialize;
use tokio::sync::{broadcast, oneshot};

/// Most questions waiting at once, across every run.
pub const MAX_PENDING: usize = 100;
/// Longest text answer, in characters.
pub const MAX_ANSWER_CHARS: usize = 20_000;

/// Where a question comes from.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum RunKind {
    Chat,
    Test,
    Job,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Origin {
    pub run_id: String,
    pub kind: RunKind,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub agent_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub job_id: Option<String>,
    /// For people: the agent's or job's name.
    pub title: String,
}

/// A question waiting for an answer.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Pending {
    pub id: String,
    pub origin: Origin,
    pub ask: Ask,
    /// Unix seconds.
    pub asked_at: i64,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum AskEvent {
    Asked {
        pending: Box<Pending>,
    },
    /// Answered, withdrawn (the run stopped or timed out) or refused.
    Closed {
        id: String,
        answered: bool,
    },
}

#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum AnswerError {
    #[error("that question was already answered, or its run stopped")]
    NotWaiting,
    #[error("this needs “Allow” or “Don't allow”, not text")]
    NeedsDecision,
    #[error("this question needs a written answer")]
    NeedsText,
    #[error("the answer is too long ({MAX_ANSWER_CHARS} characters at most)")]
    TooLong,
}

struct Waiting {
    pending: Pending,
    reply: oneshot::Sender<Answer>,
}

/// Shared between every run and every place that shows questions.
#[derive(Clone)]
pub struct AskBroker {
    inner: Arc<Mutex<BTreeMap<String, Waiting>>>,
    events: broadcast::Sender<AskEvent>,
}

impl Default for AskBroker {
    fn default() -> Self {
        Self::new()
    }
}

fn new_id() -> String {
    let mut b = [0u8; 16];
    // A failing OS random source is unrecoverable for ids that must not be
    // guessed; fall back to the time only so asking still works locally.
    if getrandom::fill(&mut b).is_err() {
        let t = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0);
        b = t.to_le_bytes();
    }
    b.iter().map(|x| format!("{x:02x}")).collect()
}

fn unix_now() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

/// Removes a question if its asker stops waiting (the run was stopped, or
/// an Ask me block timed out).
struct Withdraw<'a> {
    broker: &'a AskBroker,
    id: String,
}

impl Drop for Withdraw<'_> {
    fn drop(&mut self) {
        let removed = self.broker.lock().remove(&self.id).is_some();
        if removed {
            let _ = self.broker.events.send(AskEvent::Closed {
                id: self.id.clone(),
                answered: false,
            });
        }
    }
}

impl AskBroker {
    pub fn new() -> Self {
        Self {
            inner: Arc::default(),
            events: broadcast::channel(256).0,
        }
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, BTreeMap<String, Waiting>> {
        self.inner.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// Hear about new and closed questions.
    pub fn subscribe(&self) -> broadcast::Receiver<AskEvent> {
        self.events.subscribe()
    }

    /// Asks and waits for the answer. Dropping the future withdraws the
    /// question.
    pub async fn ask(&self, origin: Origin, ask: Ask) -> Result<Answer, String> {
        let (tx, rx) = oneshot::channel();
        let pending = Pending {
            id: new_id(),
            origin,
            ask,
            asked_at: unix_now(),
        };
        let id = pending.id.clone();
        {
            let mut map = self.lock();
            if map.len() >= MAX_PENDING {
                return Err(format!(
                    "{MAX_PENDING} questions are already waiting for an answer; answer some first."
                ));
            }
            map.insert(
                id.clone(),
                Waiting {
                    pending: pending.clone(),
                    reply: tx,
                },
            );
        }
        let _ = self.events.send(AskEvent::Asked {
            pending: Box::new(pending),
        });
        let _guard = Withdraw { broker: self, id };
        rx.await
            .map_err(|_| "The question was withdrawn.".to_string())
    }

    /// Answers a waiting question.
    pub fn answer(&self, id: &str, answer: Answer) -> Result<(), AnswerError> {
        let mut map = self.lock();
        let w = map.get(id).ok_or(AnswerError::NotWaiting)?;
        match (&w.pending.ask.kind, &answer) {
            (AskKind::Question, Answer::Text(t))
            | (AskKind::ToolApproval { .. }, Answer::Deny(t))
                if t.chars().count() > MAX_ANSWER_CHARS =>
            {
                return Err(AnswerError::TooLong);
            }
            (AskKind::Question, Answer::Text(_)) => {}
            (AskKind::Question, _) => return Err(AnswerError::NeedsText),
            (AskKind::ToolApproval { .. }, Answer::Allow | Answer::Deny(_)) => {}
            (AskKind::ToolApproval { .. }, Answer::Text(_)) => {
                return Err(AnswerError::NeedsDecision);
            }
        }
        let w = map.remove(id).ok_or(AnswerError::NotWaiting)?;
        drop(map);
        let delivered = w.reply.send(answer).is_ok();
        let _ = self.events.send(AskEvent::Closed {
            id: id.to_string(),
            answered: true,
        });
        if delivered {
            Ok(())
        } else {
            Err(AnswerError::NotWaiting)
        }
    }

    /// Questions waiting now, oldest first.
    pub fn pending(&self) -> Vec<Pending> {
        let mut v: Vec<Pending> = self.lock().values().map(|w| w.pending.clone()).collect();
        v.sort_by(|a, b| a.asked_at.cmp(&b.asked_at).then(a.id.cmp(&b.id)));
        v
    }

    /// Withdraws every question from one run (when it's stopped).
    pub fn withdraw_run(&self, run_id: &str) {
        let ids: Vec<String> = self
            .lock()
            .iter()
            .filter(|(_, w)| w.pending.origin.run_id == run_id)
            .map(|(id, _)| id.clone())
            .collect();
        for id in ids {
            // Dropping the sender wakes the asker with an error.
            if self.lock().remove(&id).is_some() {
                let _ = self.events.send(AskEvent::Closed {
                    id,
                    answered: false,
                });
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn origin(run: &str) -> Origin {
        Origin {
            run_id: run.into(),
            kind: RunKind::Job,
            agent_id: Some("a".into()),
            job_id: Some("j".into()),
            title: "Digest".into(),
        }
    }

    fn question(text: &str) -> Ask {
        Ask {
            block: "ask".into(),
            label: "Ask me".into(),
            kind: AskKind::Question,
            text: text.into(),
        }
    }

    fn approval() -> Ask {
        Ask {
            block: "run".into(),
            label: "Run tool".into(),
            kind: AskKind::ToolApproval {
                tool: "fetch_page".into(),
                arguments: json!({ "url": "https://example.com" }),
            },
            text: "Run the tool?".into(),
        }
    }

    async fn wait_for_pending(b: &AskBroker, n: usize) -> Vec<Pending> {
        for _ in 0..100 {
            let p = b.pending();
            if p.len() == n {
                return p;
            }
            tokio::task::yield_now().await;
        }
        panic!("expected {n} pending");
    }

    #[tokio::test]
    async fn questions_are_answered_and_checked() {
        let b = AskBroker::new();
        let mut events = b.subscribe();
        let asker = {
            let b = b.clone();
            tokio::spawn(async move { b.ask(origin("r1"), question("Which file?")).await })
        };
        let p = wait_for_pending(&b, 1).await;
        assert_eq!(p[0].id.len(), 32);
        assert!(matches!(
            events.recv().await.unwrap(),
            AskEvent::Asked { .. }
        ));
        assert_eq!(
            b.answer(&p[0].id, Answer::Allow),
            Err(AnswerError::NeedsText)
        );
        assert_eq!(
            b.answer(&p[0].id, Answer::Text("x".repeat(MAX_ANSWER_CHARS + 1))),
            Err(AnswerError::TooLong)
        );
        b.answer(&p[0].id, Answer::Text("notes.md".into())).unwrap();
        assert_eq!(asker.await.unwrap(), Ok(Answer::Text("notes.md".into())));
        assert_eq!(
            events.recv().await.unwrap(),
            AskEvent::Closed {
                id: p[0].id.clone(),
                answered: true
            }
        );
        assert_eq!(
            b.answer(&p[0].id, Answer::Text("again".into())),
            Err(AnswerError::NotWaiting)
        );
        assert_eq!(
            b.answer("guess", Answer::Allow),
            Err(AnswerError::NotWaiting)
        );
    }

    #[tokio::test]
    async fn tool_approvals_need_a_decision() {
        let b = AskBroker::new();
        let asker = {
            let b = b.clone();
            tokio::spawn(async move { b.ask(origin("r1"), approval()).await })
        };
        let p = wait_for_pending(&b, 1).await;
        assert_eq!(
            b.answer(&p[0].id, Answer::Text("yes".into())),
            Err(AnswerError::NeedsDecision)
        );
        b.answer(&p[0].id, Answer::Deny("not now".into())).unwrap();
        assert_eq!(asker.await.unwrap(), Ok(Answer::Deny("not now".into())));
    }

    #[tokio::test]
    async fn stopped_runs_withdraw_their_questions() {
        let b = AskBroker::new();
        // Withdrawn by run id: the asker gets an error.
        let asker = {
            let b = b.clone();
            tokio::spawn(async move { b.ask(origin("r1"), question("?")).await })
        };
        wait_for_pending(&b, 1).await;
        b.withdraw_run("r1");
        assert!(asker.await.unwrap().is_err());
        assert!(b.pending().is_empty());

        // The asker gives up (stopped or timed out): the question goes.
        let asker = {
            let b = b.clone();
            tokio::spawn(async move { b.ask(origin("r2"), question("?")).await })
        };
        let p = wait_for_pending(&b, 1).await;
        asker.abort();
        let _ = asker.await;
        assert!(b.pending().is_empty());
        assert_eq!(
            b.answer(&p[0].id, Answer::Text("late".into())),
            Err(AnswerError::NotWaiting)
        );
    }

    #[tokio::test]
    async fn the_number_waiting_is_capped() {
        let b = AskBroker::new();
        let mut tasks = Vec::new();
        for i in 0..MAX_PENDING {
            let b = b.clone();
            tasks.push(tokio::spawn(async move {
                b.ask(origin(&format!("r{i}")), question("?")).await
            }));
        }
        wait_for_pending(&b, MAX_PENDING).await;
        let r = b.ask(origin("one-too-many"), question("?")).await;
        assert!(r.unwrap_err().contains("already waiting"));
        for t in tasks {
            t.abort();
        }
    }
}
