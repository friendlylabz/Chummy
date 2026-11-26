//! What the router and the Matrix channel tell each other. Plain strings
//! only, so the router builds the same with or without Matrix.

// Without Matrix, nothing is ever sent through these.
#![cfg_attr(not(feature = "matrix"), allow(dead_code))]

/// From Matrix to the router.
#[derive(Debug, Clone)]
pub enum FromMatrix {
    /// The rooms are set up: one per agent and allowed user.
    Ready { rooms: Vec<RoomLink> },
    /// A message to act on: from the room's own user, encrypted, from a
    /// verified session, sent while chummyd was running.
    Message {
        room: RoomLink,
        text: String,
        /// The question it answers, when it was sent as a reply to one.
        answers: Option<String>,
    },
}

/// A room: the conversation between one agent and one person.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct RoomLink {
    pub agent: String,
    pub user: String,
    pub room: String,
}

/// From the router to Matrix.
#[derive(Debug)]
pub enum ToMatrix {
    /// chummyd saying something (a notice).
    Notice { room: String, text: String },
    /// An agent saying something outside a run (a job's reply).
    Text { room: String, text: String },
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
    /// The operator's answer to an emoji verification (`/verify`).
    Verify { n: Option<u32>, matches: bool },
    /// Send everything still waiting, then answer.
    Flush(tokio::sync::oneshot::Sender<()>),
}

/// The conversation (chat id) for a person's room with an agent: one per
/// person, the same across restarts.
pub fn matrix_chat(user: &str) -> String {
    use sha2::{Digest, Sha256};
    let d = Sha256::digest(user.as_bytes());
    let hex: String = d.iter().take(8).map(|b| format!("{b:02x}")).collect();
    format!("m-{hex}")
}

/// Reads the operator's `/verify [n] yes|no`. `None`: it isn't one.
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
    fn each_person_has_their_own_conversation() {
        let a = matrix_chat("@alice:example.org");
        assert_eq!(a, matrix_chat("@alice:example.org"));
        assert_ne!(a, matrix_chat("@bob:example.org"));
        assert_eq!(a.len(), 18);
        assert!(a.starts_with("m-"));
    }
}
