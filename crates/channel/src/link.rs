//! What the conversations and the Matrix channel tell each other, and how
//! the conversations' [`Out`] reads in Matrix. Plain data only, so it
//! builds without Matrix too.

use tokio::sync::mpsc::UnboundedSender;

use crate::{Conv, Out, Outlet};

/// From Matrix to the program.
#[derive(Debug, Clone)]
pub enum FromMatrix {
    /// The rooms are set up: one conversation per agent and allowed person.
    Ready { convs: Vec<Conv> },
    /// A message to act on: from the room's own person, encrypted, from a
    /// verified session, sent while the program was running.
    Message {
        conv: Conv,
        text: String,
        /// The question it answers, when it was sent as a reply to one.
        answers: Option<String>,
    },
}

/// To the Matrix channel.
#[derive(Debug)]
pub enum ToMatrix {
    /// The program saying something (a notice).
    Notice { room: String, text: String },
    /// An agent saying something outside a run (a job's reply).
    Text { room: String, text: String },
    /// A run started in a room: the bot shows it's typing until it ends
    /// (or asks something and waits).
    Started { run_id: String, room: String },
    /// A run's response so far: one message, edited as it grows.
    Run {
        run_id: String,
        room: String,
        text: String,
        done: bool,
    },
    /// A question (or approval) waiting: posted as its own message, so it
    /// can be answered by replying to it.
    Question {
        room: String,
        ask_id: String,
        text: String,
    },
    /// That question doesn't wait any more.
    Closed { ask_id: String },
    /// The answer to an emoji verification.
    Verify { n: Option<u32>, matches: bool },
    /// Send everything still waiting, then answer.
    Flush(tokio::sync::oneshot::Sender<()>),
}

/// The conversations' side of a Matrix channel.
pub struct MatrixOutlet(pub UnboundedSender<ToMatrix>);

impl Outlet for MatrixOutlet {
    fn send(&self, to: &Conv, out: Out) {
        let room = to.place.clone();
        let m = match out {
            Out::Notice(text) => ToMatrix::Notice { room, text },
            Out::Text { text, .. } => ToMatrix::Text { room, text },
            Out::Started { run_id } => ToMatrix::Started { run_id, room },
            Out::Run {
                run_id, text, done, ..
            } => ToMatrix::Run {
                run_id,
                room,
                text,
                done,
            },
            Out::Asked {
                ask_id,
                from,
                text,
                question,
            } => {
                let how = if question {
                    "Reply to this message to answer."
                } else {
                    "Send /allow, or /deny [reason]."
                };
                ToMatrix::Question {
                    room,
                    ask_id,
                    text: format!("**{from}:** {text}\n\n{how}"),
                }
            }
            Out::Closed { ask_id } => ToMatrix::Closed { ask_id },
        };
        let _ = self.0.send(m);
    }
}

/// Reads `/verify [n] yes|no` (chummyd's terminal). `None`: it isn't one.
pub fn parse_verify(text: &str) -> Option<Result<(Option<u32>, bool), String>> {
    let rest = text.trim().strip_prefix("/verify")?;
    if !(rest.is_empty() || rest.starts_with(char::is_whitespace)) {
        return None;
    }
    let words: Vec<&str> = rest.split_whitespace().collect();
    let usage = || {
        Err("Say /verify yes or /verify no (with its number when several are waiting: /verify 2 yes).".to_string())
    };
    let answer = |w: &str| match w.to_lowercase().as_str() {
        "yes" | "y" => Some(true),
        "no" | "n" => Some(false),
        _ => None,
    };
    Some(match words.as_slice() {
        [w] => match answer(w) {
            Some(a) => Ok((None, a)),
            None => usage(),
        },
        [n, w] => match (n.parse::<u32>(), answer(w)) {
            (Ok(n), Some(a)) => Ok((Some(n), a)),
            _ => usage(),
        },
        _ => usage(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn verify_answers_read_right() {
        assert_eq!(parse_verify("hello"), None);
        assert_eq!(parse_verify("/verifyx yes"), None);
        assert_eq!(parse_verify("/verify yes"), Some(Ok((None, true))));
        assert_eq!(parse_verify(" /verify 2 NO "), Some(Ok((Some(2), false))));
        assert!(matches!(parse_verify("/verify"), Some(Err(_))));
        assert!(matches!(parse_verify("/verify maybe"), Some(Err(_))));
        assert!(matches!(parse_verify("/verify x yes"), Some(Err(_))));
    }

    #[test]
    fn questions_read_as_how_to_answer() {
        let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
        let to = Conv {
            channel: "m".into(),
            place: "!r:hs".into(),
            agent: "a".into(),
            chat: "m-1".into(),
            who: "@p:hs".into(),
            console: false,
        };
        MatrixOutlet(tx).send(
            &to,
            Out::Asked {
                ask_id: "q1".into(),
                from: "Helper".into(),
                text: "wants to use fetch".into(),
                question: false,
            },
        );
        match rx.try_recv().unwrap() {
            ToMatrix::Question { room, ask_id, text } => {
                assert_eq!((room.as_str(), ask_id.as_str()), ("!r:hs", "q1"));
                assert!(text.starts_with("**Helper:** wants to use fetch"));
                assert!(text.contains("/allow"));
            }
            m => panic!("{m:?}"),
        }
    }
}
