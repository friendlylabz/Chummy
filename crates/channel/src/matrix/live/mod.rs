//! Running a Matrix channel ([`super::Matrix`]): one end-to-end encrypted
//! room per agent and allowed person, which the program (chummyd or the
//! app) creates and invites them to. The same in both programs; what
//! differs is behind [`ChannelHost`].
//!
//! - It logs in as the bot with its password once, then keeps the session
//!   (access token, device) where the host keeps secrets
//!   ([`ChannelHost::keep`]: chummyd's sealed state, the app's sealed file), and
//!   the end-to-end keys in SQLite stores in a folder of the host's,
//!   encrypted with a passphrase the host gives. It sets up cross-signing
//!   once (the password answers the homeserver's check). Key backup is off:
//!   the keys never leave that folder.
//! - A message is acted on only when it's in one of those rooms, from that
//!   room's person, encrypted, and sent from a session that's verified
//!   (people verify the bot by emoji; someone confirms the emoji in the
//!   program: chummyd's terminal, the app's Channels settings). Everything
//!   else is ignored: other people, edits, reactions, redactions. Messages
//!   sent while the program was off for more than [`STALE`] get a note
//!   instead of a run.
//! - Invites to the bot are refused; a room someone else joined is
//!   ignored (with a warning) until they leave.
//! - If the homeserver can't be reached, it keeps trying, waiting longer
//!   each time.

mod files;
mod outbox;
mod verify;

use std::collections::{BTreeMap, HashMap, HashSet};
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use matrix_sdk::authentication::matrix::MatrixSession;
use matrix_sdk::config::{RequestConfig, SyncSettings};
use matrix_sdk::deserialized_responses::{EncryptionInfo, VerificationLevel, VerificationState};
use matrix_sdk::encryption::verification::SasVerification;
use matrix_sdk::ruma::api::client::room::create_room::v3::{
    Request as CreateRoomRequest, RoomPreset,
};
use matrix_sdk::ruma::api::client::uiaa;
use matrix_sdk::ruma::api::error::ErrorKind;
use matrix_sdk::ruma::events::InitialStateEvent;
use matrix_sdk::ruma::events::key::verification::request::ToDeviceKeyVerificationRequestEvent;
use matrix_sdk::ruma::events::room::encryption::RoomEncryptionEventContent;
use matrix_sdk::ruma::events::room::member::{MembershipState, StrippedRoomMemberEvent};
use matrix_sdk::ruma::events::room::message::{
    MessageType, OriginalSyncRoomMessageEvent, Relation,
};
use matrix_sdk::ruma::serde::Raw;
use matrix_sdk::ruma::{OwnedEventId, OwnedRoomId, OwnedUserId, RoomId, UserId};
use matrix_sdk::store::RoomLoadSettings;
use matrix_sdk::{Client, Room, RoomState};
use serde::{Deserialize, Serialize};
use tokio::sync::mpsc::UnboundedSender;
use tokio::sync::watch;
use zeroize::Zeroizing;

use crate::kind::Start;
use crate::link::{FromChannel, ToChannel};
use crate::{ChannelHost, Conv, Event, Handle, STALE, Status, make_private_dir};

/// One Matrix channel: the bot's account and who may talk to which agent.
#[derive(Clone)]
pub struct MatrixSettings {
    /// The channel's id (its conversations carry it).
    pub channel: String,
    /// The homeserver's address (checked already: https, or http only on
    /// this computer or the local network).
    pub homeserver: String,
    /// The bot's Matrix ID.
    pub user_id: String,
    /// The bot's password: to log in the first time, and to set up its
    /// cross-signing.
    pub password: Zeroizing<String>,
    /// Who may talk to each agent, by agent id (Matrix IDs).
    pub users: BTreeMap<String, Vec<String>>,
    /// Largest file accepted, in MB.
    pub upload_mb: u32,
    /// The program, as rooms are named: "<agent> (<label>)".
    pub label: String,
    /// The bot's session (device) name, as people see it when verifying.
    pub device: String,
    /// Keeps each person's chat apart from other channels' (empty: just
    /// the person; see [`crate::channel_chat`]).
    pub chat_scope: String,
    /// What the people are told to verify ("chummyd", "Chummy").
    pub name: String,
}

