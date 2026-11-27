//! Channels in the app (Settings → Channels): talking to an agent from
//! outside Chummy, through the same code as chummyd (`chummy-channel`):
//! [`Conversations`] for how conversations behave, and one implementation
//! per kind of channel (Matrix and Telegram).
//!
//! - Each channel's secret (Matrix: the bot's password; Telegram: the bot's
//!   token) is in the keychain, with a key that seals what the channel
//!   keeps (`channels/<id>/login.sealed`: Matrix's session and rooms,
//!   Telegram's people let in) and opens Matrix's stores
//!   (`channels/<id>/matrix/`). So channels connect again when Chummy
//!   starts, without asking anything.
//! - One task handles what people send, the session's updates and the
//!   questions, in order. Runs started from a channel are ordinary chat
//!   runs: they show in the sessions pane, and their chat (one per person,
//!   `m-…`) in the agent's chats.
//! - Approvals from a channel's runs are answered in Chummy's window unless
//!   the channel allows them there.
//! - What waits for the user (Matrix emoji to compare, people asking to be
//!   let in to a Telegram bot) shows in Settings → Channels, with a
//!   notification.

use std::collections::{BTreeMap, HashMap};
use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use chummy_channel::matrix::MatrixSettings;
use chummy_channel::telegram::TelegramSettings;
use chummy_channel::{
    Approvals, ChannelHost, ChannelOutlet, Code, Conversations, Event, FromChannel, Handle, Hub,
    Outlet, Person, Status, ToChannel,
};
use chummy_scheduler::{AskEvent, JobView};
use chummy_session::Update;
use serde::Serialize;
use tauri::{AppHandle, Emitter, Manager};
use tokio::sync::mpsc::UnboundedSender;
use zeroize::Zeroizing;

use crate::events;
use crate::platform::AppPlatform;
use crate::settings::{ChannelConfig, ChannelKind};
use crate::state::{AppState, lock};

/// What the channels' task handles, in order.
enum Input {
    Add(String, Arc<dyn Outlet>),
    Remove(String),
    Channel(String, FromChannel),
    Update(Box<Update>),
    Ask(AskEvent),
}

/// The keychain entries of a channel: its secret (password or token), and
/// the key that seals what it keeps.
pub fn secret_key(id: &str) -> String {
    format!("channel:{id}:password")
}
fn seal_key(id: &str) -> String {
    format!("channel:{id}:key")
}

/// A channel's own folder, in the Chummy folder.
pub fn dir(state: &AppState, id: &str) -> PathBuf {
    state.dirs.root.join("channels").join(id)
}

fn kept_path(state: &AppState, id: &str) -> PathBuf {
    dir(state, id).join("login.sealed")
}

fn aad(id: &str) -> String {
    format!("chummy.channel-login/1\n{id}")
}

/// What a channel kept (sealed), if anything.
fn read_kept(state: &AppState, id: &str) -> Option<serde_json::Value> {
    let plain = chummy_store::sealed::open(
        state.secrets.as_ref(),
        &seal_key(id),
        &kept_path(state, id),
        &aad(id),
    )
    .ok()
    .flatten()?;
    serde_json::from_slice(&plain).ok()
}

fn write_kept(state: &AppState, id: &str, kept: &serde_json::Value) -> Result<(), String> {
    let plain = Zeroizing::new(serde_json::to_vec(kept).unwrap_or_default());
    chummy_channel::make_private_dir(&dir(state, id))?;
    chummy_store::sealed::seal(
        state.secrets.as_ref(),
        &seal_key(id),
        &kept_path(state, id),
        &aad(id),
        &plain,
    )
}

/// The people let in to a Telegram channel (kept even while it's off).
pub fn people(state: &AppState, id: &str) -> Vec<Person> {
    read_kept(state, id)
        .map(|k| chummy_channel::telegram::people(&k))
        .unwrap_or_default()
}

