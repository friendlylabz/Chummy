//! What the conversations and a channel (Matrix, Telegram) tell each
//! other. Plain data only, so it builds without any channel too.

use tokio::sync::mpsc::UnboundedSender;

use crate::{Conv, Invite, Out, Outlet};

/// From a channel to the program.
#[derive(Debug, Clone)]
pub enum FromChannel {
    /// The conversations it has now: one per agent and allowed person.
    Ready { convs: Vec<Conv> },
    /// A message to act on, from someone allowed, sent while the program
    /// was running.
    Message {
        conv: Conv,
        text: String,
        /// The question it answers, when it was sent as a reply to one.
        answers: Option<String>,
    },
}

/// To a channel.
#[derive(Debug)]
pub enum ToChannel {
    /// The program saying something (a notice).
    Notice { place: String, text: String },
    /// An agent saying something outside a run (a job's reply).
    Text { place: String, text: String },
    /// A run started: the bot shows it's typing until it ends (or asks
    /// something and waits).
    Started { run_id: String, place: String },
    /// A run's response so far (the whole of it): shown as it grows.
    Run {
        run_id: String,
        place: String,
        text: String,
        done: bool,
    },
    /// A question (or approval) waiting: posted as its own message, so it
    /// can be answered by replying to it (or with buttons).
    Question {
        place: String,
        ask_id: String,
        /// Who asks.
        from: String,
        text: String,
        /// A question (answered with a message), not an approval.
        question: bool,
    },
    /// That question doesn't wait any more.
    Closed { ask_id: String },
    /// The answer to what waits for the program ([`crate::Verify`]).
    Answer { n: Option<u32>, yes: bool },
    /// A new invitation, for channels where people join by one.
    Invite(tokio::sync::oneshot::Sender<Result<Invite, String>>),
    /// Stop letting someone in (by their id).
    Remove { id: String },
    /// Send everything still waiting, then answer.
    Flush(tokio::sync::oneshot::Sender<()>),
}

/// The conversations' side of a channel.
pub struct ChannelOutlet(pub UnboundedSender<ToChannel>);

impl Outlet for ChannelOutlet {
    fn send(&self, to: &Conv, out: Out) {
        let place = to.place.clone();
        let m = match out {
            Out::Notice(text) => ToChannel::Notice { place, text },
            Out::Text { text, .. } => ToChannel::Text { place, text },
            Out::Started { run_id } => ToChannel::Started { run_id, place },
            Out::Run {
                run_id, text, done, ..
            } => ToChannel::Run {
                run_id,
                place,
                text,
                done,
            },
            Out::Asked {
                ask_id,
                from,
                text,
                question,
            } => ToChannel::Question {
                place,
                ask_id,
                from,
                text,
                question,
            },
            Out::Closed { ask_id } => ToChannel::Closed { ask_id },
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
    fn questions_carry_who_asks() {
        let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
        let to = Conv {
            channel: "m".into(),
            place: "!r:hs".into(),
            agent: "a".into(),
            chat: "m-1".into(),
            who: "@p:hs".into(),
            person: "@p:hs".into(),
            console: false,
        };
        ChannelOutlet(tx).send(
            &to,
            Out::Asked {
                ask_id: "q1".into(),
                from: "Helper".into(),
                text: "wants to use fetch".into(),
                question: false,
            },
        );
        match rx.try_recv().unwrap() {
            ToChannel::Question {
                place,
                ask_id,
                from,
                question,
                ..
            } => {
                assert_eq!((place.as_str(), ask_id.as_str()), ("!r:hs", "q1"));
                assert_eq!(from, "Helper");
                assert!(!question);
            }
            m => panic!("{m:?}"),
        }
    }
}
