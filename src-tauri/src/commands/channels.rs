//! Settings → Channels: adding, changing, turning off and deleting the
//! channels agents can be talked to through; answering Matrix emoji
//! verifications; Telegram pairing codes and the people let in. Anything
//! that lets more people reach an agent asks natively first.

use serde::Serialize;
use tauri::{AppHandle, State, WebviewWindow};
use zeroize::Zeroizing;

use super::CmdResult;
use crate::channels::{self, Live};
use crate::dialogs;
use crate::settings::{ChannelConfig, ChannelKind, MatrixChannel, Settings};
use crate::state::AppState;
use chummy_channel::{Code, Person};

#[derive(Debug, Clone, Serialize)]
pub struct ChannelView {
    #[serde(flatten)]
    pub config: ChannelConfig,
    /// Its secret (Matrix: password; Telegram: bot token) is saved in the
    /// keychain.
    pub has_secret: bool,
    /// Who was let in (Telegram).
    pub people: Vec<Person>,
    pub live: Live,
}

#[tauri::command]
pub fn list_channels(state: State<'_, AppState>) -> Vec<ChannelView> {
    let live = state.channels.get().map(|c| c.live()).unwrap_or_default();
    state
        .settings()
        .channels
        .into_iter()
        .map(|c| ChannelView {
            has_secret: state
                .secrets
                .get(&channels::secret_key(&c.id))
                .ok()
                .flatten()
                .is_some(),
            people: match c.kind {
                ChannelKind::Telegram(_) => channels::people(&state, &c.id),
                ChannelKind::Matrix(_) => vec![],
            },
            live: live.get(&c.id).cloned().unwrap_or_default(),
            config: c,
        })
        .collect()
}

/// What's wrong with a channel, if anything.
fn check(s: &Settings, c: &ChannelConfig) -> CmdResult<()> {
    let name = c.name.trim();
    if name.is_empty() || name.chars().count() > 60 {
        return Err("Give the channel a name (up to 60 characters).".into());
    }
    if s.agent(&c.agent).is_none() {
        return Err("Choose the agent people talk to.".into());
    }
    match &c.kind {
        ChannelKind::Matrix(m) => check_matrix(s, c, m),
        ChannelKind::Telegram(t) => {
            server("The Bot API server", &t.api_url)?;
            if !(1..=chummy_bundle::MAX_TELEGRAM_UPLOAD_MB).contains(&t.upload_mb) {
                return Err(format!(
                    "The largest file must be between 1 and {} MB (Telegram gives bots no more).",
                    chummy_bundle::MAX_TELEGRAM_UPLOAD_MB
                ));
            }
            Ok(())
        }
    }
}

/// An address Chummy connects to: https, or plain http only nearby.
fn server(what: &str, url: &str) -> CmdResult<()> {
    match chummy_llm::endpoint_problem(url) {
        Some(p) => Err(format!("{what}: {p}")),
        None if chummy_llm::unencrypted_remote(url) => Err(format!(
            "{what} must use https (plain http only on this computer or your local network)."
        )),
        None => Ok(()),
    }
}

fn check_matrix(s: &Settings, c: &ChannelConfig, m: &MatrixChannel) -> CmdResult<()> {
    server("The homeserver", &m.homeserver)?;
    if !chummy_bundle::valid_matrix_id(&m.user_id) {
        return Err("The bot's Matrix ID isn't valid (it looks like @name:server).".into());
    }
    if m.users.is_empty() {
        return Err("Add at least one person who may talk to the agent.".into());
    }
    for u in &m.users {
        if !chummy_bundle::valid_matrix_id(u) {
            return Err(format!(
                "“{u}” isn't a valid Matrix ID (it looks like @name:server)."
            ));
        }
        if *u == m.user_id {
            return Err("The bot can't be one of the people who talk to the agent.".into());
        }
    }
    if !(1..=chummy_bundle::MAX_UPLOAD_MB).contains(&m.upload_mb) {
        return Err(format!(
            "The largest file must be between 1 and {} MB.",
            chummy_bundle::MAX_UPLOAD_MB
        ));
    }
    let same_bot = s.channels.iter().any(|o| {
        o.id != c.id && matches!(&o.kind, ChannelKind::Matrix(om) if om.user_id == m.user_id)
    });
    if same_bot {
        return Err(
            "Another channel already uses that bot account: give each channel its own.".into(),
        );
    }
    Ok(())
}

