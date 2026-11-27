//! Channels in the app (Settings → Channels): talking to an agent from
//! outside Chummy, through the same code as chummyd (`chummy-channel`):
//! [`Conversations`] for how conversations behave, and one implementation
//! per kind of channel (Matrix so far).
//!
//! - Each channel's password, and a key that seals its login
//!   (`channels/<id>/login.sealed`) and opens its Matrix stores
//!   (`channels/<id>/matrix/`), are in the keychain. So channels connect
//!   again when Chummy starts, without asking anything.
//! - One task handles what people send, the session's updates and the
//!   questions, in order. Runs started from a channel are ordinary chat
//!   runs: they show in the sessions pane, and their chat (one per person,
//!   `m-…`) in the agent's chats.
//! - Approvals from a channel's runs are answered in Chummy's window unless
//!   the channel allows them over Matrix.

use std::collections::{BTreeMap, HashMap};
use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use chummy_channel::matrix::{MatrixHandle, MatrixHost, MatrixSettings, Status, VerifyEvent};
use chummy_channel::{Approvals, Conversations, FromMatrix, Hub, MatrixOutlet, Outlet};
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
    Matrix(String, FromMatrix),
    Update(Box<Update>),
    Ask(AskEvent),
}

/// The keychain entries of a channel.
pub fn password_key(id: &str) -> String {
    format!("channel:{id}:password")
}
fn seal_key(id: &str) -> String {
    format!("channel:{id}:key")
}

/// A channel's own folder, in the Chummy folder.
pub fn dir(state: &AppState, id: &str) -> PathBuf {
    state.dirs.root.join("channels").join(id)
}

/// A channel as the window shows it: its state now.
#[derive(Debug, Clone, Default, Serialize)]
pub struct Live {
    /// "off", "connecting", "connected", "problem".
    pub state: String,
    pub message: Option<String>,
    /// Emoji verifications waiting for an answer.
    pub verifications: Vec<Waiting>,
    /// The latest thing said about verifying.
    pub note: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct Waiting {
    pub n: u32,
    pub who: String,
    pub emoji: Vec<(String, String)>,
}

struct Running {
    config: ChannelConfig,
    password: Zeroizing<String>,
    handle: MatrixHandle,
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