/// A channel as the window shows it: its state now.
#[derive(Debug, Clone, Default, Serialize)]
pub struct Live {
    /// "off", "connecting", "connected", "problem".
    pub state: String,
    pub message: Option<String>,
    /// What waits for an answer: emoji to compare, people to let in.
    pub pending: Vec<Pending>,
    /// The latest thing said about verifying or pairing.
    pub note: Option<String>,
    /// The bot, as people find it ("@helper_bot").
    pub account: Option<String>,
    /// The pairing code made last (Telegram).
    pub code: Option<Code>,
}

#[derive(Debug, Clone, Serialize)]
pub struct Pending {
    pub n: u32,
    /// "emoji" or "pairing".
    pub kind: &'static str,
    pub who: String,
    /// Emoji to compare, and their names.
    pub emoji: Vec<(String, String)>,
}

struct Running {
    config: ChannelConfig,
    secret: Zeroizing<String>,
    handle: Handle,
}

/// The channels running in the app.
pub struct Channels {
    inbox: UnboundedSender<Input>,
    running: Mutex<HashMap<String, Running>>,
    live: Mutex<BTreeMap<String, Live>>,
}

impl Channels {
    /// The state each channel is in.
    pub fn live(&self) -> BTreeMap<String, Live> {
        lock(&self.live).clone()
    }

    /// What waits as `n` on a channel.
    pub fn pending(&self, channel: &str, n: u32) -> Option<Pending> {
        lock(&self.live)
            .get(channel)
            .and_then(|l| l.pending.iter().find(|p| p.n == n).cloned())
    }

    /// Answers what waits: emoji that match (or don't), someone to let in
    /// (or not).
    pub fn answer(&self, channel: &str, n: u32, yes: bool) -> Result<(), String> {
        let running = lock(&self.running);
        let r = running.get(channel).ok_or("That channel isn't running.")?;
        r.handle.answer(Some(n), yes);
        drop(running);
        self.set_live(channel, |l| l.pending.retain(|w| w.n != n));
        Ok(())
    }

    fn sender(&self, channel: &str) -> Option<UnboundedSender<ToChannel>> {
        lock(&self.running).get(channel).map(|r| r.handle.sender())
    }

    /// A new pairing code for a Telegram channel.
    pub async fn pairing_code(&self, channel: &str) -> Result<Code, String> {
        let tx = self
            .sender(channel)
            .ok_or("Turn the channel on first: the code goes to its bot.")?;
        let (otx, orx) = tokio::sync::oneshot::channel();
        tx.send(ToChannel::NewCode(otx))
            .map_err(|_| "The channel isn't running.".to_string())?;
        let code = tokio::time::timeout(std::time::Duration::from_secs(10), orx)
            .await
            .map_err(|_| "The channel didn't answer.".to_string())?
            .map_err(|_| "The channel isn't running.".to_string())??;
        self.set_live(channel, |l| l.code = Some(code.clone()));
        Ok(code)
    }

    /// Stops someone talking through a running channel; `false`: it isn't
    /// running.
    pub fn remove_person(&self, channel: &str, id: &str) -> bool {
        match self.sender(channel) {
            Some(tx) => tx.send(ToChannel::Remove { id: id.into() }).is_ok(),
            None => false,
        }
    }

    /// Updates from the session go to the channels too.
    pub fn update(&self, u: &Update) {
        if !lock(&self.running).is_empty() {
            let _ = self.inbox.send(Input::Update(Box::new(u.clone())));
        }
    }

    fn set_live(&self, channel: &str, f: impl FnOnce(&mut Live)) {
        f(lock(&self.live).entry(channel.to_string()).or_default());
    }
}

/// Stops someone talking to a Telegram channel's agent: through the
/// channel when it runs (they're told), else in what it keeps.
pub fn remove_person(app: &AppHandle, channel: &str, id: &str) -> Result<(), String> {
    let state = app.state::<AppState>();
    let ch = state.channels.get().ok_or("Channels aren't running yet.")?;
    if !ch.remove_person(channel, id) {
        let mut kept = read_kept(&state, channel).ok_or("Nobody is let in to that channel.")?;
        if let Some(people) = kept.get_mut("people").and_then(|p| p.as_array_mut()) {
            people.retain(|p| p.get("id").and_then(|v| v.as_str()) != Some(id));
        }
        write_kept(&state, channel, &kept)?;
    }
    changed(app);
    Ok(())
}

