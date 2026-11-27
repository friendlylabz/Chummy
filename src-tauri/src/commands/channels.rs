//! Settings → Channels: adding, changing, turning off and deleting the
//! channels agents can be talked to through, answering what they ask to
//! verify, invitations and the people let in. None of it names a kind of
//! channel: what each takes, how it's checked and described, and what
//! letting people in through it means come from its kind
//! (`chummy_channel::ChannelKind`). Anything that lets more people reach an
//! agent asks natively first.

use std::collections::BTreeMap;

use serde::Serialize;
use tauri::{AppHandle, State, WebviewWindow};
use zeroize::Zeroizing;

use super::CmdResult;
use crate::channels::{self, Live, Prepared};
use crate::dialogs;
use crate::settings::{ChannelConfig, Settings};
use crate::state::AppState;
use chummy_channel::{Input, Invite, KindInfo, Person};

/// Every kind of channel, for the form.
#[tauri::command]
pub fn channel_kinds() -> Vec<KindInfo> {
    chummy_channel::kind::info()
}

/// Whether the server in a channel's address field answers.
#[tauri::command]
pub async fn channel_probe(kind: String, field: String, url: String) -> CmdResult<String> {
    chummy_channel::kind::probe(&kind, &field, &url).await
}

#[derive(Debug, Clone, Serialize)]
pub struct ChannelView {
    #[serde(flatten)]
    pub config: ChannelConfig,
    /// Its secret fields with something saved in the keychain.
    pub saved_secrets: Vec<String>,
    /// Who may talk through it now.
    pub people: Vec<Person>,
    /// One line about it, as its kind says it.
    pub summary: String,
    pub live: Live,
}

#[tauri::command]
pub fn list_channels(state: State<'_, AppState>) -> Vec<ChannelView> {
    let live = state.channels.get().map(|c| c.live()).unwrap_or_default();
    let s = state.settings();
    s.channels
        .iter()
        .map(|c| {
            let p = channels::prepare(&state, &s, c, &BTreeMap::new()).ok();
            ChannelView {
                saved_secrets: p
                    .as_ref()
                    .map(|p| p.secrets.keys().cloned().collect())
                    .unwrap_or_default(),
                people: p
                    .as_ref()
                    .map(|p| {
                        p.kind
                            .people(&p.setup(), channels::kept(&state, &c.id).as_ref())
                    })
                    .unwrap_or_default(),
                summary: p
                    .as_ref()
                    .map(|p| p.kind.summary(&p.setup()))
                    .unwrap_or_default(),
                live: live.get(&c.id).cloned().unwrap_or_default(),
                config: c.clone(),
            }
        })
        .collect()
}

/// The accounts the other channels use.
fn others(state: &AppState, s: &Settings, id: &str) -> Vec<String> {
    s.channels
        .iter()
        .filter(|o| o.id != id)
        .filter_map(|o| {
            let op = channels::prepare(state, s, o, &BTreeMap::new()).ok()?;
            op.kind.identity(&op.setup())
        })
        .collect()
}

/// What's wrong with a channel, if anything: its name and agents, what its
/// kind checks, and an account another channel uses (`others`).
fn check(s: &Settings, c: &ChannelConfig, p: &Prepared, others: &[String]) -> CmdResult<()> {
    let name = c.name.trim();
    if name.is_empty() || name.chars().count() > 60 {
        return Err("Give the channel a name (up to 60 characters).".into());
    }
    if c.agents.iter().any(|a| s.agent(a).is_none()) {
        return Err("Choose the agent people talk to.".into());
    }
    chummy_channel::kind::check_agents(p.kind, &c.agents)?;
    chummy_channel::kind::check(p.kind, &p.setup())?;
    if p.kind
        .identity(&p.setup())
        .is_some_and(|me| others.contains(&me))
    {
        return Err("Another channel already uses that account: give each channel its own.".into());
    }
    Ok(())
}