/// A Telegram channel's token: one from @BotFather, and a bot no other
/// channel uses.
fn check_token(state: &AppState, s: &Settings, c: &ChannelConfig, token: &str) -> CmdResult<()> {
    if !chummy_bundle::valid_telegram_token(token) {
        return Err(
            "That isn't a bot token: it looks like 123456:ABC-def…, as @BotFather gives it.".into(),
        );
    }
    let bot = chummy_bundle::telegram_bot_id(token);
    let same = s.channels.iter().any(|o| {
        o.id != c.id
            && matches!(o.kind, ChannelKind::Telegram(_))
            && state
                .secrets
                .get(&channels::secret_key(&o.id))
                .ok()
                .flatten()
                .is_some_and(|t| chummy_bundle::telegram_bot_id(&t) == bot)
    });
    if same {
        return Err("Another channel already uses that bot: give each channel its own.".into());
    }
    Ok(())
}

/// The confirmation for letting people reach an agent through a channel.
fn reach_message(s: &Settings, c: &ChannelConfig) -> String {
    let agent = s
        .agent(&c.agent)
        .map(|a| a.name.clone())
        .unwrap_or_default();
    let mut msg = match &c.kind {
        ChannelKind::Matrix(m) => format!(
            "{} will be able to talk to “{agent}” in Matrix: each message they send starts a run of its \
             loop, in its folder, with the tools it has. Chummy connects to {} as {} (the Internet permission \
             doesn't apply to it, as for model connections) and makes an encrypted room for each person.\n\n\
             They verify the bot by comparing emoji with you here first. Use a Matrix account of its own for \
             the bot, not one a chummyd uses.",
            m.users.join(", "),
            m.homeserver,
            m.user_id
        ),
        ChannelKind::Telegram(t) => format!(
            "“{agent}” gets a Telegram bot. People you let in can talk to it: each message they send starts a \
             run of its loop, in its folder, with the tools it has. Someone asks with a pairing code you make \
             here, and you're asked before they're let in.\n\nTelegram isn't end-to-end encrypted: Telegram's \
             servers can read what they and the agent say. Chummy connects to {} (the Internet permission \
             doesn't apply to it, as for model connections). Use a bot of its own, not one a chummyd uses.",
            t.api_url
        ),
    };
    let label = c.kind.label();
    if c.approvals {
        msg.push_str(&format!(
            "\n\nTool approvals can be answered in {label} too, so whoever can talk to the agent there can let it \
             use its tools. A message or page the agent reads could ask for exactly that."
        ));
    } else {
        msg.push_str("\n\nTool approvals stay here, in Chummy.");
    }
    msg
}