/// What's kept between runs.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
struct Kept {
    /// The bot the rest belongs to.
    user_id: String,
    #[serde(default)]
    session: Option<MatrixSession>,
    #[serde(default)]
    cross_signed: bool,
    /// Synced at least once (so what came before is history, not messages).
    #[serde(default)]
    synced: bool,
    #[serde(default)]
    rooms: Vec<KeptRoom>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
struct KeptRoom {
    agent: String,
    user: String,
    room: String,
}

/// What the Matrix side shares between its tasks.
pub struct Ctx {
    host: Arc<dyn ChannelHost>,
    cfg: MatrixSettings,
    /// To the outbox (notices from here go out in order too).
    out: UnboundedSender<ToChannel>,
    /// The rooms, by room id: (agent, person).
    rooms: Mutex<HashMap<OwnedRoomId, (String, String)>>,
    /// Question messages, by event: the question waiting.
    questions: Mutex<HashMap<OwnedEventId, String>>,
    /// Rooms told to verify, or told someone else is in them.
    told: Mutex<HashSet<(OwnedRoomId, &'static str)>>,
    verifications: Mutex<BTreeMap<u32, SasVerification>>,
}

fn lock<T>(m: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|e| e.into_inner())
}

impl Ctx {
    fn notice(&self, room: &str, text: String) {
        let _ = self.out.send(ToChannel::Notice {
            place: room.into(),
            text,
        });
    }

    /// Says something in a room once (while the program runs).
    fn tell_once(&self, room: &RoomId, what: &'static str, text: &str) {
        if lock(&self.told).insert((room.to_owned(), what)) {
            self.notice(room.as_str(), text.into());
        }
    }

    fn note(&self, text: impl Into<String>) {
        self.host.event(Event::Note { text: text.into() });
    }

    fn allowed_user(&self, user: &str) -> bool {
        self.cfg
            .users
            .values()
            .any(|us| us.iter().any(|u| u == user))
    }

    fn remember_question(&self, ev: OwnedEventId, ask: String) {
        lock(&self.questions).insert(ev, ask);
    }

    fn forget_question(&self, ask: &str) {
        lock(&self.questions).retain(|_, a| a != ask);
    }

    fn add_verification(&self, sas: SasVerification) -> u32 {
        let n = crate::next_number();
        lock(&self.verifications).insert(n, sas);
        n
    }

    fn remove_verification(&self, n: u32) {
        lock(&self.verifications).remove(&n);
    }

    fn take_verification(&self, n: Option<u32>) -> Result<SasVerification, String> {
        let mut v = lock(&self.verifications);
        let n = match n {
            Some(n) => n,
            None if v.len() == 1 => *v.keys().next().expect("one"),
            None if v.is_empty() => return Err("No verification is waiting.".into()),
            None => {
                return Err(format!(
                    "Several verifications are waiting ({}): say which, like /verify {} yes.",
                    v.keys()
                        .map(|k| k.to_string())
                        .collect::<Vec<_>>()
                        .join(", "),
                    v.keys().next().expect("some")
                ));
            }
        };
        v.remove(&n)
            .ok_or_else(|| format!("There's no verification {n} waiting."))
    }

    fn kept(&self) -> Kept {
        self.host
            .kept()
            .and_then(|v| serde_json::from_value(v).ok())
            .unwrap_or_default()
    }

    /// Changes what's kept, and keeps it now.
    fn keep(&self, f: impl FnOnce(&mut Kept)) {
        let mut k = self.kept();
        f(&mut k);
        if let Ok(v) = serde_json::to_value(&k) {
            self.host.keep(v);
        }
    }

    fn agent_name(&self, id: &str) -> String {
        self.host.agent_name(id).unwrap_or_else(|| id.into())
    }

