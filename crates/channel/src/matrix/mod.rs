//! Matrix: end-to-end encrypted rooms with a bot account, one per person
//! allowed. People verify the bot by comparing emoji with whoever runs the
//! program. Running it needs the `matrix` feature ([`live`]).

#[cfg(feature = "matrix")]
pub mod live;

use std::sync::Arc;

use serde_json::Value;

use crate::kind::{ChannelKind, Field, Input, Setup, Start};
use crate::{ChannelHost, Handle, Person};

/// The largest file a Matrix channel may take, in MB.
pub const MAX_UPLOAD_MB: u32 = 1024;

pub struct Matrix;

impl ChannelKind for Matrix {
    fn id(&self) -> &'static str {
        "matrix"
    }

    fn label(&self) -> &'static str {
        "Matrix"
    }

    fn about(&self) -> &'static str {
        "End-to-end encrypted Matrix rooms: the program logs in as a bot account and makes a room with each \
         person you allow. Before they can talk to the agent, they verify the bot from their Matrix app, and \
         you compare the emoji."
    }

    /// A room for each agent and person, with one bot.
    fn many_agents(&self) -> bool {
        true
    }

    fn fields(&self) -> Vec<Field> {
        vec![
            Field::new("homeserver", "Homeserver", Input::Url)
                .required()
                .default("https://")
                .placeholder("https://matrix.example.org")
                .probe("/_matrix/client/versions")
                .hint("https, or plain http only on this computer or your local network."),
            Field::new("user_id", "Bot's Matrix ID", Input::Text)
                .required()
                .placeholder("@chummy-bot:example.org")
                .hint("An account of its own: not yours, and not one another channel or program uses."),
            Field::new("password", "Bot's password", Input::Secret)
                .required()
                .placeholder("The bot account's password")
                .hint(
                    "Used to log in once and to set up the bot's cross-signing; after that the session is kept, \
                     sealed, and it connects again by itself.",
                ),
            Field::new("people", "People (Matrix IDs, one per line)", Input::Lines)
                .required()
                .placeholder("@you:example.org")
                .hint("Each gets an encrypted room with the bot for each agent, and verifies it before talking to them."),
            Field::new(
                "upload_mb",
                "Largest file (MB)",
                Input::Number {
                    min: 1,
                    max: MAX_UPLOAD_MB,
                },
            )
            .default(100)
            .hint("Files people send are saved in the agent's folder, under uploads/."),
        ]
    }

    fn check(&self, s: &Setup) -> Result<(), String> {
        let bot = s.text("user_id");
        if !chummy_bundle::valid_matrix_id(bot) {
            return Err("The bot's Matrix ID isn't valid (it looks like @name:server).".into());
        }
        for u in s.lines("people") {
            if !chummy_bundle::valid_matrix_id(&u) {
                return Err(format!(
                    "“{u}” isn't a valid Matrix ID (it looks like @name:server)."
                ));
            }
            if u == bot {
                return Err("The bot can't be one of the people who talk to the agent.".into());
            }
        }
        Ok(())
    }

    fn identity(&self, s: &Setup) -> Option<String> {
        Some(format!("matrix:{}", s.text("user_id")))
    }

    fn summary(&self, s: &Setup) -> String {
        format!("{} on {}", s.text("user_id"), s.text("homeserver"))
    }

    fn reach(&self, s: &Setup) -> String {
        format!(
            "{} will be able to talk to “{}” in Matrix: each message they send starts a run of its loop, in its \
             folder, with the tools it has. {} connects to {} as {} (the Internet permission doesn't apply to \
             it, as for model connections) and makes an encrypted room for each person.\n\nThey verify the bot \
             by comparing emoji with you first.",
            s.lines("people").join(", "),
            s.agent,
            s.program,
            s.text("homeserver"),
            s.text("user_id"),
        )
    }

    fn widens(&self, old: &Setup, new: &Setup) -> bool {
        let before = old.lines("people");
        new.lines("people").iter().any(|u| !before.contains(u))
            || old.text("homeserver") != new.text("homeserver")
            || old.text("user_id") != new.text("user_id")
    }

    fn cautions(&self, s: &Setup) -> Vec<String> {
        let n = s.lines("people").len();
        vec![format!(
            "{n} {} can talk to {} in Matrix",
            if n == 1 { "person" } else { "people" },
            s.agent
        )]
    }

    fn deleting(&self) -> &'static str {
        "Its password and the bot's login and keys on this computer go too. The Matrix rooms stay, but nobody \
         can talk to the agent through them."
    }

    fn people(&self, s: &Setup, _kept: Option<&Value>) -> Vec<Person> {
        s.lines("people")
            .into_iter()
            .map(|u| Person {
                name: u.clone(),
                id: u,
                since: 0,
            })
            .collect()
    }

    fn valid_person(&self, s: &Setup, id: &str) -> bool {
        s.lines("people").iter().any(|u| u == id)
    }

    #[cfg(feature = "matrix")]
    fn start(&self, host: Arc<dyn ChannelHost>, start: Start) -> Result<Handle, String> {
        live::start_kind(host, start)
    }

    #[cfg(not(feature = "matrix"))]
    fn start(&self, _host: Arc<dyn ChannelHost>, _start: Start) -> Result<Handle, String> {
        Err("This build has no Matrix channel.".into())
    }
}