/// Adds or changes a channel. `password`: its secret, the Matrix bot's
/// password or the Telegram bot's token (`None` or empty keeps the saved
/// one). Letting more people in, a new agent, or approvals over the
/// channel ask natively first. Returns the channel's id, or `None` if the
/// user said no.
#[tauri::command]
pub async fn save_channel(
    app: AppHandle,
    window: WebviewWindow,
    state: State<'_, AppState>,
    mut channel: ChannelConfig,
    password: Option<String>,
) -> CmdResult<Option<String>> {
    channel.name = channel.name.trim().to_string();
    match &mut channel.kind {
        ChannelKind::Matrix(m) => {
            m.homeserver = m.homeserver.trim().trim_end_matches('/').to_string();
            m.user_id = m.user_id.trim().to_string();
            let mut users: Vec<String> = m
                .users
                .iter()
                .map(|u| u.trim().to_string())
                .filter(|u| !u.is_empty())
                .collect();
            users.sort();
            users.dedup();
            m.users = users;
        }
        ChannelKind::Telegram(t) => {
            t.api_url = t.api_url.trim().trim_end_matches('/').to_string();
        }
    }
    let password = password
        .map(|p| Zeroizing::new(p.trim().to_string()))
        .filter(|p| !p.is_empty());

    let s = state.settings();
    let old = s.channels.iter().find(|c| c.id == channel.id).cloned();
    if !channel.id.is_empty() && old.is_none() {
        return Err("That channel no longer exists.".into());
    }
    if let Some(o) = &old
        && o.kind.label() != channel.kind.label()
    {
        return Err("A channel's type can't change: make a new one.".into());
    }
    if channel.id.is_empty() {
        channel.id = format!("ch-{}", chummy_store::random_id(4));
    }
    check(&s, &channel)?;
    let saved = state
        .secrets
        .get(&channels::secret_key(&channel.id))
        .ok()
        .flatten()
        .map(Zeroizing::new);
    if password.is_none() && saved.is_none() {
        return Err(match channel.kind {
            ChannelKind::Matrix(_) => "Enter the bot's Matrix password.",
            ChannelKind::Telegram(_) => "Enter the bot's token, from @BotFather.",
        }
        .into());
    }
    let new_bot = matches!(channel.kind, ChannelKind::Telegram(_))
        && password.as_ref().is_some_and(|p| {
            saved.as_ref().map(|s| chummy_bundle::telegram_bot_id(s))
                != Some(chummy_bundle::telegram_bot_id(p))
        });
    if let (ChannelKind::Telegram(_), Some(token)) =
        (&channel.kind, password.as_ref().or(saved.as_ref()))
    {
        check_token(&state, &s, &channel, token)?;
    }

    // Asks when more people (or another agent) can be reached, or
    // approvals move to the channel.
    let ask = match &old {
        None => channel.enabled,
        Some(o) => {
            let wider = match (&o.kind, &channel.kind) {
                (ChannelKind::Matrix(om), ChannelKind::Matrix(m)) => {
                    m.users.iter().any(|u| !om.users.contains(u)) || m.homeserver != om.homeserver
                }
                (ChannelKind::Telegram(ot), ChannelKind::Telegram(t)) => {
                    ot.api_url != t.api_url || new_bot
                }
                _ => true,
            };
            channel.enabled
                && (!o.enabled
                    || o.agent != channel.agent
                    || (channel.approvals && !o.approvals)
                    || wider)
        }
    };
    if ask {
        let title = format!(
            "Let people talk to {} in {}?",
            agent_name(&s, &channel),
            channel.kind.label()
        );
        if !dialogs::confirm(
            &app,
            window.label(),
            &title,
            &reach_message(&s, &channel),
            "Allow",
            true,
        )
        .await
        {
            return Ok(None);
        }
    }

    if let Some(p) = &password {
        state
            .secrets
            .set(&channels::secret_key(&channel.id), p)
            .map_err(|e| e.to_string())?;
    }
    let id = channel.id.clone();
    let name = channel.name.clone();
    state.update(&app, |s| {
        match s.channels.iter_mut().find(|c| c.id == channel.id) {
            Some(c) => *c = channel,
            None => s.channels.push(channel),
        }
    })?;
    chummy_log::system::log(
        chummy_log::system::Level::Warn,
        "channels",
        format!(
            "Channel {name} {}",
            if old.is_some() { "changed" } else { "added" }
        ),
        &[],
    );
    channels::sync(&app);
    Ok(Some(id))
}

fn agent_name(s: &Settings, c: &ChannelConfig) -> String {
    s.agent(&c.agent)
        .map(|a| format!("“{}”", a.name))
        .unwrap_or_else(|| "the agent".into())
}

/// Turns a channel on or off. Turning it on asks first. Returns whether
/// it's on now.
#[tauri::command]
pub async fn set_channel_enabled(
    app: AppHandle,
    window: WebviewWindow,
    state: State<'_, AppState>,
    id: String,
    enabled: bool,
) -> CmdResult<bool> {
    let s = state.settings();
    let c = s
        .channels
        .iter()
        .find(|c| c.id == id)
        .cloned()
        .ok_or("That channel no longer exists.")?;
    if enabled && !c.enabled {
        check(&s, &c)?;
        let title = format!(
            "Let people talk to {} in {}?",
            agent_name(&s, &c),
            c.kind.label()
        );
        if !dialogs::confirm(
            &app,
            window.label(),
            &title,
            &reach_message(&s, &c),
            "Turn on",
            true,
        )
        .await
        {
            return Ok(false);
        }
    }
    state.update(&app, |s| {
        if let Some(c) = s.channels.iter_mut().find(|c| c.id == id) {
            c.enabled = enabled;
        }
    })?;
    chummy_log::system::log(
        chummy_log::system::Level::Warn,
        "channels",
        format!(
            "Channel {} turned {}",
            c.name,
            if enabled { "on" } else { "off" }
        ),
        &[],
    );
    channels::sync(&app);
    Ok(enabled)
}

