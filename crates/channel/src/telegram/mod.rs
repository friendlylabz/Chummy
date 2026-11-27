//! Telegram: a bot (from @BotFather) for the agent, talked to in private
//! chats. Not end-to-end encrypted. People join with an invitation code
//! made in the program, and are let in only after whoever runs it says
//! yes. Running it needs the `telegram` feature ([`live`]).

#[cfg(feature = "telegram")]
pub mod live;

use std::sync::Arc;

use serde_json::Value;

use crate::kind::{ChannelKind, Field, Input, Setup, Start};
use crate::{ChannelHost, Handle, Person};

/// Telegram's own Bot API server.
pub const DEFAULT_API: &str = "https://api.telegram.org";

/// The largest file a bot can download from Telegram's Bot API, in MB.
pub const MAX_UPLOAD_MB: u32 = 20;

pub struct Telegram;

/// Whether a token looks like one from @BotFather (`123456:ABC-def…`).
pub fn valid_token(token: &str) -> bool {
    match token.split_once(':') {
        Some((id, secret)) => {
            (1..=20).contains(&id.len())
                && id.bytes().all(|b| b.is_ascii_digit())
                && (20..=100).contains(&secret.len())
                && secret
                    .bytes()
                    .all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-')
        }
        None => false,
    }
}

/// A bot's id: its token's first part (not a secret).
pub fn bot_id(token: &str) -> Option<&str> {
    token.split_once(':').map(|(id, _)| id)
}

/// The people let in, from what a Telegram channel kept.
pub fn kept_people(kept: &Value) -> Vec<Person> {
    kept.get("people")
        .and_then(|p| serde_json::from_value(p.clone()).ok())
        .unwrap_or_default()
}

impl ChannelKind for Telegram {
    fn id(&self) -> &'static str {
        "telegram"
    }

    fn label(&self) -> &'static str {
        "Telegram"
    }

    fn about(&self) -> &'static str {
        "A Telegram bot for the agent: people you let in talk to it in a private chat. Not end-to-end \
         encrypted: Telegram's servers can read these chats. People ask to talk to the agent with a code you \
         make, and you're asked before anyone is let in."
    }

    fn fields(&self) -> Vec<Field> {
        vec![
            Field::new("token", "Bot token", Input::Secret)
                .required()
                .placeholder("123456:ABC-def…")
                .hint(
                    "Make a bot with @BotFather in Telegram (/newbot) and paste the token it gives. It's the bot's \
                     whole login. Use a bot of its own, not one another channel or program uses.",
                ),
            Field::new("stream", "Show answers as they're written", Input::Toggle)
                .default(true)
                .hint("Off: each answer comes when it's done."),
            Field::new(
                "upload_mb",
                "Largest file (MB)",
                Input::Number {
                    min: 1,
                    max: MAX_UPLOAD_MB,
                },
            )
            .default(MAX_UPLOAD_MB)
            .hint("Files people send are saved in the agent's folder, under uploads/. Telegram gives bots files of up to 20 MB."),
            Field::new("api_url", "Bot API server", Input::Url)
                .required()
                .probe("/bot/getMe")
                .default(DEFAULT_API)
                .hint("Telegram's own, unless you run one."),
        ]
    }

    fn invites(&self) -> Option<&'static str> {
        Some("Add a person")
    }

    fn removes_people(&self) -> bool {
        true
    }

    fn check(&self, s: &Setup) -> Result<(), String> {
        if !valid_token(s.secret("token").trim()) {
            return Err(
                "That isn't a bot token: it looks like 123456:ABC-def…, as @BotFather gives it."
                    .into(),
            );
        }
        Ok(())
    }

    fn identity(&self, s: &Setup) -> Option<String> {
        bot_id(s.secret("token").trim()).map(|id| format!("telegram:{id}"))
    }

    fn summary(&self, s: &Setup) -> String {
        let bot = bot_id(s.secret("token").trim()).unwrap_or("?");
        let server = s.text("api_url");
        if server == DEFAULT_API {
            format!("Telegram bot {bot}, not end-to-end encrypted")
        } else {
            format!("Telegram bot {bot} through {server}, not end-to-end encrypted")
        }
    }

    fn reach(&self, s: &Setup) -> String {
        format!(
            "“{}” gets a Telegram bot. People you let in can talk to it: each message they send starts a run of \
             its loop, in its folder, with the tools it has. Someone asks with a code you make, and you're asked \
             before they're let in.\n\nTelegram isn't end-to-end encrypted: Telegram's servers can read what \
             they and the agent say. {} connects to {} (the Internet permission doesn't apply to it, as for \
             model connections).",
            s.agent,
            s.program,
            s.text("api_url")
        )
    }

    fn widens(&self, old: &Setup, new: &Setup) -> bool {
        old.text("api_url") != new.text("api_url")
            || bot_id(old.secret("token").trim()) != bot_id(new.secret("token").trim())
    }

    fn cautions(&self, s: &Setup) -> Vec<String> {
        vec![format!(
            "Telegram isn't end-to-end encrypted: Telegram can read what people and {} say there",
            s.agent
        )]
    }

    fn deleting(&self) -> &'static str {
        "The bot's token and the people let in go too. The bot stays in Telegram (delete it in @BotFather if \
         you like), but it no longer answers."
    }

    fn people(&self, _s: &Setup, kept: Option<&Value>) -> Vec<Person> {
        kept.map(kept_people).unwrap_or_default()
    }

    /// A Telegram id: people may be let in later.
    // A Bot API server turns down a missing token with its own JSON, so
    // that's what tells it apart from any other web server.
    fn answers(&self, _field: &str, _status: u16, body: &str) -> bool {
        serde_json::from_str::<Value>(body).is_ok_and(|v| v["ok"].is_boolean())
    }
    fn valid_person(&self, _s: &Setup, id: &str) -> bool {
        !id.is_empty() && id.len() <= 20 && id.bytes().all(|b| b.is_ascii_digit())
    }

    #[cfg(feature = "telegram")]
    fn start(&self, host: Arc<dyn ChannelHost>, start: Start) -> Result<Handle, String> {
        live::start_kind(host, start)
    }

    #[cfg(not(feature = "telegram"))]
    fn start(&self, _host: Arc<dyn ChannelHost>, _start: Start) -> Result<Handle, String> {
        Err("This build has no Telegram channel.".into())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tokens_are_checked() {
        let token = "123456789:AAHdqTcvCH1vGWJxfSeofSAs0K5PALDsaw";
        assert!(valid_token(token));
        assert!(!valid_token("123456789"));
        assert!(!valid_token("abc:AAHdqTcvCH1vGWJxfSeofSAs0K5PALDsaw"));
        assert!(!valid_token("123:short"));
        assert!(!valid_token("123:AAHdqTcvCH1vGWJxfSeofSAs0K5PALD saw"));
        assert_eq!(bot_id(token), Some("123456789"));
    }

    #[test]
    fn only_a_bot_api_server_answers_the_probe() {
        let t = Telegram;
        let not_found = r#"{"ok":false,"error_code":404,"description":"Not Found"}"#;
        assert!(t.answers("api_url", 404, not_found));
        assert!(!t.answers("api_url", 200, "<html>hello</html>"));
        assert!(!t.answers("api_url", 404, ""));
    }

    #[test]
    fn people_are_read_from_what_was_kept() {
        let v = serde_json::json!({ "bot_id": 1, "offset": 5, "people": [{ "id": "42", "name": "Alice", "since": 1 }] });
        assert_eq!(kept_people(&v)[0].name, "Alice");
        assert!(kept_people(&serde_json::json!("nonsense")).is_empty());
    }
}