/// The app's rules for conversations.
struct AppHub {
    p: AppPlatform,
}

impl AppHub {
    fn state(&self) -> &AppState {
        self.p.app.state::<AppState>().inner()
    }
}

impl Hub for AppHub {
    type P = AppPlatform;

    fn platform(&self) -> &AppPlatform {
        &self.p
    }

    fn agent_name(&self, id: &str) -> String {
        self.state()
            .settings()
            .agent(id)
            .map(|a| a.name.clone())
            .unwrap_or_else(|| id.to_string())
    }

    /// In the channel when it allows them; otherwise in Chummy's window.
    fn approvals(&self, _agent: &str, channel: &str) -> Approvals {
        let here = self
            .state()
            .settings()
            .channels
            .iter()
            .any(|c| c.id == channel && c.approvals);
        if here {
            Approvals::Here
        } else {
            Approvals::Elsewhere
        }
    }

    /// Chummy's window answers them.
    fn refuse_unseen_approvals(&self) -> bool {
        false
    }

    fn jobs(&self, agent: &str) -> Vec<JobView> {
        self.state()
            .jobs
            .get()
            .map(|m| {
                m.overview()
                    .jobs
                    .into_iter()
                    .filter(|j| j.job.agent == agent)
                    .collect()
            })
            .unwrap_or_default()
    }

    fn run_job(&self, id: &str) -> Result<(), String> {
        self.state()
            .jobs
            .get()
            .ok_or("Jobs aren't running yet.")?
            .run_now(id)
    }

    fn job_reports(&self, _job: &str) -> Vec<(String, String)> {
        vec![]
    }
}

/// Starts the channels' task, then the channels that are on.
pub fn start(app: &AppHandle) {
    let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel::<Input>();
    let state = app.state::<AppState>();
    if state
        .channels
        .set(Channels {
            inbox: tx.clone(),
            running: Mutex::new(HashMap::new()),
            live: Mutex::new(BTreeMap::new()),
        })
        .is_err()
    {
        return;
    }
    let p = AppPlatform::new(app);
    tauri::async_runtime::spawn(async move {
        let mut convs = Conversations::new(AppHub { p });
        while let Some(input) = rx.recv().await {
            match input {
                Input::Add(id, outlet) => convs.add_channel(&id, outlet),
                Input::Remove(id) => convs.remove_channel(&id),
                Input::Channel(id, FromChannel::Ready { convs: c }) => convs.set_convs(&id, c),
                Input::Channel(
                    _,
                    FromChannel::Message {
                        conv,
                        text,
                        answers,
                    },
                ) => {
                    convs.receive(&conv, &text, answers);
                }
                Input::Update(u) => convs.update(&u),
                Input::Ask(AskEvent::Asked { pending }) => convs.asked(&pending),
                Input::Ask(AskEvent::Closed { id, .. }) => convs.closed(&id),
            }
        }
    });
    // Questions, to the channels.
    let mut asks = state.session.asks.subscribe();
    tauri::async_runtime::spawn(async move {
        loop {
            match asks.recv().await {
                Ok(e) => {
                    if tx.send(Input::Ask(e)).is_err() {
                        return;
                    }
                }
                Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => continue,
                Err(_) => return,
            }
        }
    });
    sync(app);
}

/// Makes the running channels match the settings: starts the ones that
/// are on (and ready), stops the rest, and restarts changed ones.
pub fn sync(app: &AppHandle) {
    let app = app.clone();
    tauri::async_runtime::spawn(async move { sync_now(&app) });
}

