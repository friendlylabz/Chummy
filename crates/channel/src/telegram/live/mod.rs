//! Running a Telegram channel ([`super::Telegram`]): one bot (from
//! @BotFather) per agent, talked to in private chats. The same in both
//! programs; what differs is behind [`ChannelHost`].
//!
//! - **Telegram isn't end-to-end encrypted**: its servers can read these
//!   chats. The programs say so wherever a Telegram channel is set up.
//! - The bot's token is its whole login. It's only in the host's secrets
//!   and in the addresses of the calls (never logged: see [`api`]).
//! - **People pair**: whoever runs the program makes a code
//!   ([`crate::Handle::invite`], good for [`CODE_LIFE`]); the person sends
//!   `/start <code>` to the bot; the program is asked about them
//!   ([`crate::Event::Verify`]) and they're let in only if it says yes.
//!   Anyone else gets no answer at all. The people let in are kept (with
//!   [`ChannelHost::keep`]) and can be removed again.
//! - Private chats only: when someone adds the bot to a group, it leaves.
//! - It asks Telegram for messages (long polling): nothing listens for
//!   connections. If the bot has a webhook, or another program reads its
//!   messages, it says so and keeps trying.
//! - Messages sent while the program was off for more than [`STALE`] get a
//!   note instead of a run.

mod api;
mod files;
pub mod format;
mod outbox;

pub use api::ApiError;

use std::collections::{BTreeMap, HashMap};
use std::sync::atomic::AtomicBool;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use serde::{Deserialize, Serialize};
use serde_json::json;
use tokio::sync::mpsc::UnboundedSender;
use zeroize::Zeroizing;

use api::{Api, CallbackQuery, Me, Message, Update, User, WebhookInfo};

use super::MAX_UPLOAD_MB;
use crate::kind::Start;
use crate::link::{FromChannel, ToChannel};
use crate::{ChannelHost, Conv, Event, Handle, Invite, Person, STALE, Status, Verify, now_ms};

/// How long a pairing code works.
pub const CODE_LIFE: Duration = Duration::from_secs(10 * 60);

/// Wrong codes sent before the code stops working.
const MAX_WRONG: u32 = 10;

/// One Telegram channel: the bot and the agent it talks for.
#[derive(Clone)]
pub struct TelegramSettings {
    /// The channel's id (its conversations carry it).
    pub channel: String,
    /// The Bot API server (checked already: https, or http only on this
    /// computer or the local network). [`super::DEFAULT_API`] unless someone runs
    /// their own.
    pub api_url: String,
    /// The bot's token, from @BotFather.
    pub token: Zeroizing<String>,
    /// The agent people talk to.
    pub agent: String,
    /// Largest file accepted, in MB (at most [`MAX_UPLOAD_MB`]).
    pub upload_mb: u32,
    /// Show answers as they're written.
    pub stream: bool,
    /// Keeps each person's chat apart from other channels' (see
    /// [`crate::channel_chat`]).
    pub chat_scope: String,
    /// The program, as people are told ("Chummy", "chummyd").
    pub name: String,
}

/// What's kept between runs.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
struct Kept {
    /// The bot the rest belongs to.
    #[serde(default)]
    bot_id: i64,
    /// The next update to ask for (`None`: from the first waiting).
    #[serde(default)]
    offset: Option<i64>,
    /// Who was let in.
    #[serde(default)]
    people: Vec<Person>,
}

struct PairCode {
    code: String,
    expires: u64,
    wrong: u32,
}

struct Pairing {
    user: i64,
    name: String,
}

pub(crate) struct Question {
    ask_id: String,
    chat: i64,
    message_id: i64,
    /// An approval, with buttons.
    buttons: bool,
}