/// The confirmation for letting people reach agents through a channel.
fn reach_message(c: &ChannelConfig, p: &Prepared) -> String {
    let mut msg = p.kind.reach(&p.setup());
    let label = p.kind.label();
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

fn reach_title(p: &Prepared) -> String {
    format!("Let people talk to {} in {}?", p.agent, p.kind.label())
}

/// Adds or changes a channel. `secrets`: what was typed in its secret
/// fields (empty keeps what's saved). Letting more people (or other
/// agents) in, or approvals in the channel, ask natively first. Returns
/// the channel's id, or `None` if the user said no.
#[tauri::command]
pub async fn save_channel(
    app: AppHandle,
    window: WebviewWindow,
    state: State<'_, AppState>,
    mut channel: ChannelConfig,
    secrets: BTreeMap<String, String>,
) -> CmdResult<Option<String>> {
    let kind = chummy_channel::kind(&channel.kind).ok_or("That kind of channel isn't known.")?;
    channel.name = channel.name.trim().to_string();
    channel.options = chummy_channel::kind::tidy(kind, &channel.options);
    channel.agents.dedup();
    let s = state.settings();
    let old = s.channels.iter().find(|c| c.id == channel.id).cloned();
    if !channel.id.is_empty() && old.is_none() {
        return Err("That channel no longer exists.".into());
    }
    if let Some(o) = &old
        && o.kind != channel.kind
    {
        return Err("A channel's type can't change: make a new one.".into());
    }
    if channel.id.is_empty() {
        channel.id = format!("ch-{}", chummy_store::random_id(4));
    }
    let new = channels::prepare(&state, &s, &channel, &secrets)?;
    check(&s, &channel, &new, &others(&state, &s, &channel.id))?;

    // Asks when more people (or other agents) can be reached, or approvals
    // move to the channel.
    let ask = channel.enabled
        && match &old {
            None => true,
            Some(o) => {
                let before = channels::prepare(&state, &s, o, &BTreeMap::new())?;
                !o.enabled
                    || channel.agents.iter().any(|a| !o.agents.contains(a))
                    || (channel.approvals && !o.approvals)
                    || kind.widens(&before.setup(), &new.setup())
            }
        };
    if ask
        && !dialogs::confirm(
            &app,
            window.label(),
            &reach_title(&new),
            &reach_message(&channel, &new),
            "Allow",
            true,
        )
        .await
    {
        return Ok(None);
    }

    for f in kind.fields() {
        if f.input != Input::Secret {
            continue;
        }
        if let Some(v) = secrets
            .get(f.key)
            .map(|v| v.trim())
            .filter(|v| !v.is_empty())
        {
            state
                .secrets
                .set(
                    &channels::secret_key(&channel.id, f.key),
                    &Zeroizing::new(v.to_string()),
                )
                .map_err(|e| e.to_string())?;
        }
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
        let p = channels::prepare(&state, &s, &c, &BTreeMap::new())?;
        check(&s, &c, &p, &others(&state, &s, &c.id))?;
        if !dialogs::confirm(
            &app,
            window.label(),
            &reach_title(&p),
            &reach_message(&c, &p),
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

/// Deletes a channel after a native confirmation: its secrets and what it
/// kept go too. The agents' chats stay. Returns false if the user said no.
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
    let left = chummy_channel::kind(&c.kind)
        .map(|k| k.deleting())
        .unwrap_or("");
    let msg = format!(
        "“{}” will be deleted. {left} The conversations stay in the agents' chats.",
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
    channels::forget(&state, &c);
    chummy_log::system::log(
        chummy_log::system::Level::Warn,
        "channels",
        format!("Channel {} deleted", c.name),
        &[],
    );
    Ok(true)
}

/// The answer to what a channel asks to verify (emoji that match, someone
/// to let in…). When the channel says yes needs saying again, it's asked
/// natively first. Returns false if the user said no there.
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
    let v = ch
        .pending(&channel, n)
        .ok_or("That doesn't wait any more.")?;
    if yes
        && let Some(confirm) = &v.confirm
        && !dialogs::confirm(
            &app,
            window.label(),
            &format!("{}?", v.yes),
            confirm,
            &v.yes,
            true,
        )
        .await
    {
        return Ok(false);
    }
    ch.answer(&channel, n, yes)?;
    Ok(true)
}

/// An invitation for a channel whose people join by one.
#[tauri::command]
pub async fn channel_invite(
    app: AppHandle,
    state: State<'_, AppState>,
    channel: String,
) -> CmdResult<Invite> {
    let ch = state.channels.get().ok_or("Channels aren't running yet.")?;
    let invite = ch.invite(&channel).await?;
    chummy_log::system::info("channels", "Made an invitation");
    channels::changed(&app);
    Ok(invite)
}

/// Stops someone talking through a channel (kinds that let people in
/// themselves).
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
    use serde_json::json;

    fn settings() -> Settings {
        serde_json::from_value(json!({
            "agents": [
                { "id": "a", "name": "A", "loop_id": "l",
                  "workspace": { "path": "/tmp/w", "confirmed_at": 1 }, "created_at": 1 },
                { "id": "b", "name": "B", "loop_id": "l",
                  "workspace": { "path": "/tmp/w2", "confirmed_at": 1 }, "created_at": 1 }
            ]
        }))
        .unwrap()
    }

    fn prepared(c: &ChannelConfig, secrets: &[(&str, &str)]) -> Prepared {
        let kind = chummy_channel::kind(&c.kind).unwrap();
        Prepared {
            kind,
            agent: "A".into(),
            options: chummy_channel::kind::tidy(kind, &c.options),
            secrets: secrets
                .iter()
                .map(|(k, v)| (k.to_string(), Zeroizing::new(v.to_string())))
                .collect(),
        }
    }

    fn channel(kind: &str, agents: &[&str], options: serde_json::Value) -> ChannelConfig {
        ChannelConfig {
            id: "ch-1".into(),
            name: "Phone".into(),
            enabled: true,
            kind: kind.into(),
            agents: agents.iter().map(|a| a.to_string()).collect(),
            approvals: false,
            options: options.as_object().cloned().unwrap_or_default(),
        }
    }

    #[test]
    fn channels_are_checked_by_their_kind() {
        let s = settings();
        let m = |people: serde_json::Value, hs: &str| {
            channel(
                "matrix",
                &["a"],
                json!({ "homeserver": hs, "user_id": "@bot:example.org", "people": people }),
            )
        };
        let pw = [("password", "pw")];
        let ok = m(json!(["@me:example.org"]), "https://matrix.example.org");
        assert!(check(&s, &ok, &prepared(&ok, &pw), &[]).is_ok());
        assert!(
            check(&s, &ok, &prepared(&ok, &[]), &[]).is_err(),
            "no password"
        );
        let c = m(json!([]), "https://matrix.example.org");
        assert!(check(&s, &c, &prepared(&c, &pw), &[]).is_err(), "nobody");
        let c = m(json!(["@bot:example.org"]), "https://matrix.example.org");
        assert!(
            check(&s, &c, &prepared(&c, &pw), &[]).is_err(),
            "the bot itself"
        );
        let c = m(json!(["@me:example.org"]), "http://matrix.example.org");
        assert!(
            check(&s, &c, &prepared(&c, &pw), &[]).is_err(),
            "plain http far away"
        );
        let c = m(json!(["@me:example.org"]), "http://192.168.1.5:8008");
        assert!(
            check(&s, &c, &prepared(&c, &pw), &[]).is_ok(),
            "plain http nearby"
        );
        // Matrix talks for several agents; Telegram for one.
        let mut two = ok.clone();
        two.agents.push("b".into());
        assert!(check(&s, &two, &prepared(&two, &pw), &[]).is_ok());
        let mut gone = ok.clone();
        gone.agents = vec!["gone".into()];
        assert!(check(&s, &gone, &prepared(&gone, &pw), &[]).is_err());

        let token = [("token", "123456789:AAHdqTcvCH1vGWJxfSeofSAs0K5PALDsaw")];
        let t = channel("telegram", &["a"], json!({}));
        assert!(check(&s, &t, &prepared(&t, &token), &[]).is_ok());
        // Another channel with the same bot.
        assert!(
            check(
                &s,
                &t,
                &prepared(&t, &token),
                &["telegram:123456789".into()]
            )
            .is_err()
        );
        assert!(check(&s, &t, &prepared(&t, &[("token", "nope")]), &[]).is_err());
        let t2 = channel("telegram", &["a", "b"], json!({}));
        assert!(
            check(&s, &t2, &prepared(&t2, &token), &[]).is_err(),
            "one agent"
        );
        let big = channel("telegram", &["a"], json!({ "upload_mb": 50 }));
        assert!(
            check(&s, &big, &prepared(&big, &token), &[]).is_err(),
            "20 MB at most"
        );
    }

    #[test]
    fn channels_read_and_write_with_their_kind() {
        let c: ChannelConfig = serde_json::from_value(json!({
            "id": "t", "name": "T", "type": "telegram", "agents": ["a"],
            "options": { "stream": false }
        }))
        .unwrap();
        assert!(c.enabled && !c.approvals);
        let v = serde_json::to_value(&c).unwrap();
        assert_eq!(v["type"], "telegram");
        assert_eq!(v["options"]["stream"], false);
    }
}
