//! Channels in the app (Settings → Channels): talking to agents from
//! outside Chummy, through the same code as chummyd (`chummy-channel`):
//! [`Conversations`] for how conversations behave, and the kinds of channel
//! (`chummy_channel::ChannelKind`) for everything particular to each. None
//! of this names a kind.
//!
//! - A channel's secret fields are in the keychain (`channel:<id>:<field>`),
//!   with a key that seals what it keeps (`channels/<id>/kept.sealed`: a
//!   login, the people let in) and that it may use for its own stores in
//!   its folder (`channels/<id>/`). So channels connect again when Chummy
//!   starts, without asking anything.
//! - One task handles what people send, the session's updates and the
//!   questions, in order. Runs started from a channel are ordinary chat
//!   runs: they show in the sessions pane, and their chat (one per person,
//!   `m-…`) in the agent's chats.
//! - Approvals from a channel's runs are answered in Chummy's window unless
//!   the channel allows them there.
//! - Verifying in the app: what a channel asks ([`Verify`]: emoji to
//!   compare, someone to let in…) shows in Settings → Channels, with a
//!   notification, and is answered there; invitations are made there too.

use std::collections::{BTreeMap, HashMap};
use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use chummy_channel::{
    Approvals, ChannelHost, ChannelKind, ChannelOutlet, Conversations, Event, FromChannel, Handle,
    Hub, Input as Field, Invite, Options, Outlet, Secrets, Setup, Start, Status, ToChannel, Verify,
};
use chummy_scheduler::{AskEvent, JobView};
use chummy_session::Update;
use serde::Serialize;
use tauri::{AppHandle, Emitter, Manager};
use tokio::sync::mpsc::UnboundedSender;
use zeroize::Zeroizing;

use crate::events;
use crate::platform::AppPlatform;
use crate::settings::{ChannelConfig, Settings};
use crate::state::{AppState, lock};

/// What the channels' task handles, in order.
enum Input {
    Add(String, Arc<dyn Outlet>),
    Remove(String),
    Channel(String, FromChannel),
    Update(Box<Update>),
    Ask(AskEvent),
}

/// The keychain entry of a channel's secret field.
pub fn secret_key(id: &str, field: &str) -> String {
    format!("channel:{id}:{field}")
}
fn seal_key(id: &str) -> String {
    format!("channel:{id}:key")
}

/// A channel's own folder, in the Chummy folder.
pub fn dir(state: &AppState, id: &str) -> PathBuf {
    state.dirs.root.join("channels").join(id)
}

fn kept_path(state: &AppState, id: &str) -> PathBuf {
    dir(state, id).join("kept.sealed")
}

fn aad(id: &str) -> String {
    format!("chummy.channel-kept/1\n{id}")
}

/// What a channel kept (sealed), if anything.
pub fn kept(state: &AppState, id: &str) -> Option<serde_json::Value> {
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

/// A channel ready to be checked, described or started: its kind, the
/// agents' names, its options and its secrets (saved, or typed now).
pub struct Prepared {
    pub kind: &'static dyn ChannelKind,
    pub agent: String,
    pub options: Options,
    pub secrets: Secrets,
}

impl Prepared {
    pub fn setup(&self) -> Setup<'_> {
        Setup {
            agent: &self.agent,
            options: &self.options,
            secrets: &self.secrets,
            program: "Chummy",
        }
    }
}

/// Prepares a channel: `typed` secrets (non-empty) take the place of saved
/// ones.
pub fn prepare(
    state: &AppState,
    s: &Settings,
    c: &ChannelConfig,
    typed: &BTreeMap<String, String>,
) -> Result<Prepared, String> {
    let kind = chummy_channel::kind(&c.kind).ok_or_else(|| {
        format!(
            "“{}” is a kind of channel this version doesn't know.",
            c.name
        )
    })?;
    let mut secrets = Secrets::new();
    for f in kind.fields() {
        if f.input != Field::Secret {
            continue;
        }
        let v = match typed.get(f.key).map(|v| v.trim()).filter(|v| !v.is_empty()) {
            Some(v) => Some(v.to_string()),
            None => state
                .secrets
                .get(&secret_key(&c.id, f.key))
                .map_err(|e| e.to_string())?,
        };
        if let Some(v) = v {
            secrets.insert(f.key.to_string(), Zeroizing::new(v));
        }
    }
    let names: Vec<String> = c
        .agents
        .iter()
        .map(|a| {
            s.agent(a)
                .map(|a| a.name.clone())
                .unwrap_or_else(|| a.clone())
        })
        .collect();
    Ok(Prepared {
        kind,
        agent: chummy_channel::kind::names(&names),
        options: chummy_channel::kind::tidy(kind, &c.options),
        secrets,
    })
}

/// A channel as the window shows it: its state now.
#[derive(Debug, Clone, Default, Serialize)]
pub struct Live {
    /// "off", "connecting", "connected", "problem".
    pub state: String,
    pub message: Option<String>,
    /// What it asks to verify, waiting for an answer.
    pub pending: Vec<Verify>,
    /// The latest thing it said about verifying or people.
    pub note: Option<String>,
    /// Its account, as people find it ("@helper_bot").
    pub account: Option<String>,
    /// The invitation made last.
    pub invite: Option<Invite>,
}