/// What the Telegram side shares between its tasks.
pub struct Ctx {
    host: Arc<dyn ChannelHost>,
    cfg: TelegramSettings,
    api: Api,
    /// To the outbox (notices from here go out in order too).
    out: UnboundedSender<ToChannel>,
    kept: Mutex<Kept>,
    bot: Mutex<Option<Me>>,
    code: Mutex<Option<PairCode>>,
    pairings: Mutex<BTreeMap<u32, Pairing>>,
    /// Questions waiting, by the key their buttons carry.
    questions: Mutex<HashMap<u32, Question>>,
    /// Drafts work (they're tried until they don't).
    drafts: AtomicBool,
    told_groups: AtomicBool,
}

pub(crate) fn lock<T>(m: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|e| e.into_inner())
}

impl Ctx {
    fn notice(&self, chat: i64, text: String) {
        let _ = self.out.send(ToChannel::Notice {
            place: chat.to_string(),
            text,
        });
    }

    fn note(&self, text: impl Into<String>) {
        self.host.event(Event::Note { text: text.into() });
    }

    fn agent_name(&self) -> String {
        self.host
            .agent_name(&self.cfg.agent)
            .unwrap_or_else(|| self.cfg.agent.clone())
    }

    /// Changes what's kept, and keeps it now.
    fn keep(&self, f: impl FnOnce(&mut Kept)) {
        let k = {
            let mut k = lock(&self.kept);
            f(&mut k);
            k.clone()
        };
        if let Ok(v) = serde_json::to_value(&k) {
            self.host.keep(v);
        }
    }

    fn person(&self, user: i64) -> Option<Person> {
        let id = user.to_string();
        lock(&self.kept).people.iter().find(|p| p.id == id).cloned()
    }

    fn conv(&self, p: &Person) -> Conv {
        Conv {
            channel: self.cfg.channel.clone(),
            place: p.id.clone(),
            agent: self.cfg.agent.clone(),
            chat: crate::channel_chat(&self.cfg.chat_scope, &p.id),
            who: p.name.clone(),
            person: p.id.clone(),
            console: false,
        }
    }

    /// Tells the program who can talk now.
    fn announce(&self) {
        let people = lock(&self.kept).people.clone();
        self.host.received(FromChannel::Ready {
            convs: people.iter().map(|p| self.conv(p)).collect(),
        });
        self.host.event(Event::People { people });
    }

    fn new_code(&self) -> Invite {
        let mut b = [0u8; 4];
        let n = if getrandom::fill(&mut b).is_ok() {
            u32::from_le_bytes(b)
        } else {
            (now_ms() as u32) ^ std::process::id()
        };
        let code = format!("{:06}", n % 1_000_000);
        let expires = now_ms() + CODE_LIFE.as_millis() as u64;
        *lock(&self.code) = Some(PairCode {
            code: code.clone(),
            expires,
            wrong: 0,
        });
        chummy_log::system::info("telegram", "Made a pairing code");
        let bot = lock(&self.bot)
            .as_ref()
            .and_then(|b| b.username.clone())
            .map(|u| format!("@{u}"))
            .unwrap_or_else(|| "the bot".into());
        Invite {
            text: format!(
                "Ask them to send /start {code} to {bot} in Telegram within {} minutes. You'll be asked before \
                 they're let in. The code works once.",
                CODE_LIFE.as_secs() / 60
            ),
            code,
            expires_at: expires,
        }
    }
}

/// Starts a Telegram channel as a kind of channel ([`super::Telegram`]):
/// from its options (`api_url`, `upload_mb`, `stream`) and secret
/// (`token`).
pub fn start_kind(host: Arc<dyn ChannelHost>, s: Start) -> Result<Handle, String> {
    let setup = s.setup("");
    let cfg = TelegramSettings {
        channel: s.channel.clone(),
        api_url: setup.text("api_url").to_string(),
        token: Zeroizing::new(setup.secret("token").trim().to_string()),
        agent: s
            .agents
            .first()
            .cloned()
            .ok_or("A Telegram channel talks for one agent.")?,
        upload_mb: setup.number("upload_mb"),
        stream: setup.toggle("stream"),
        chat_scope: s.channel.clone(),
        name: s.program.clone(),
    };
    start(host, cfg)
}

