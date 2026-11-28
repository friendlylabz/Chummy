//! Discord: a bot (from Discord's Developer Portal) for the agent, talked to
//! in direct messages only. Not end-to-end encrypted. People join with an
//! invitation code made in the program, and are let in only after whoever
//! runs it says yes. Running it needs the `discord` feature ([`live`]).

#[cfg(feature = "discord")]
pub mod live;

use std::sync::Arc;

use base64::Engine as _;
use serde_json::Value;

use crate::kind::{ChannelKind, Field, Input, Setup, Start};
use crate::{ChannelHost, Handle, Person};

/// Discord's own web API.
pub const DEFAULT_API: &str = "https://discord.com/api/v10";

/// The largest file people can send that the channel takes, in MB.
pub const MAX_UPLOAD_MB: u32 = 100;

pub struct Discord;

/// Whether a token looks like a Discord bot token: three parts with dots
/// between them, the first the bot's id in base64.
pub fn valid_token(token: &str) -> bool {
    let parts: Vec<&str> = token.split('.').collect();
    parts.len() == 3
        && (50..=120).contains(&token.len())
        && parts.iter().all(|p| {
            !p.is_empty()
                && p.bytes()
                    .all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-')
        })
        && bot_id(token).is_some()
}

/// The bot's id, from its token's first part (not a secret).
/// A made-up token of the right shape for the bot `bot`, for tests: no
/// token, real or fake, is written out anywhere in the code.
#[doc(hidden)]
pub fn fake_token(bot: &str) -> String {
    let id = base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(bot);
    format!("{id}.fake00.{}", "fake".repeat(10))
}

pub fn bot_id(token: &str) -> Option<String> {
    let first = token.split('.').next()?;
    let engine = base64::engine::general_purpose::GeneralPurpose::new(
        &base64::alphabet::URL_SAFE,
        base64::engine::GeneralPurposeConfig::new()
            .with_decode_padding_mode(base64::engine::DecodePaddingMode::Indifferent),
    );
    let bytes = engine
        .decode(first)
        .or_else(|_| base64::engine::general_purpose::STANDARD_NO_PAD.decode(first))
        .ok()?;
    let id = String::from_utf8(bytes).ok()?;
    (!id.is_empty() && id.len() <= 20 && id.bytes().all(|b| b.is_ascii_digit())).then_some(id)
}

/// The link that adds the bot to a server (so people there can message it).
pub fn invite_link(bot: &str) -> String {
    format!("https://discord.com/oauth2/authorize?client_id={bot}&scope=bot")
}

/// The people let in, from what a Discord channel kept.
pub fn kept_people(kept: &Value) -> Vec<Person> {
    kept.get("people")
        .and_then(|p| serde_json::from_value(p.clone()).ok())
        .unwrap_or_default()
}