/// Deletes a channel after a native confirmation: its password, login and
/// keys go too. The agent's chats stay. Returns false if the user said no.
#[tauri::command]
pub async fn delete_channel(
    app: AppHandle,
    window: WebviewWindow,
    state: State<'_, AppState>,
    id: String,
) -> CmdResult<bool> {
    let Some(c) = state.settings().channels.into_iter().find(|c| c.id == id) else {
        return Ok(true);
    };
    let msg = match c.kind {
        ChannelKind::Matrix(_) => format!(
            "“{}” will be deleted, with its password and the bot's login and keys on this computer. The \
             Matrix rooms stay, but nobody can talk to the agent through them. The conversations stay in \
             the agent's chats.",
            c.name
        ),
        ChannelKind::Telegram(_) => format!(
            "“{}” will be deleted, with the bot's token and the people let in. The bot stays in Telegram \
             (delete it in @BotFather if you like), but it no longer answers. The conversations stay in \
             the agent's chats.",
            c.name
        ),
    };
    if !dialogs::confirm(
        &app,
        window.label(),
        "Delete this channel?",
        &msg,
        "Delete",
        true,
    )
    .await
    {
        return Ok(false);
    }
    state.update(&app, |s| s.channels.retain(|x| x.id != id))?;
    // Stopped before its keys go.
    channels::sync(&app);
    tokio::time::sleep(std::time::Duration::from_millis(200)).await;
    channels::forget(&state, &id);
    chummy_log::system::log(
        chummy_log::system::Level::Warn,
        "channels",
        format!("Channel {} deleted", c.name),
        &[],
    );
    Ok(true)
}

/// The answer to what waits on a channel: Matrix emoji that match (or
/// don't), or someone to let in to a Telegram bot (or not). Letting
/// someone in asks natively first.
#[tauri::command]
pub async fn answer_channel_pending(
    app: AppHandle,
    window: WebviewWindow,
    state: State<'_, AppState>,
    channel: String,
    n: u32,
    yes: bool,
) -> CmdResult<bool> {
    let ch = state.channels.get().ok_or("Channels aren't running yet.")?;
    let p = ch
        .pending(&channel, n)
        .ok_or("That doesn't wait any more.")?;
    if yes && p.kind == "pairing" {
        let s = state.settings();
        let agent = s
            .channels
            .iter()
            .find(|c| c.id == channel)
            .map(|c| agent_name(&s, c))
            .unwrap_or_else(|| "the agent".into());
        let msg = format!(
            "{} will be able to talk to {agent} in Telegram: each message they send starts a run of its \
             loop, in its folder, with the tools it has. Telegram can read what they say.\n\nOnly let in \
             someone you gave the code to.",
            p.who
        );
        if !dialogs::confirm(
            &app,
            window.label(),
            "Let them talk to the agent?",
            &msg,
            "Let them in",
            true,
        )
        .await
        {
            return Ok(false);
        }
    }
    ch.answer(&channel, n, yes)?;
    Ok(true)
}

/// A pairing code for a Telegram channel: whoever sends it to the bot asks
/// to be let in.
#[tauri::command]
pub async fn channel_pairing_code(
    app: AppHandle,
    state: State<'_, AppState>,
    channel: String,
) -> CmdResult<Code> {
    let ch = state.channels.get().ok_or("Channels aren't running yet.")?;
    let code = ch.pairing_code(&channel).await?;
    chummy_log::system::info("channels", "Made a pairing code");
    channels::changed(&app);
    Ok(code)
}