/// Starts a Telegram channel. Must be called inside a Tokio runtime.
pub fn start(host: Arc<dyn ChannelHost>, cfg: TelegramSettings) -> Result<Handle, String> {
    let api = Api::new(&cfg.api_url, cfg.token.clone())?;
    let kept: Kept = host
        .kept()
        .and_then(|v| serde_json::from_value(v).ok())
        .unwrap_or_default();
    let (tx, rx) = tokio::sync::mpsc::unbounded_channel();
    let ctx = Arc::new(Ctx {
        host,
        cfg,
        api,
        out: tx.clone(),
        kept: Mutex::new(kept),
        bot: Mutex::new(None),
        code: Mutex::new(None),
        pairings: Mutex::new(BTreeMap::new()),
        questions: Mutex::new(HashMap::new()),
        drafts: AtomicBool::new(true),
        told_groups: AtomicBool::new(false),
    });
    let tasks = vec![
        tokio::spawn(outbox::run(ctx.clone(), rx)),
        tokio::spawn(serve(ctx)),
    ];
    Ok(Handle { tx, tasks })
}

/// What went wrong, worded for whoever runs the program.
fn problem(e: &ApiError) -> String {
    if e.unauthorized() {
        "Telegram doesn't accept the bot's token: check it in @BotFather (/token), and paste it again."
            .into()
    } else if e.conflict() {
        "Another program is reading this bot's messages (a chummyd, or Chummy on another computer): \
         give each its own bot."
            .into()
    } else {
        e.to_string()
    }
}

async fn serve(ctx: Arc<Ctx>) {
    let mut wait = Duration::from_secs(2);
    loop {
        ctx.host.status(Status::Connecting);
        let why = match connect(&ctx).await {
            Ok(()) => {
                wait = Duration::from_secs(2);
                poll(&ctx).await
            }
            Err(why) => why,
        };
        ctx.host.status(Status::Problem(why.clone()));
        chummy_log::system::warn(
            "telegram",
            format!(
                "Telegram isn't reachable ({why}); trying again in {}s",
                wait.as_secs()
            ),
        );
        tokio::time::sleep(wait).await;
        wait = (wait * 2).min(Duration::from_secs(300));
    }
}

/// Checks the bot (and that its messages can come here), and tells
/// Telegram its commands.
async fn connect(ctx: &Ctx) -> Result<(), String> {
    let quick = Duration::from_secs(30);
    let me: Me = ctx
        .api
        .call("getMe", &json!({}), quick)
        .await
        .map_err(|e| problem(&e))?;
    if lock(&ctx.kept).bot_id != me.id {
        // Another bot: the people let in, and where it was, belong to the
        // old one.
        ctx.keep(|k| {
            *k = Kept {
                bot_id: me.id,
                ..Default::default()
            }
        });
    }
    let hook: WebhookInfo = ctx
        .api
        .call("getWebhookInfo", &json!({}), quick)
        .await
        .map_err(|e| problem(&e))?;
    if !hook.url.is_empty() {
        return Err(
            "This bot sends its messages to a webhook, so they can't come here: remove the webhook \
             (deleteWebhook), or use another bot."
                .into(),
        );
    }
    let commands: Vec<serde_json::Value> = [
        ("new", "Start a fresh conversation"),
        ("status", "What's going on"),
        ("stop", "Stop the run going on"),
        ("pause", "Pause it"),
        ("resume", "Carry on the paused run"),
        ("discard", "Throw away the paused run"),
        ("jobs", "This agent's jobs"),
        ("allow", "Allow the tool call waiting for you"),
        ("deny", "Refuse it (with a reason)"),
        ("help", "What you can send"),
    ]
    .iter()
    .map(|(c, d)| json!({ "command": c, "description": d }))
    .collect();
    if let Err(e) = ctx
        .api
        .call::<bool>("setMyCommands", &json!({ "commands": commands }), quick)
        .await
    {
        chummy_log::system::warn("telegram", format!("Couldn't set the bot's commands: {e}"));
    }
    if let Some(u) = &me.username {
        ctx.host.event(Event::Account {
            name: format!("@{u}"),
        });
    }
    if me.can_join_groups == Some(true) {
        ctx.note(
            "This bot can be added to groups. It leaves any it's added to, but turning that off in \
             @BotFather (Bot Settings → Allow Groups) is better.",
        );
    }
    // (Messages waiting from before need no skipping: nobody is let in
    // yet the first time, and a pairing code only works once it's made.)
    *lock(&ctx.bot) = Some(me);
    ctx.announce();
    ctx.host.status(Status::Connected);
    chummy_log::system::info("telegram", "Connected to Telegram");
    Ok(())
}