impl ChannelKind for Discord {
    fn id(&self) -> &'static str {
        "discord"
    }

    fn label(&self) -> &'static str {
        "Discord"
    }

    fn about(&self) -> &'static str {
        "A Discord bot for the agent: people you let in talk to it in direct messages. Not end-to-end \
         encrypted: Discord can read these messages. People ask to talk to the agent with a code you make, \
         and you're asked before anyone is let in."
    }

    fn fields(&self) -> Vec<Field> {
        vec![
            Field::new("token", "Bot token", Input::Secret)
                .required()
                .placeholder("The token from the Developer Portal")
                .hint(
                    "In Discord's Developer Portal (discord.com/developers): New Application, then Bot → Reset \
                     Token, and paste it here. It's the bot's whole login. Turn off Public Bot there too, so \
                     only you can add it to servers. Use a bot of its own, not one another channel or program \
                     uses.",
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
            .default(25)
            .hint("Files people send are saved in the agent's folder, under uploads/."),
            Field::new("api_url", "Discord API", Input::Url)
                .required()
                .probe("/gateway")
                .default(DEFAULT_API)
                .hint("Discord's own; only change it for testing."),
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
                "That isn't a Discord bot token: it's three parts with dots between them, from the Developer \
                 Portal (Bot → Reset Token)."
                    .into(),
            );
        }
        Ok(())
    }

    fn identity(&self, s: &Setup) -> Option<String> {
        bot_id(s.secret("token").trim()).map(|id| format!("discord:{id}"))
    }

    fn summary(&self, s: &Setup) -> String {
        let bot = bot_id(s.secret("token").trim()).unwrap_or_else(|| "?".into());
        let server = s.text("api_url");
        if server == DEFAULT_API {
            format!("Discord bot {bot}, not end-to-end encrypted")
        } else {
            format!("Discord bot {bot} through {server}, not end-to-end encrypted")
        }
    }

    fn reach(&self, s: &Setup) -> String {
        format!(
            "“{}” gets a Discord bot. People you let in can talk to it in direct messages: each message they \
             send starts a run of its loop, in its folder, with the tools it has. Someone asks with a code you \
             make, and you're asked before they're let in.\n\nDiscord isn't end-to-end encrypted: Discord can \
             read what they and the agent say. {} connects to {} (the Internet permission doesn't apply to it, \
             as for model connections).",
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
            "Discord isn't end-to-end encrypted: Discord can read what people and {} say there",
            s.agent
        )]
    }

    fn deleting(&self) -> &'static str {
        "The bot's token and the people let in go too. The bot stays in Discord (delete its application in the \
         Developer Portal if you like), but it no longer answers."
    }

    fn people(&self, _s: &Setup, kept: Option<&Value>) -> Vec<Person> {
        kept.map(kept_people).unwrap_or_default()
    }

    // Discord's API answers /gateway with where its Gateway is, without a
    // token: that's what tells it apart from any other web server.
    fn answers(&self, _field: &str, status: u16, body: &str) -> bool {
        status == 200
            && serde_json::from_str::<Value>(body)
                .is_ok_and(|v| v["url"].as_str().is_some_and(|u| u.starts_with("ws")))
    }

    /// A Discord user id: people may be let in later.
    fn valid_person(&self, _s: &Setup, id: &str) -> bool {
        !id.is_empty() && id.len() <= 20 && id.bytes().all(|b| b.is_ascii_digit())
    }

    #[cfg(feature = "discord")]
    fn start(&self, host: Arc<dyn ChannelHost>, start: Start) -> Result<Handle, String> {
        live::start_kind(host, start)
    }

    #[cfg(not(feature = "discord"))]
    fn start(&self, _host: Arc<dyn ChannelHost>, _start: Start) -> Result<Handle, String> {
        Err("This build has no Discord channel.".into())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tokens_are_checked() {
        let token = fake_token("123456789012345678");
        assert!(valid_token(&token));
        assert_eq!(bot_id(&token).as_deref(), Some("123456789012345678"));
        assert!(!valid_token(&crate::telegram::fake_token("123456789")));
        assert!(!valid_token("MTIz.ab.cd"));
        assert!(!valid_token(&token.replace(".fake00.", ".fa e00.")));
        // A first part that isn't an id.
        assert!(!valid_token(&fake_token("hello world hello")));
        assert!(invite_link("42").contains("client_id=42"));
    }

    #[test]
    fn only_discords_api_answers_the_probe() {
        let d = Discord;
        assert!(d.answers("api_url", 200, r#"{"url":"wss://gateway.discord.gg"}"#));
        assert!(!d.answers("api_url", 200, "<html>hello</html>"));
        assert!(!d.answers("api_url", 404, r#"{"url":"wss://x"}"#));
    }

    #[test]
    fn people_are_read_from_what_was_kept() {
        let v = serde_json::json!({ "bot_id": "1", "people": [{ "id": "42", "name": "Alice", "since": 1 }] });
        assert_eq!(kept_people(&v)[0].name, "Alice");
        assert!(kept_people(&serde_json::json!("nonsense")).is_empty());
    }
}