    fn conv(&self, agent: &str, user: &str, room: &str) -> Conv {
        Conv {
            channel: self.cfg.channel.clone(),
            place: room.to_string(),
            agent: agent.to_string(),
            chat: crate::channel_chat(&self.cfg.chat_scope, user),
            who: user.to_string(),
            person: user.to_string(),
            console: false,
        }
    }
}

/// Starts a Matrix channel as a kind of channel ([`super::Matrix`]): from
/// its options (`homeserver`, `user_id`, `people`, `upload_mb`) and secret
/// (`password`), with its stores in its folder.
pub fn start_kind(host: Arc<dyn ChannelHost>, s: Start) -> Result<Handle, String> {
    let setup = s.setup("");
    let cfg = MatrixSettings {
        channel: s.channel.clone(),
        homeserver: setup.text("homeserver").to_string(),
        user_id: setup.text("user_id").to_string(),
        password: Zeroizing::new(setup.secret("password").to_string()),
        users: s
            .agents
            .iter()
            .map(|a| (a.clone(), setup.lines("people")))
            .collect(),
        upload_mb: setup.number("upload_mb"),
        label: s.place.clone(),
        device: format!("{} ({})", s.program, s.place),
        chat_scope: s.channel.clone(),
        name: s.program.clone(),
    };
    let store = s.folder.join("store");
    Ok(start(host, cfg, store, store_passphrase(&*s.key)))
}

/// Starts a Matrix channel. `store`: a private folder for its SQLite
/// stores, opened with `passphrase`. Must be called inside a Tokio runtime.
pub fn start(
    host: Arc<dyn ChannelHost>,
    cfg: MatrixSettings,
    store: PathBuf,
    passphrase: Zeroizing<String>,
) -> Handle {
    let (tx, rx) = tokio::sync::mpsc::unbounded_channel();
    let ctx = Arc::new(Ctx {
        host,
        cfg,
        out: tx.clone(),
        rooms: Mutex::new(HashMap::new()),
        questions: Mutex::new(HashMap::new()),
        told: Mutex::new(HashSet::new()),
        verifications: Mutex::new(BTreeMap::new()),
    });
    let (client_tx, client_rx) = watch::channel(None);
    let tasks = vec![
        tokio::spawn(outbox::run(ctx.clone(), rx, client_rx)),
        tokio::spawn(serve(ctx, store, passphrase, client_tx)),
    ];
    Handle { tx, tasks }
}

/// A passphrase for the SQLite stores, made from a secret key (so the
/// stores open only with it).
pub fn store_passphrase(key: &[u8]) -> Zeroizing<String> {
    use sha2::{Digest, Sha256};
    let mut h = Sha256::new();
    h.update(b"chummyd matrix store");
    h.update(key);
    Zeroizing::new(h.finalize().iter().map(|b| format!("{b:02x}")).collect())
}

/// Why serving stopped.
enum Stop {
    /// Try again after a while.
    Retry(String),
    /// The homeserver ended the session: log in afresh.
    LoggedOut,
}

async fn serve(
    ctx: Arc<Ctx>,
    store: PathBuf,
    pass: Zeroizing<String>,
    client_tx: watch::Sender<Option<Client>>,
) {
    let mut wait = Duration::from_secs(2);
    loop {
        ctx.host.status(Status::Connecting);
        let stop = match connect(&ctx, &store, &pass).await {
            Ok(client) => {
                wait = Duration::from_secs(2);
                serve_client(&ctx, client, &client_tx).await
            }
            Err(s) => s,
        };
        let _ = client_tx.send(None);
        match stop {
            Stop::LoggedOut => {
                chummy_log::system::warn(
                    "matrix",
                    "The homeserver ended the bot's session: logging in again",
                );
                ctx.keep(|k| {
                    k.session = None;
                    k.cross_signed = false;
                    k.synced = false;
                });
                wipe(&store);
                continue;
            }
            Stop::Retry(e) => {
                ctx.host.status(Status::Problem(e.clone()));
                chummy_log::system::warn(
                    "matrix",
                    format!(
                        "Matrix isn't reachable ({e}); trying again in {}s",
                        wait.as_secs()
                    ),
                );
                tokio::time::sleep(wait).await;
                wait = (wait * 2).min(Duration::from_secs(300));
            }
        }
    }
}

/// Removes the SQLite stores (their keys belong to a session that's gone).
fn wipe(store: &std::path::Path) {
    if let Err(e) = std::fs::remove_dir_all(store)
        && e.kind() != std::io::ErrorKind::NotFound
    {
        chummy_log::system::error(
            "matrix",
            format!("Couldn't remove the old Matrix store: {e}"),
        );
    }
}

fn stop_for(e: &matrix_sdk::Error) -> Stop {
    if matches!(e.client_api_error_kind(), Some(ErrorKind::UnknownToken(_))) {
        Stop::LoggedOut
    } else {
        Stop::Retry(e.to_string())
    }
}

fn stop_for_http(e: &matrix_sdk::HttpError) -> Stop {
    if matches!(e.client_api_error_kind(), Some(ErrorKind::UnknownToken(_))) {
        Stop::LoggedOut
    } else {
        Stop::Retry(e.to_string())
    }
}

/// Logs in (or carries on the kept session) and sets up cross-signing.
async fn connect(ctx: &Ctx, store: &std::path::Path, pass: &str) -> Result<Client, Stop> {
    let cfg = &ctx.cfg;
    // Another bot: what's kept belongs to the old one.
    if ctx.kept().user_id != cfg.user_id {
        wipe(store);
        ctx.keep(|k| {
            *k = Kept {
                user_id: cfg.user_id.clone(),
                ..Default::default()
            }
        });
    }
    make_private_dir(store).map_err(Stop::Retry)?;
    let http = chummy_http::builder()
        .connect_timeout(Duration::from_secs(20))
        .build()
        .map_err(|e| Stop::Retry(e.to_string()))?;
    let client = Client::builder()
        .homeserver_url(&cfg.homeserver)
        .http_client(http)
        .request_config(
            RequestConfig::new()
                .retry_limit(4)
                .timeout(Duration::from_secs(90)),
        )
        .sqlite_store(store, Some(pass))
        .build()
        .await
        .map_err(|e| Stop::Retry(e.to_string()))?;

    match ctx.kept().session {
        Some(s) => {
            client
                .matrix_auth()
                .restore_session(s, RoomLoadSettings::default())
                .await
                .map_err(|e| Stop::Retry(e.to_string()))?;
            client.whoami().await.map_err(|e| stop_for_http(&e))?;
        }
        None => {
            let device = ctx.cfg.device.clone();
            client
                .matrix_auth()
                .login_username(&cfg.user_id, &cfg.password)
                .initial_device_display_name(&device)
                .send()
                .await
                .map_err(|e| {
                    if matches!(e.client_api_error_kind(), Some(ErrorKind::Forbidden)) {
                        Stop::Retry("the homeserver refused the bot's password".into())
                    } else {
                        Stop::Retry(e.to_string())
                    }
                })?;
            let session = client
                .matrix_auth()
                .session()
                .ok_or_else(|| Stop::Retry("no session after logging in".into()))?;
            chummy_log::system::info(
                "matrix",
                format!(
                    "Logged in as {} (session {})",
                    cfg.user_id, session.meta.device_id
                ),
            );
            ctx.keep(|k| {
                k.session = Some(session);
                k.cross_signed = false;
                k.synced = false;
            });
        }
    }

    if !ctx.kept().cross_signed {
        cross_sign(ctx, &client).await?;
        ctx.keep(|k| k.cross_signed = true);
        chummy_log::system::info("matrix", "Set up cross-signing for the bot");
    }
    Ok(client)
}

async fn cross_sign(ctx: &Ctx, client: &Client) -> Result<(), Stop> {
    let e = match client.encryption().bootstrap_cross_signing(None).await {
        Ok(()) => return Ok(()),
        Err(e) => e,
    };
    let Some(uiaa) = e.as_uiaa_response() else {
        return Err(stop_for(&e));
    };
    let mut pw = uiaa::Password::new(
        uiaa::UserIdentifier::Matrix(uiaa::MatrixUserIdentifier::new(ctx.cfg.user_id.clone())),
        ctx.cfg.password.to_string(),
    );
    pw.session = uiaa.session.clone();
    client
        .encryption()
        .bootstrap_cross_signing(Some(uiaa::AuthData::Password(pw)))
        .await
        .map_err(|e| Stop::Retry(format!("couldn't set up cross-signing: {e}")))
}

/// Syncs, sets up the rooms and handlers, then syncs until it fails.
async fn serve_client(
    ctx: &Arc<Ctx>,
    client: Client,
    client_tx: &watch::Sender<Option<Client>>,
) -> Stop {
    let settings = SyncSettings::default().timeout(Duration::from_secs(30));
    // The first sync ever: what's there already is history.
    if !ctx.kept().synced {
        if let Err(e) = client.sync_once(settings.clone()).await {
            return stop_for(&e);
        }
        ctx.keep(|k| k.synced = true);
    }
    let rooms = match set_up_rooms(ctx, &client).await {
        Ok(r) => r,
        Err(s) => return s,
    };
    let handlers = add_handlers(ctx, &client);
    let _ = client_tx.send(Some(client.clone()));
    ctx.host.received(FromChannel::Ready { convs: rooms });
    ctx.host.status(Status::Connected);
    chummy_log::system::info("matrix", "Connected to Matrix");

    let mut wait = Duration::from_secs(1);
    let stop = loop {
        match client.sync_once(settings.clone()).await {
            Ok(_) => wait = Duration::from_secs(1),
            Err(e) => match stop_for(&e) {
                Stop::LoggedOut => break Stop::LoggedOut,
                Stop::Retry(why) => {
                    ctx.host.status(Status::Problem(why.clone()));
                    chummy_log::system::warn(
                        "matrix",
                        format!(
                            "Syncing failed ({why}); trying again in {}s",
                            wait.as_secs()
                        ),
                    );
                    tokio::time::sleep(wait).await;
                    wait = (wait * 2).min(Duration::from_secs(120));
                }
            },
        }
    };
    for h in handlers {
        client.remove_event_handler(h);
    }
    stop
}

/// One room per agent and allowed person: kept ones checked (the person
/// re-invited if they left), missing ones created. Rooms no longer in the
/// settings are left, and invites refused.
async fn set_up_rooms(ctx: &Ctx, client: &Client) -> Result<Vec<Conv>, Stop> {
    let kept = ctx.kept().rooms;
    let mut now: Vec<KeptRoom> = vec![];
    for (agent, users) in &ctx.cfg.users {
        if ctx.host.agent_name(agent).is_none() {
            continue;
        }
        let name = format!("{} ({})", ctx.agent_name(agent), ctx.cfg.label);
        for user in users {
            let Ok(uid) = UserId::parse(user.as_str()) else {
                continue;
            };
            let old = kept
                .iter()
                .find(|r| &r.agent == agent && &r.user == user)
                .and_then(|r| RoomId::parse(r.room.as_str()).ok())
                .and_then(|id| client.get_room(&id))
                .filter(|r| r.state() == RoomState::Joined);
            let room = match old {
                Some(r) => {
                    check_room(&r, &uid, &name).await;
                    r
                }
                None => match create_room(client, &uid, &name, agent).await {
                    Ok(r) => r,
                    Err(e) => {
                        if matches!(e.client_api_error_kind(), Some(ErrorKind::UnknownToken(_))) {
                            return Err(Stop::LoggedOut);
                        }
                        chummy_log::system::warn(
                            "matrix",
                            format!("Couldn't create the room for {user} and {agent}: {e}"),
                        );
                        continue;
                    }
                },
            };
            now.push(KeptRoom {
                agent: agent.clone(),
                user: user.clone(),
                room: room.room_id().to_string(),
            });
        }
    }
    // Rooms no one may use any more.
    for r in kept
        .iter()
        .filter(|r| !now.iter().any(|n| n.room == r.room))
    {
        if let Some(room) = RoomId::parse(r.room.as_str())
            .ok()
            .and_then(|id| client.get_room(&id))
            && room.state() == RoomState::Joined
        {
            chummy_log::system::info("matrix", "Left a room no one may use any more");
            let _ = room.leave().await;
        }
    }
    // Invites that came while the program was off.
    for room in client.invited_rooms() {
        let _ = room.leave().await;
    }
    ctx.keep(|k| k.rooms = now.clone());
    *lock(&ctx.rooms) = now
        .iter()
        .filter_map(|r| {
            Some((
                RoomId::parse(r.room.as_str()).ok()?,
                (r.agent.clone(), r.user.clone()),
            ))
        })
        .collect();
    Ok(now
        .iter()
        .map(|r| ctx.conv(&r.agent, &r.user, &r.room))
        .collect())
}

async fn check_room(room: &Room, user: &UserId, name: &str) {
    let member = room.get_member(user).await.ok().flatten();
    let there = member.is_some_and(|m| {
        matches!(
            m.membership(),
            MembershipState::Join | MembershipState::Invite
        )
    });
    if !there {
        match room.invite_user_by_id(user).await {
            Ok(()) => chummy_log::system::info("matrix", format!("Invited {user} back")),
            Err(e) => {
                chummy_log::system::warn("matrix", format!("Couldn't invite {user} back: {e}"))
            }
        }
    }
    if room.name().as_deref() != Some(name) {
        let _ = room.set_name(name.to_string()).await;
    }
}

async fn create_room(
    client: &Client,
    user: &UserId,
    name: &str,
    agent: &str,
) -> matrix_sdk::Result<Room> {
    // Only the bot (the creator, with the most power in every room
    // version) can invite, change the room or remove people.
    let power = serde_json::json!({
        "users_default": 0,
        "events_default": 0,
        "state_default": 100,
        "invite": 100,
        "kick": 100,
        "ban": 100,
        "redact": 50,
    });
    let mut req = CreateRoomRequest::new();
    req.invite = vec![user.to_owned()];
    req.is_direct = true;
    req.preset = Some(RoomPreset::PrivateChat);
    req.name = Some(name.to_string());
    req.topic = Some("Send a message to talk to the agent, or /help.".into());
    req.initial_state = vec![
        InitialStateEvent::with_empty_state_key(
            RoomEncryptionEventContent::with_recommended_defaults(),
        )
        .to_raw_any(),
    ];
    req.power_level_content_override = serde_json::value::to_raw_value(&power)
        .ok()
        .map(Raw::from_json);
    let room = client.create_room(req).await?;
    chummy_log::system::info("matrix", format!("Created a room for {user} and {agent}"));
    Ok(room)
}

fn add_handlers(
    ctx: &Arc<Ctx>,
    client: &Client,
) -> Vec<matrix_sdk::event_handler::EventHandlerHandle> {
    let mut hs = vec![];

    // Invites to the bot are refused: it makes its own rooms.
    hs.push(client.add_event_handler(
        |ev: StrippedRoomMemberEvent, room: Room, client: Client| async move {
            if Some(ev.state_key.as_ref()) == client.user_id()
                && ev.content.membership == MembershipState::Invite
            {
                chummy_log::system::info("matrix", "Refused an invite to a room");
                let _ = room.leave().await;
            }
        },
    ));

    let c = ctx.clone();
    hs.push(client.add_event_handler(
        move |ev: ToDeviceKeyVerificationRequestEvent, client: Client| {
            let c = c.clone();
            async move {
                verify::requested(c, client, &ev.sender, ev.content.transaction_id.as_str()).await;
            }
        },
    ));

    let c = ctx.clone();
    hs.push(client.add_event_handler(
        move |ev: OriginalSyncRoomMessageEvent,
              room: Room,
              client: Client,
              info: Option<EncryptionInfo>| {
            let c = c.clone();
            async move { message(c, client, room, ev, info).await }
        },
    ));
    hs
}

async fn message(
    ctx: Arc<Ctx>,
    client: Client,
    room: Room,
    ev: OriginalSyncRoomMessageEvent,
    info: Option<EncryptionInfo>,
) {
    if Some(ev.sender.as_ref()) == client.user_id() {
        return;
    }
    let Some((agent, user)) = lock(&ctx.rooms).get(room.room_id()).cloned() else {
        return;
    };
    let sender: OwnedUserId = ev.sender.clone();
    if sender.as_str() != user {
        return;
    }
    if let MessageType::VerificationRequest(_) = &ev.content.msgtype {
        verify::requested(ctx, client, &sender, ev.event_id.as_str()).await;
        return;
    }
    let id = room.room_id();
    if room.active_members_count() > 2 {
        chummy_log::system::warn(
            "matrix",
            format!("Someone else is in {user}'s room with {agent}: ignoring it until they leave"),
        );
        ctx.tell_once(
            id,
            "crowded",
            "Someone else is in this room, so it's ignored until they leave.",
        );
        return;
    }
    // Encrypted, from a verified session; anything else isn't trusted.
    let verified = verified(&client, &sender, info.as_ref()).await;
    if !verified {
        chummy_log::system::info(
            "matrix",
            format!(
                "Ignored a message from a session that isn't verified ({})",
                match &info {
                    Some(i) => format!("{:?}", i.verification_state),
                    None => "not encrypted".into(),
                }
            ),
        );
        ctx.tell_once(
            id,
            "verify",
            &format!("Verify this session to talk to {}.", ctx.cfg.name),
        );
        return;
    }
    if matches!(ev.content.relates_to, Some(Relation::Replacement(_))) {
        return;
    }
    let sent = u64::from(ev.origin_server_ts.0);
    let now = jiff::Timestamp::now().as_millisecond().max(0) as u64;
    if now.saturating_sub(sent) > STALE.as_millis() as u64 {
        ctx.notice(
            id.as_str(),
            format!(
                "That message came while {} was off, so it wasn't passed on. Send it again if you still want it.",
                ctx.cfg.name
            ),
        );
        return;
    }
    match &ev.content.msgtype {
        MessageType::Text(t) => {
            let reply_to = match &ev.content.relates_to {
                Some(Relation::Reply(r)) => Some(r.in_reply_to.event_id.clone()),
                _ => None,
            };
            let answers = reply_to.and_then(|e| lock(&ctx.questions).get(&e).cloned());
            let text = if answers.is_some() {
                strip_reply_fallback(&t.body)
            } else {
                t.body.clone()
            };
            ctx.host.received(FromChannel::Message {
                conv: ctx.conv(&agent, &user, id.as_str()),
                text,
                answers,
            });
        }
        m @ (MessageType::File(_)
        | MessageType::Image(_)
        | MessageType::Video(_)
        | MessageType::Audio(_)) => {
            files::save(&ctx, &client, id.as_str(), &agent, m).await;
        }
        _ => {}
    }
}

/// Whether a message came from a session that's verified now. A session
/// started before the person verified the bot stays marked as from an
/// unverified identity (it's known to be from their cross-signed device,
/// just not yet trusted): then the device is checked as it stands.
async fn verified(client: &Client, sender: &UserId, info: Option<&EncryptionInfo>) -> bool {
    let Some(info) = info else {
        return false;
    };
    match &info.verification_state {
        VerificationState::Verified => true,
        VerificationState::Unverified(VerificationLevel::UnverifiedIdentity) => {
            let Some(device) = &info.sender_device else {
                return false;
            };
            client
                .encryption()
                .get_device(sender, device)
                .await
                .ok()
                .flatten()
                .is_some_and(|d| d.is_verified_with_cross_signing())
        }
        _ => false,
    }
}

/// Drops the quoted message older apps put before a reply.
fn strip_reply_fallback(body: &str) -> String {
    if !body.starts_with("> ") {
        return body.to_string();
    }
    let rest: Vec<&str> = body.lines().skip_while(|l| l.starts_with('>')).collect();
    rest.join("\n").trim_start().to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reply_quotes_are_dropped() {
        assert_eq!(strip_reply_fallback("yes"), "yes");
        assert_eq!(
            strip_reply_fallback("> <@bot:hs> Which one?\n> more\n\nThe blue one"),
            "The blue one"
        );
        assert_eq!(strip_reply_fallback(">no space"), ">no space");
    }

    #[test]
    fn the_store_passphrase_comes_from_the_key() {
        let a = store_passphrase(&[1; 32]);
        assert_eq!(a.len(), 64);
        assert_eq!(*a, *store_passphrase(&[1; 32]));
        assert_ne!(*a, *store_passphrase(&[2; 32]));
    }
}