/// Asks for messages until something goes wrong that needs connecting
/// again; returns why.
async fn poll(ctx: &Arc<Ctx>) -> String {
    let mut wait = Duration::from_secs(1);
    let mut troubled = false;
    loop {
        let offset = lock(&ctx.kept).offset.unwrap_or(0);
        let r: Result<Vec<Update>, ApiError> = ctx
            .api
            .call(
                "getUpdates",
                &json!({
                    "offset": offset,
                    "timeout": 25,
                    "allowed_updates": ["message", "callback_query"],
                }),
                Duration::from_secs(40),
            )
            .await;
        match r {
            Ok(updates) => {
                wait = Duration::from_secs(1);
                if troubled {
                    troubled = false;
                    ctx.host.status(Status::Connected);
                }
                let Some(last) = updates.last().map(|u| u.update_id) else {
                    continue;
                };
                for u in updates {
                    update(ctx, u).await;
                }
                ctx.keep(|k| k.offset = Some(last + 1));
            }
            Err(e) if e.unauthorized() => return problem(&e),
            Err(e) => {
                let why = problem(&e);
                troubled = true;
                ctx.host.status(Status::Problem(why.clone()));
                if let ApiError::Api {
                    retry_after: Some(s),
                    ..
                } = e
                {
                    wait = Duration::from_secs(s.min(300));
                }
                chummy_log::system::warn(
                    "telegram",
                    format!(
                        "Couldn't get messages ({why}); trying again in {}s",
                        wait.as_secs()
                    ),
                );
                tokio::time::sleep(wait).await;
                wait = (wait * 2).min(Duration::from_secs(120));
            }
        }
    }
}

async fn update(ctx: &Arc<Ctx>, u: Update) {
    if let Some(m) = u.message {
        message(ctx, m).await;
    }
    if let Some(q) = u.callback_query {
        button(ctx, q).await;
    }
}

/// `/cmd@thisbot …` as `/cmd …`.
fn without_bot_name(text: &str, bot: Option<&str>) -> String {
    let t = text.trim();
    let Some(bot) = bot else {
        return t.to_string();
    };
    let (first, rest) = t.split_once(char::is_whitespace).unwrap_or((t, ""));
    match first.strip_suffix(&format!("@{bot}")) {
        Some(cmd) if cmd.starts_with('/') => format!("{cmd} {rest}").trim().to_string(),
        _ => t.to_string(),
    }
}