fn sync_now(app: &AppHandle) {
    let state = app.state::<AppState>();
    let Some(ch) = state.channels.get() else {
        return;
    };
    let settings = state.settings();
    let mut running = lock(&ch.running);
    // Stop what's off, gone or changed.
    let ids: Vec<String> = running.keys().cloned().collect();
    for id in ids {
        let keep = settings
            .channels
            .iter()
            .find(|c| c.id == id)
            .is_some_and(|c| {
                c.enabled
                    && running.get(&id).is_some_and(|r| {
                        r.config == *c
                            && state
                                .secrets
                                .get(&secret_key(&id))
                                .ok()
                                .flatten()
                                .is_some_and(|p| p == *r.secret)
                    })
            });
        if !keep && let Some(r) = running.remove(&id) {
            r.handle.stop();
            let _ = ch.inbox.send(Input::Remove(id.clone()));
            ch.set_live(&id, |l| *l = Live::default());
            chummy_log::system::info("channels", format!("Stopped the channel {}", r.config.name));
        }
    }
    let mut live = BTreeMap::new();
    for c in &settings.channels {
        if !c.enabled {
            live.insert(c.id.clone(), off("Off"));
            continue;
        }
        if running.contains_key(&c.id) {
            continue;
        }
        let Some(agent) = settings.agent(&c.agent) else {
            live.insert(
                c.id.clone(),
                problem("Its agent was deleted: choose another."),
            );
            continue;
        };
        let secret = match state.secrets.get(&secret_key(&c.id)) {
            Ok(Some(p)) => Zeroizing::new(p),
            Ok(None) => {
                live.insert(
                    c.id.clone(),
                    problem(match c.kind {
                        ChannelKind::Matrix(_) => "No password saved: edit it to enter one.",
                        ChannelKind::Telegram(_) => "No bot token saved: edit it to enter one.",
                    }),
                );
                continue;
            }
            Err(e) => {
                live.insert(c.id.clone(), problem(&e.to_string()));
                continue;
            }
        };
        let host = Arc::new(AppChannel {
            app: app.clone(),
            channel: c.id.clone(),
        });
        let handle = match &c.kind {
            ChannelKind::Matrix(m) => {
                let key = match chummy_store::sealed::key(state.secrets.as_ref(), &seal_key(&c.id))
                {
                    Ok(k) => k,
                    Err(e) => {
                        live.insert(c.id.clone(), problem(&e));
                        continue;
                    }
                };
                let settings = MatrixSettings {
                    channel: c.id.clone(),
                    homeserver: m.homeserver.clone(),
                    user_id: m.user_id.clone(),
                    password: secret.clone(),
                    users: [(agent.id.clone(), m.users.clone())].into(),
                    upload_mb: m.upload_mb,
                    label: "Chummy".into(),
                    device: format!("Chummy ({})", c.name),
                    chat_scope: c.id.clone(),
                    name: "Chummy".into(),
                };
                chummy_channel::matrix::start(
                    host,
                    settings,
                    dir(&state, &c.id).join("matrix"),
                    chummy_channel::matrix::store_passphrase(&key),
                )
            }
            ChannelKind::Telegram(t) => {
                let settings = TelegramSettings {
                    channel: c.id.clone(),
                    api_url: t.api_url.clone(),
                    token: secret.clone(),
                    agent: agent.id.clone(),
                    upload_mb: t.upload_mb,
                    stream: t.stream,
                    chat_scope: c.id.clone(),
                    name: "Chummy".into(),
                };
                match chummy_channel::telegram::start(host, settings) {
                    Ok(h) => h,
                    Err(e) => {
                        live.insert(c.id.clone(), problem(&e));
                        continue;
                    }
                }
            }
        };
        let _ = ch.inbox.send(Input::Add(
            c.id.clone(),
            Arc::new(ChannelOutlet(handle.sender())),
        ));
        chummy_log::system::info("channels", format!("Started the channel {}", c.name));
        running.insert(
            c.id.clone(),
            Running {
                config: c.clone(),
                secret,
                handle,
            },
        );
    }
    drop(running);
    {
        let mut l = lock(&ch.live);
        l.retain(|id, _| settings.channels.iter().any(|c| &c.id == id));
        for (id, v) in live {
            l.insert(id, v);
        }
    }
    changed(app);
}

fn off(message: &str) -> Live {
    Live {
        state: "off".into(),
        message: Some(message.into()),
        ..Default::default()
    }
}

fn problem(message: &str) -> Live {
    Live {
        state: "problem".into(),
        message: Some(message.into()),
        ..Default::default()
    }
}