/// Stops someone talking to a Telegram channel's agent.
#[tauri::command]
pub fn remove_channel_person(app: AppHandle, channel: String, person: String) -> CmdResult<()> {
    channels::remove_person(&app, &channel, &person)?;
    chummy_log::system::log(
        chummy_log::system::Level::Warn,
        "channels",
        "Removed someone from a channel",
        &[],
    );
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::settings::TelegramChannel;

    fn channel(users: &[&str]) -> ChannelConfig {
        ChannelConfig {
            id: "ch-1".into(),
            name: "Phone".into(),
            enabled: true,
            agent: "a".into(),
            approvals: false,
            kind: ChannelKind::Matrix(MatrixChannel {
                homeserver: "https://matrix.example.org".into(),
                user_id: "@bot:example.org".into(),
                users: users.iter().map(|u| u.to_string()).collect(),
                upload_mb: 100,
            }),
        }
    }

    fn settings() -> Settings {
        serde_json::from_value(serde_json::json!({
            "agents": [{ "id": "a", "name": "A", "loop_id": "l",
                         "workspace": { "path": "/tmp/w", "confirmed_at": 1 }, "created_at": 1 }]
        }))
        .unwrap()
    }

    #[test]
    fn channels_are_checked() {
        let s = settings();
        assert!(check(&s, &channel(&["@me:example.org"])).is_ok());
        assert!(check(&s, &channel(&[])).is_err(), "nobody to talk to");
        assert!(check(&s, &channel(&["me"])).is_err());
        assert!(
            check(&s, &channel(&["@bot:example.org"])).is_err(),
            "the bot itself"
        );
        let mut c = channel(&["@me:example.org"]);
        c.agent = "gone".into();
        assert!(check(&s, &c).is_err());
        let at = |hs: &str| {
            let mut c = channel(&["@me:example.org"]);
            let ChannelKind::Matrix(m) = &mut c.kind else {
                unreachable!()
            };
            m.homeserver = hs.into();
            c
        };
        assert!(
            check(&s, &at("http://matrix.example.org")).is_err(),
            "plain http far away"
        );
        assert!(
            check(&s, &at("http://192.168.1.5:8008")).is_ok(),
            "plain http nearby"
        );
        // Two channels can't share a bot.
        let mut s = settings();
        s.channels.push(channel(&["@me:example.org"]));
        let mut other = channel(&["@me:example.org"]);
        other.id = "ch-2".into();
        assert!(check(&s, &other).is_err());
    }

    #[test]
    fn telegram_channels_are_checked() {
        let s = settings();
        let mut c = channel(&[]);
        c.kind = ChannelKind::Telegram(TelegramChannel {
            api_url: chummy_bundle::TELEGRAM_API.into(),
            upload_mb: 20,
            stream: true,
        });
        assert!(check(&s, &c).is_ok(), "nobody yet is fine: people pair");
        let with = |mb: u32, url: &str| {
            let mut c = c.clone();
            c.kind = ChannelKind::Telegram(TelegramChannel {
                api_url: url.into(),
                upload_mb: mb,
                stream: true,
            });
            c
        };
        assert!(
            check(&s, &with(50, chummy_bundle::TELEGRAM_API)).is_err(),
            "bots get 20 MB at most"
        );
        assert!(
            check(&s, &with(20, "http://telegram.example.org")).is_err(),
            "plain http far away"
        );
        let v = serde_json::to_value(&c).unwrap();
        assert_eq!(v["type"], "telegram");
        let back: ChannelConfig = serde_json::from_value(serde_json::json!({
            "id": "t", "name": "T", "agent": "a", "type": "telegram"
        }))
        .unwrap();
        assert_eq!(
            back.kind,
            ChannelKind::Telegram(TelegramChannel {
                api_url: chummy_bundle::TELEGRAM_API.into(),
                upload_mb: 20,
                stream: true,
            })
        );
    }

    #[test]
    fn channels_read_and_write_with_their_type() {
        let c = channel(&["@me:example.org"]);
        let v = serde_json::to_value(&c).unwrap();
        assert_eq!(v["type"], "matrix");
        assert_eq!(v["homeserver"], "https://matrix.example.org");
        let back: ChannelConfig = serde_json::from_value(v).unwrap();
        assert_eq!(back, c);
    }
}