async fn message(ctx: &Arc<Ctx>, m: Message) {
    if m.chat.kind != "private" {
        if matches!(m.chat.kind.as_str(), "group" | "supergroup") {
            let _ = ctx
                .api
                .send("leaveChat", &json!({ "chat_id": m.chat.id }))
                .await;
            chummy_log::system::info("telegram", "Left a group someone added the bot to");
            if !ctx
                .told_groups
                .swap(true, std::sync::atomic::Ordering::Relaxed)
            {
                ctx.note(
                    "Someone added the bot to a group, so it left: it only talks in private chats.",
                );
            }
        }
        return;
    }
    let Some(from) = m.from.clone() else {
        return;
    };
    if from.is_bot {
        return;
    }
    let username = lock(&ctx.bot).as_ref().and_then(|b| b.username.clone());
    let text = m
        .text
        .as_deref()
        .map(|t| without_bot_name(t, username.as_deref()));
    let Some(person) = ctx.person(from.id) else {
        if let Some(t) = &text {
            stranger(ctx, &from, m.chat.id, t);
        }
        return;
    };
    let age = (now_ms() / 1000).saturating_sub(m.date.max(0) as u64);
    if age > STALE.as_secs() {
        ctx.notice(
            m.chat.id,
            format!(
                "That message came while {} was off, so it wasn't passed on. Send it again if you still want it.",
                ctx.cfg.name
            ),
        );
        return;
    }
    match text {
        Some(t) => {
            if t == "/start" || t.starts_with("/start ") {
                ctx.notice(
                    m.chat.id,
                    format!(
                        "You can talk to {} here. Send a message, or /help.",
                        ctx.agent_name()
                    ),
                );
                return;
            }
            let answers = m.reply_to_message.as_ref().and_then(|r| {
                lock(&ctx.questions)
                    .values()
                    .find(|q| q.chat == m.chat.id && q.message_id == r.message_id)
                    .map(|q| q.ask_id.clone())
            });
            ctx.host.received(FromChannel::Message {
                conv: ctx.conv(&person),
                text: t,
                answers,
            });
        }
        None if files::has_file(&m) => {
            tokio::spawn(files::save(ctx.clone(), m));
        }
        None => {}
    }
}

/// Someone not let in: only a right pairing code gets an answer.
fn stranger(ctx: &Ctx, from: &User, chat: i64, text: &str) {
    let Some(code) = text.strip_prefix("/start") else {
        return;
    };
    let code = code.trim();
    if code.is_empty() || !code.chars().all(|c| c.is_ascii_digit()) {
        return;
    }
    {
        let mut c = lock(&ctx.code);
        let Some(pc) = c.as_mut() else {
            return;
        };
        if pc.expires < now_ms() {
            *c = None;
            return;
        }
        if pc.code != code {
            pc.wrong += 1;
            if pc.wrong >= MAX_WRONG {
                *c = None;
                drop(c);
                ctx.note("Too many wrong pairing codes were sent to the bot, so the code stopped working: make a new one.");
            }
            return;
        }
        // Each code lets one person ask.
        *c = None;
    }
    let name = from.name();
    let n = crate::next_number();
    lock(&ctx.pairings).insert(
        n,
        Pairing {
            user: from.id,
            name: name.clone(),
        },
    );
    chummy_log::system::info(
        "telegram",
        format!("Pairing {n}: someone sent the code and waits to be let in"),
    );
    let agent = ctx.agent_name();
    ctx.host.event(Event::Verify(Verify {
        n,
        who: format!("{name}, Telegram id {}", from.id),
        ask: format!(
            "sent the pairing code and wants to talk to {agent} in Telegram. Let them in only if it's the person \
             you gave it to."
        ),
        items: vec![],
        yes: "Let them in".into(),
        no: "Refuse".into(),
        confirm: Some(format!(
            "{name} will be able to talk to {agent} in Telegram: each message they send starts a run of its loop, \
             in its folder, with the tools it has. Telegram can read what they say.\n\nOnly let in someone you \
             gave the code to."
        )),
    }));
    ctx.notice(
        chat,
        format!(
            "Thanks. Now wait for whoever runs {} to let you in.",
            ctx.cfg.name
        ),
    );
}