struct Running {
    config: ChannelConfig,
    secrets: Secrets,
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
    pub fn pending(&self, channel: &str, n: u32) -> Option<Verify> {
        lock(&self.live)
            .get(channel)
            .and_then(|l| l.pending.iter().find(|p| p.n == n).cloned())
    }

    /// Answers what a channel asks to verify.
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

    /// A new invitation, for a channel whose people join by one.
    pub async fn invite(&self, channel: &str) -> Result<Invite, String> {
        let tx = self
            .sender(channel)
            .ok_or("Turn the channel on first: the invitation is to it.")?;
        let invite = chummy_channel::invite(&tx).await?;
        self.set_live(channel, |l| l.invite = Some(invite.clone()));
        Ok(invite)
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

/// Stops someone talking through a channel (kinds that let people in
/// themselves), which tells them. The channel has to be running.
pub fn remove_person(app: &AppHandle, channel: &str, id: &str) -> Result<(), String> {
    let state = app.state::<AppState>();
    let ch = state.channels.get().ok_or("Channels aren't running yet.")?;
    let removes = state
        .settings()
        .channels
        .iter()
        .find(|c| c.id == channel)
        .and_then(|c| chummy_channel::kind(&c.kind))
        .is_some_and(|k| k.removes_people());
    if !removes {
        return Err("People are changed in this channel's settings.".into());
    }
    if !ch.remove_person(channel, id) {
        return Err("Turn the channel on to remove someone: they're told.".into());
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
    let none = BTreeMap::new();
    let mut running = lock(&ch.running);
    // Stop what's off, gone or changed (its settings or its secrets).
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
                            && prepare(&state, &settings, c, &none)
                                .is_ok_and(|p| p.secrets == r.secrets)
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
        if c.agents.is_empty() || c.agents.iter().any(|a| settings.agent(a).is_none()) {
            live.insert(
                c.id.clone(),
                problem("One of its agents was deleted: choose again."),
            );
            continue;
        }
        // What its kind needs, checked as when it was saved.
        let p = match prepare(&state, &settings, c, &none).and_then(|p| {
            chummy_channel::kind::check(p.kind, &p.setup())?;
            Ok(p)
        }) {
            Ok(p) => p,
            Err(e) => {
                live.insert(c.id.clone(), problem(&format!("{e} Edit it to fix that.")));
                continue;
            }
        };
        let key = match chummy_store::sealed::key(state.secrets.as_ref(), &seal_key(&c.id)) {
            Ok(k) => k,
            Err(e) => {
                live.insert(c.id.clone(), problem(&e));
                continue;
            }
        };
        let folder = dir(&state, &c.id);
        if let Err(e) = chummy_channel::make_private_dir(&folder) {
            live.insert(c.id.clone(), problem(&e));
            continue;
        }
        let host = Arc::new(AppChannel {
            app: app.clone(),
            channel: c.id.clone(),
        });
        let start = Start {
            channel: c.id.clone(),
            agents: c.agents.clone(),
            options: p.options.clone(),
            secrets: p.secrets.clone(),
            folder,
            key: match <[u8; 32]>::try_from(key.as_slice()) {
                Ok(k) => Zeroizing::new(k),
                Err(_) => {
                    live.insert(c.id.clone(), problem("Its key isn't valid."));
                    continue;
                }
            },
            program: "Chummy".into(),
            place: c.name.clone(),
        };
        let handle = match p.kind.start(host, start) {
            Ok(h) => h,
            Err(e) => {
                live.insert(c.id.clone(), problem(&e));
                continue;
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
                secrets: p.secrets,
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

/// Forgets a channel's secrets, what it kept and its folder (when it's
/// deleted).
pub fn forget(state: &AppState, c: &ChannelConfig) {
    if let Some(k) = chummy_channel::kind(&c.kind) {
        for f in k.fields().iter().filter(|f| f.input == Field::Secret) {
            let _ = state.secrets.delete(&secret_key(&c.id, f.key));
        }
    }
    let _ = state.secrets.delete(&seal_key(&c.id));
    let _ = std::fs::remove_dir_all(dir(state, &c.id));
}

/// Tells the windows the channels' state changed.
pub fn changed(app: &AppHandle) {
    if let Some(ch) = app.state::<AppState>().channels.get() {
        let _ = app.emit(events::CHANNELS_CHANGED, ch.live());
    }
}

/// The app's side of one channel, whatever its kind.
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

    fn name(&self) -> String {
        self.state()
            .settings()
            .channels
            .iter()
            .find(|c| c.id == self.channel)
            .map(|c| c.name.clone())
            .unwrap_or_default()
    }
}

impl ChannelHost for AppChannel {
    fn agent_name(&self, id: &str) -> Option<String> {
        self.state().settings().agent(id).map(|a| a.name.clone())
    }

    fn kept(&self) -> Option<serde_json::Value> {
        kept(self.state(), &self.channel)
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
            // Verifying in the app: a card in Settings → Channels, and a
            // notification.
            Event::Verify(v) => {
                crate::notify::show(
                    &self.app,
                    &format!("“{}” asks you to check something", self.name()),
                    &format!("{} {} Answer in Settings → Channels.", v.who, v.ask),
                );
                self.set_live(|l| {
                    l.pending.retain(|w| w.n != v.n);
                    l.pending.push(v);
                    // An invitation is used once.
                    l.invite = None;
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