    /// Answers an emoji verification.
    pub fn verify(&self, channel: &str, n: u32, matches: bool) -> Result<(), String> {
        let running = lock(&self.running);
        let r = running.get(channel).ok_or("That channel isn't running.")?;
        r.handle.verify(Some(n), matches);
        drop(running);
        self.set_live(channel, |l| l.verifications.retain(|w| w.n != n));
        Ok(())
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
                Input::Matrix(id, FromMatrix::Ready { convs: c }) => convs.set_convs(&id, c),
                Input::Matrix(
                    _,
                    FromMatrix::Message {
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
                                .get(&password_key(&id))
                                .ok()
                                .flatten()
                                .is_some_and(|p| p == *r.password)
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
        let password = match state.secrets.get(&password_key(&c.id)) {
            Ok(Some(p)) => Zeroizing::new(p),
            Ok(None) => {
                live.insert(
                    c.id.clone(),
                    problem("No password saved: edit it to enter one."),
                );
                continue;
            }
            Err(e) => {
                live.insert(c.id.clone(), problem(&e.to_string()));
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
        let ChannelKind::Matrix(m) = &c.kind;
        let settings = MatrixSettings {
            channel: c.id.clone(),
            homeserver: m.homeserver.clone(),
            user_id: m.user_id.clone(),
            password: password.clone(),
            users: [(agent.id.clone(), m.users.clone())].into(),
            upload_mb: m.upload_mb,
            label: "Chummy".into(),
            device: format!("Chummy ({})", c.name),
            chat_scope: c.id.clone(),
            name: "Chummy".into(),
        };
        let host = Arc::new(AppMatrix {
            app: app.clone(),
            channel: c.id.clone(),
        });
        let handle = chummy_channel::matrix::start(
            host,
            settings,
            dir(&state, &c.id).join("matrix"),
            chummy_channel::matrix::store_passphrase(&key),
        );
        let _ = ch.inbox.send(Input::Add(
            c.id.clone(),
            Arc::new(MatrixOutlet(handle.sender())),
        ));
        chummy_log::system::info("channels", format!("Started the channel {}", c.name));
        running.insert(
            c.id.clone(),
            Running {
                config: c.clone(),
                password,
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

/// Forgets a channel's login, keys and stores (when it's deleted).
pub fn forget(state: &AppState, id: &str) {
    let _ = state.secrets.delete(&password_key(id));
    let _ = state.secrets.delete(&seal_key(id));
    let _ = std::fs::remove_dir_all(dir(state, id));
}

/// Tells the windows the channels' state changed.
fn changed(app: &AppHandle) {
    if let Some(ch) = app.state::<AppState>().channels.get() {
        let _ = app.emit(events::CHANNELS_CHANGED, ch.live());
    }
}

/// The app's side of one Matrix channel.
struct AppMatrix {
    app: AppHandle,
    channel: String,
}

impl AppMatrix {
    fn state(&self) -> &AppState {
        self.app.state::<AppState>().inner()
    }

    fn login_path(&self) -> PathBuf {
        dir(self.state(), &self.channel).join("login.sealed")
    }

    fn aad(&self) -> String {
        format!("chummy.channel-login/1\n{}", self.channel)
    }

    fn set_live(&self, f: impl FnOnce(&mut Live)) {
        if let Some(ch) = self.state().channels.get() {
            ch.set_live(&self.channel, f);
            changed(&self.app);
        }
    }
}

impl MatrixHost for AppMatrix {
    fn agent_name(&self, id: &str) -> Option<String> {
        self.state().settings().agent(id).map(|a| a.name.clone())
    }

    fn kept(&self) -> Option<serde_json::Value> {
        let state = self.state();
        let plain = chummy_store::sealed::open(
            state.secrets.as_ref(),
            &seal_key(&self.channel),
            &self.login_path(),
            &self.aad(),
        )
        .ok()
        .flatten()?;
        serde_json::from_slice(&plain).ok()
    }

    fn keep(&self, kept: serde_json::Value) {
        let state = self.state();
        let plain = Zeroizing::new(serde_json::to_vec(&kept).unwrap_or_default());
        let r =
            chummy_channel::matrix::make_private_dir(&dir(state, &self.channel)).and_then(|()| {
                chummy_store::sealed::seal(
                    state.secrets.as_ref(),
                    &seal_key(&self.channel),
                    &self.login_path(),
                    &self.aad(),
                    &plain,
                )
            });
        if let Err(e) = r {
            chummy_log::system::error("channels", format!("Couldn't keep a channel's login: {e}"));
        }
    }

    fn received(&self, m: FromMatrix) {
        if let Some(ch) = self.state().channels.get() {
            let _ = ch.inbox.send(Input::Matrix(self.channel.clone(), m));
        }
    }

    fn verification(&self, v: VerifyEvent) {
        match v {
            VerifyEvent::Waiting { n, who, emoji } => {
                crate::notify::show(
                    &self.app,
                    "Verify Chummy in Matrix",
                    &format!(
                        "{who} wants to verify Chummy: compare the emoji in Settings → Channels."
                    ),
                );
                self.set_live(|l| {
                    l.verifications.push(Waiting { n, who, emoji });
                    l.note = None;
                });
            }
            VerifyEvent::Over { n } => self.set_live(|l| l.verifications.retain(|w| w.n != n)),
            VerifyEvent::Note { text } => self.set_live(|l| l.note = Some(text)),
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
        chummy_channel::matrix::save_to_folder(&AppPlatform::new(&self.app), agent, name, data)
    }
}