/// The answer to a pairing: let them in or not.
pub(crate) async fn answer_pairing(ctx: &Ctx, n: Option<u32>, yes: bool) {
    let p = {
        let mut ps = lock(&ctx.pairings);
        let n = match n {
            Some(n) => Some(n),
            None if ps.len() == 1 => ps.keys().next().copied(),
            None => None,
        };
        n.and_then(|n| ps.remove(&n).map(|p| (n, p)))
    };
    let Some((n, p)) = p else {
        ctx.note(match n {
            Some(n) => format!("There's no pairing {n} waiting."),
            None => "Say which pairing (several, or none, are waiting).".into(),
        });
        return;
    };
    ctx.host.event(Event::Over { n });
    if !yes {
        chummy_log::system::info("telegram", format!("Pairing {n} refused"));
        ctx.notice(p.user, "You weren't let in.".into());
        return;
    }
    let id = p.user.to_string();
    ctx.keep(|k| {
        k.people.retain(|x| x.id != id);
        k.people.push(Person {
            id: id.clone(),
            name: p.name.clone(),
            since: now_ms(),
        });
    });
    chummy_log::system::warn(
        "telegram",
        format!("Let someone in to talk to {} (pairing {n})", ctx.cfg.agent),
    );
    ctx.note(format!("{} can talk to {} now.", p.name, ctx.agent_name()));
    ctx.announce();
    ctx.notice(
        p.user,
        format!(
            "You can talk to {} now. Send a message, or /help.",
            ctx.agent_name()
        ),
    );
}

/// Stops letting someone in.
pub(crate) async fn remove_person(ctx: &Ctx, id: &str) {
    let Some(p) = id.parse().ok().and_then(|u| ctx.person(u)) else {
        return;
    };
    ctx.keep(|k| k.people.retain(|x| x.id != id));
    chummy_log::system::warn(
        "telegram",
        format!("Removed someone from talking to {}", ctx.cfg.agent),
    );
    ctx.note(format!(
        "{} can't talk to {} any more.",
        p.name,
        ctx.agent_name()
    ));
    ctx.announce();
    if let Ok(chat) = p.id.parse() {
        ctx.notice(
            chat,
            format!("You can't talk to {} here any more.", ctx.agent_name()),
        );
    }
}

/// A button pressed: Allow or Deny on an approval.
async fn button(ctx: &Arc<Ctx>, q: CallbackQuery) {
    let answer = |text: &str| json!({ "callback_query_id": q.id, "text": text });
    let Some(person) = ctx.person(q.from.id) else {
        let _ = ctx
            .api
            .send("answerCallbackQuery", &json!({ "callback_query_id": q.id }))
            .await;
        return;
    };
    let parsed = q.data.as_deref().and_then(|d| {
        let mut it = d.split(':');
        match (it.next(), it.next(), it.next()) {
            (Some("a"), Some(k), Some(y)) => Some((k.parse::<u32>().ok()?, y == "y")),
            _ => None,
        }
    });
    let found = parsed.and_then(|(k, yes)| {
        lock(&ctx.questions)
            .get(&k)
            .filter(|w| w.chat == q.from.id)
            .map(|w| (w.ask_id.clone(), w.message_id, yes))
    });
    match found {
        Some((ask_id, message_id, yes)) => {
            let _ = ctx
                .api
                .send(
                    "answerCallbackQuery",
                    &answer(if yes { "Allowed" } else { "Denied" }),
                )
                .await;
            outbox::remove_buttons(ctx, q.from.id, message_id).await;
            ctx.host.received(FromChannel::Message {
                conv: ctx.conv(&person),
                text: if yes { "/allow" } else { "/deny" }.into(),
                answers: Some(ask_id),
            });
        }
        None => {
            let _ = ctx
                .api
                .send(
                    "answerCallbackQuery",
                    &answer("That doesn't wait any more."),
                )
                .await;
            if let Some(m) = &q.message {
                outbox::remove_buttons(ctx, m.chat.id, m.message_id).await;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn commands_lose_the_bot_name() {
        assert_eq!(
            without_bot_name("/status@helper_bot", Some("helper_bot")),
            "/status"
        );
        assert_eq!(
            without_bot_name("/deny@helper_bot too risky", Some("helper_bot")),
            "/deny too risky"
        );
        assert_eq!(
            without_bot_name("hi @helper_bot", Some("helper_bot")),
            "hi @helper_bot"
        );
        assert_eq!(
            without_bot_name("/status@other", Some("helper_bot")),
            "/status@other"
        );
    }
}
