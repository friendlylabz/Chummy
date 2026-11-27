//! Settings → Channels: adding, changing, turning off and deleting the
//! channels agents can be talked to through, and answering their emoji
//! verifications. Anything that lets more people reach an agent asks
//! natively first.

use serde::Serialize;
use tauri::{AppHandle, State, WebviewWindow};
use zeroize::Zeroizing;

use super::CmdResult;
use crate::channels::{self, Live};
use crate::dialogs;
use crate::settings::{ChannelConfig, ChannelKind, Settings};
use crate::state::AppState;

#[derive(Debug, Clone, Serialize)]
pub struct ChannelView {
    #[serde(flatten)]
    pub config: ChannelConfig,
    /// A password is saved in the keychain.
    pub has_password: bool,
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
            has_password: state
                .secrets
                .get(&channels::password_key(&c.id))
                .ok()
                .flatten()
                .is_some(),
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
    let ChannelKind::Matrix(m) = &c.kind;
    match chummy_llm::endpoint_problem(&m.homeserver) {
        Some(p) => return Err(format!("The homeserver: {p}")),
        None if chummy_llm::unencrypted_remote(&m.homeserver) => {
            return Err(
                "The homeserver must use https (plain http only on this computer or your local network)."
                    .into(),
            );
        }
        None => {}
    }
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
        let ChannelKind::Matrix(om) = &o.kind;
        o.id != c.id && om.user_id == m.user_id
    });
    if same_bot {
        return Err(
            "Another channel already uses that bot account: give each channel its own.".into(),
        );
    }
    Ok(())
}

/// The confirmation for letting people reach an agent through a channel.
fn reach_message(s: &Settings, c: &ChannelConfig) -> String {
    let ChannelKind::Matrix(m) = &c.kind;
    let agent = s
        .agent(&c.agent)
        .map(|a| a.name.clone())
        .unwrap_or_default();
    let people = m.users.join(", ");
    let mut msg = format!(
        "{people} will be able to talk to “{agent}” in Matrix: each message they send starts a run of its \
         loop, in its folder, with the tools it has. Chummy connects to {} as {} (the Internet permission \
         doesn't apply to it, as for model connections) and makes an encrypted room for each person.\n\n\
         They verify the bot by comparing emoji with you here first. Use a Matrix account of its own for \
         the bot, not one a chummyd uses.",
        m.homeserver, m.user_id
    );
    if c.approvals {
        msg.push_str(
            "\n\nTool approvals can be answered in Matrix too, so whoever has those accounts can let the \
             agent use its tools. A message or page the agent reads could ask for exactly that.",
        );
    } else {
        msg.push_str("\n\nTool approvals stay here, in Chummy.");
    }
    msg
}

/// Adds or changes a channel. `password`: the bot's password (`None` or
/// empty keeps the saved one). Letting more people in, a new agent, or
/// approvals over the channel ask natively first. Returns the channel's
/// id, or `None` if the user said no.
#[tauri::command]
pub async fn save_channel(
    app: AppHandle,
    window: WebviewWindow,
    state: State<'_, AppState>,
    mut channel: ChannelConfig,
    password: Option<String>,
) -> CmdResult<Option<String>> {
    channel.name = channel.name.trim().to_string();
    let ChannelKind::Matrix(m) = &mut channel.kind;
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
    let password = password.map(Zeroizing::new).filter(|p| !p.is_empty());

    let s = state.settings();
    let old = s.channels.iter().find(|c| c.id == channel.id).cloned();
    if !channel.id.is_empty() && old.is_none() {
        return Err("That channel no longer exists.".into());
    }
    if channel.id.is_empty() {
        channel.id = format!("ch-{}", chummy_store::random_id(4));
    }
    check(&s, &channel)?;
    let has_password = state
        .secrets
        .get(&channels::password_key(&channel.id))
        .ok()
        .flatten()
        .is_some();
    if password.is_none() && !has_password {
        return Err("Enter the bot's Matrix password.".into());
    }

    // Asks when more people (or another agent) can be reached, or
    // approvals move to the channel.
    let ask = match &old {
        None => channel.enabled,
        Some(o) => {
            let (ChannelKind::Matrix(om), ChannelKind::Matrix(m)) = (&o.kind, &channel.kind);
            channel.enabled
                && (!o.enabled
                    || o.agent != channel.agent
                    || (channel.approvals && !o.approvals)
                    || m.users.iter().any(|u| !om.users.contains(u))
                    || m.homeserver != om.homeserver)
        }
    };
    if ask {
        let title = format!("Let people talk to {} in Matrix?", agent_name(&s, &channel));
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
            .set(&channels::password_key(&channel.id), p)
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
        let title = format!("Let people talk to {} in Matrix?", agent_name(&s, &c));
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
    let msg = format!(
        "“{}” will be deleted, with its password and the bot's login and keys on this computer. The \
         Matrix rooms stay, but nobody can talk to the agent through them. The conversations stay in \
         the agent's chats.",
        c.name
    );
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

/// The answer to an emoji verification: whether the emoji match what the
/// person sees.
#[tauri::command]
pub fn answer_channel_verification(
    state: State<'_, AppState>,
    channel: String,
    n: u32,
    matches: bool,
) -> CmdResult<()> {
    state
        .channels
        .get()
        .ok_or("Channels aren't running yet.")?
        .verify(&channel, n, matches)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::settings::MatrixChannel;

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
            let ChannelKind::Matrix(m) = &mut c.kind;
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
    fn channels_read_and_write_with_their_type() {
        let c = channel(&["@me:example.org"]);
        let v = serde_json::to_value(&c).unwrap();
        assert_eq!(v["type"], "matrix");
        assert_eq!(v["homeserver"], "https://matrix.example.org");
        let back: ChannelConfig = serde_json::from_value(v).unwrap();
        assert_eq!(back, c);
    }
}