/// Forgets a channel's secret, what it kept and its stores (when it's
/// deleted).
pub fn forget(state: &AppState, id: &str) {
    let _ = state.secrets.delete(&secret_key(id));
    let _ = state.secrets.delete(&seal_key(id));
    let _ = std::fs::remove_dir_all(dir(state, id));
}

/// Tells the windows the channels' state changed.
pub fn changed(app: &AppHandle) {
    if let Some(ch) = app.state::<AppState>().channels.get() {
        let _ = app.emit(events::CHANNELS_CHANGED, ch.live());
    }
}

/// The app's side of one channel (Matrix or Telegram).
struct AppChannel {
    app: AppHandle,
    channel: String,
}

impl AppChannel {
    fn state(&self) -> &AppState {
        self.app.state::<AppState>().inner()
    }

    fn set_live(&self, f: impl FnOnce(&mut Live)) {
        if let Some(ch) = self.state().channels.get() {
            ch.set_live(&self.channel, f);
            changed(&self.app);
        }
    }

    fn agent_name(&self) -> String {
        let s = self.state().settings();
        s.channels
            .iter()
            .find(|c| c.id == self.channel)
            .and_then(|c| s.agent(&c.agent))
            .map(|a| a.name.clone())
            .unwrap_or_else(|| "an agent".into())
    }
}

impl ChannelHost for AppChannel {
    fn agent_name(&self, id: &str) -> Option<String> {
        self.state().settings().agent(id).map(|a| a.name.clone())
    }

    fn kept(&self) -> Option<serde_json::Value> {
        read_kept(self.state(), &self.channel)
    }

    fn keep(&self, kept: serde_json::Value) {
        if let Err(e) = write_kept(self.state(), &self.channel, &kept) {
            chummy_log::system::error(
                "channels",
                format!("Couldn't keep what a channel keeps: {e}"),
            );
        }
    }

    fn received(&self, m: FromChannel) {
        if let Some(ch) = self.state().channels.get() {
            let _ = ch.inbox.send(Input::Channel(self.channel.clone(), m));
        }
    }

    fn event(&self, e: Event) {
        match e {
            Event::Emoji { n, who, emoji } => {
                crate::notify::show(
                    &self.app,
                    "Verify Chummy in Matrix",
                    &format!(
                        "{who} wants to verify Chummy: compare the emoji in Settings → Channels."
                    ),
                );
                self.set_live(|l| {
                    l.pending.push(Pending {
                        n,
                        kind: "emoji",
                        who,
                        emoji,
                    });
                    l.note = None;
                });
            }
            Event::Pairing { n, who } => {
                crate::notify::show(
                    &self.app,
                    "Someone wants to talk to an agent",
                    &format!(
                        "{who} sent the pairing code for {} in Telegram: let them in, or not, in Settings → Channels.",
                        self.agent_name()
                    ),
                );
                self.set_live(|l| {
                    l.pending.push(Pending {
                        n,
                        kind: "pairing",
                        who,
                        emoji: vec![],
                    });
                    // The code is used up.
                    l.code = None;
                    l.note = None;
                });
            }
            Event::Over { n } => self.set_live(|l| l.pending.retain(|w| w.n != n)),
            Event::Note { text } => self.set_live(|l| l.note = Some(text)),
            // The window reads who's let in from what the channel keeps.
            Event::People { .. } => changed(&self.app),
            Event::Account { name } => self.set_live(|l| l.account = Some(name)),
        }
    }

    fn status(&self, s: Status) {
        self.set_live(|l| match s {
            Status::Connecting => {
                l.state = "connecting".into();
                l.message = None;
            }
            Status::Connected => {
                l.state = "connected".into();
                l.message = None;
            }
            Status::Problem(m) => {
                l.state = "problem".into();
                l.message = Some(m);
            }
        });
    }

    fn save_upload(&self, agent: &str, name: &str, data: &[u8]) -> Result<String, String> {
        chummy_channel::save_to_folder(&AppPlatform::new(&self.app), agent, name, data)
    }
}
