//! Running a Discord channel ([`super::Discord`]): one bot (from Discord's
//! Developer Portal) per agent, talked to in direct messages only. The same
//! in both programs; what differs is behind [`ChannelHost`].
//!
//! - **Discord isn't end-to-end encrypted**: Discord can read these
//!   messages. The programs say so wherever a Discord channel is set up.
//! - The bot's token is its whole login. It's only in the host's secrets
//!   and in the calls' headers and the Gateway's identify message (never
//!   logged: see [`api`]).
//! - **People ask with a code**: whoever runs the program makes one
//!   ([`crate::Handle::invite`], good for [`CODE_LIFE`]); the person sends it
//!   to the bot in a direct message; the program is asked about them
//!   ([`crate::Event::Verify`]) and they're let in only if it says yes.
//!   Anyone else gets no answer at all. The people let in are kept (with
//!   [`ChannelHost::keep`]) and can be removed again.
//! - **Direct messages only**: messages in servers are ignored (the bot
//!   asks only for direct messages). People need to share a server with
//!   the bot to message it; the program shows the link that adds it to one.
//! - Messages come over Discord's Gateway, a WebSocket the bot opens
//!   ([`gateway`]): nothing listens for connections. Messages sent while
//!   the program was off are fetched when it connects; those older than
//!   [`STALE`] get a note instead of a run.
//! - Commands are Discord slash commands (direct messages only), and
//!   typed ones work too (`/status`, or `!status`).

mod api;
mod files;
mod gateway;
mod outbox;

pub use api::ApiError;

use std::collections::{BTreeMap, HashMap};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use tokio::sync::mpsc::UnboundedSender;
use zeroize::Zeroizing;

use api::{Api, Application, Interaction, Message, User};

use crate::kind::Start;
use crate::link::{FromChannel, ToChannel};
use crate::{ChannelHost, Conv, Event, Handle, Invite, Person, STALE, Status, Verify, now_ms};

/// How long an invitation code works.
pub const CODE_LIFE: Duration = Duration::from_secs(10 * 60);

/// Wrong codes sent before the code stops working.
const MAX_WRONG: u32 = 10;

/// One Discord channel: the bot and the agent it talks for.
#[derive(Clone)]
pub struct DiscordSettings {
    /// The channel's id (its conversations carry it).
    pub channel: String,
    /// Discord's API (checked already). [`super::DEFAULT_API`] unless
    /// testing.
    pub api_url: String,
    /// The bot's token, from the Developer Portal.
    pub token: Zeroizing<String>,
    /// The agent people talk to.
    pub agent: String,
    /// Largest file accepted, in MB.
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
    bot_id: String,
    /// Who was let in.
    #[serde(default)]
    people: Vec<Person>,
    /// Each person's direct-message channel, by their id.
    #[serde(default)]
    dms: BTreeMap<String, String>,
    /// The last message seen in each direct-message channel (to fetch what
    /// came while the program was off).
    #[serde(default)]
    last: BTreeMap<String, String>,
}

struct Code {
    code: String,
    expires: u64,
    wrong: u32,
}

struct Pairing {
    user: String,
    name: String,
    dm: String,
}

pub(crate) struct Question {
    ask_id: String,
    user: String,
    dm: String,
    message_id: String,
    /// An approval, with buttons.
    buttons: bool,
}

/// What the Discord side shares between its tasks.
pub struct Ctx {
    host: Arc<dyn ChannelHost>,
    cfg: DiscordSettings,
    api: Api,
    /// To the outbox (notices from here go out in order too).
    out: UnboundedSender<ToChannel>,
    kept: Mutex<Kept>,
    me: Mutex<Option<User>>,
    app: Mutex<Option<Application>>,
    code: Mutex<Option<Code>>,
    pairings: Mutex<BTreeMap<u32, Pairing>>,
    /// Questions waiting, by the key their buttons carry.
    questions: Mutex<HashMap<u32, Question>>,
    /// Direct-message channels seen (anyone's), by user id.
    dms: Mutex<HashMap<String, String>>,
    commands_set: AtomicBool,
}

pub(crate) fn lock<T>(m: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|e| e.into_inner())
}

impl Ctx {
    /// A message to someone (by their user id), in order with the rest.
    fn notice(&self, user: &str, text: String) {
        let _ = self.out.send(ToChannel::Notice {
            place: user.to_string(),
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

    fn person(&self, user: &str) -> Option<Person> {
        lock(&self.kept)
            .people
            .iter()
            .find(|p| p.id == user)
            .cloned()
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

    /// Someone's direct-message channel: seen, kept, or opened now.
    pub(crate) async fn dm(&self, user: &str) -> Option<String> {
        if let Some(c) = lock(&self.dms).get(user).cloned() {
            return Some(c);
        }
        if let Some(c) = lock(&self.kept).dms.get(user).cloned() {
            return Some(c);
        }
        let c: api::Channel = self
            .api
            .post("/users/@me/channels", &json!({ "recipient_id": user }))
            .await
            .ok()
            .and_then(|v| serde_json::from_value(v).ok())?;
        lock(&self.dms).insert(user.to_string(), c.id.clone());
        Some(c.id)
    }

    fn bot_name(&self) -> String {
        lock(&self.me)
            .as_ref()
            .map(|u| format!("@{}", u.username))
            .unwrap_or_else(|| "the bot".into())
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
        *lock(&self.code) = Some(Code {
            code: code.clone(),
            expires,
            wrong: 0,
        });
        chummy_log::system::info("discord", "Made an invitation code");
        Invite {
            text: format!(
                "Ask them to send {code} to {} in a direct message on Discord within {} minutes (they need to \
                 share a server with the bot). You'll be asked before they're let in. The code works once.",
                self.bot_name(),
                CODE_LIFE.as_secs() / 60
            ),
            code,
            expires_at: expires,
        }
    }
}

/// Starts a Discord channel as a kind of channel ([`super::Discord`]): from
/// its options (`api_url`, `upload_mb`, `stream`) and secret (`token`).
pub fn start_kind(host: Arc<dyn ChannelHost>, s: Start) -> Result<Handle, String> {
    let setup = s.setup("");
    let cfg = DiscordSettings {
        channel: s.channel.clone(),
        api_url: setup.text("api_url").to_string(),
        token: Zeroizing::new(setup.secret("token").trim().to_string()),
        agent: s
            .agents
            .first()
            .cloned()
            .ok_or("A Discord channel talks for one agent.")?,
        upload_mb: setup.number("upload_mb"),
        stream: setup.toggle("stream"),
        chat_scope: s.channel.clone(),
        name: s.program.clone(),
    };
    start(host, cfg)
}

/// Starts a Discord channel. Must be called inside a Tokio runtime.
pub fn start(host: Arc<dyn ChannelHost>, cfg: DiscordSettings) -> Result<Handle, String> {
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
        me: Mutex::new(None),
        app: Mutex::new(None),
        code: Mutex::new(None),
        pairings: Mutex::new(BTreeMap::new()),
        questions: Mutex::new(HashMap::new()),
        dms: Mutex::new(HashMap::new()),
        commands_set: AtomicBool::new(false),
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
        "Discord doesn't accept the bot's token: reset it in the Developer Portal (Bot → Reset Token), and \
         paste it again."
            .into()
    } else {
        e.to_string()
    }
}

async fn serve(ctx: Arc<Ctx>) {
    let mut resume = None;
    let mut wait = Duration::from_secs(2);
    loop {
        ctx.host.status(Status::Connecting);
        let end = gateway::session(&ctx, &mut resume).await;
        if end.was_ready {
            // It worked a while: connect again soon, quietly.
            wait = Duration::from_secs(2);
            chummy_log::system::info(
                "discord",
                format!("Discord's Gateway closed ({}); connecting again", end.why),
            );
            tokio::time::sleep(Duration::from_secs(1)).await;
            continue;
        }
        ctx.host.status(Status::Problem(end.why.clone()));
        chummy_log::system::warn(
            "discord",
            format!(
                "Discord isn't reachable ({}); trying again in {}s",
                end.why,
                wait.as_secs()
            ),
        );
        tokio::time::sleep(wait).await;
        wait = (wait * 2).min(Duration::from_secs(300));
    }
}

/// The Gateway says the bot is in: checks the bot, sets its commands, and
/// fetches what came while the program was off.
async fn ready(ctx: &Arc<Ctx>, me: User, in_servers: usize) {
    if lock(&ctx.kept).bot_id != me.id {
        // Another bot: the people let in belong to the old one.
        ctx.keep(|k| {
            *k = Kept {
                bot_id: me.id.clone(),
                ..Default::default()
            }
        });
    }
    ctx.host.event(Event::Account {
        name: format!("@{}", me.username),
    });
    *lock(&ctx.me) = Some(me.clone());
    match ctx.api.get::<Application>("/oauth2/applications/@me").await {
        Ok(app) => {
            if app.bot_public == Some(true) {
                ctx.note(
                    "This bot is public: anyone could add it to their server. Strangers still get no answer, \
                     but turning off Public Bot in the Developer Portal (Bot) is better.",
                );
            }
            if !ctx.commands_set.swap(true, Ordering::Relaxed) {
                set_commands(ctx, &app.id).await;
            }
            *lock(&ctx.app) = Some(app);
        }
        Err(e) => chummy_log::system::warn(
            "discord",
            format!("Couldn't read the bot's application: {e}"),
        ),
    }
    if in_servers == 0 {
        let id = lock(&ctx.app)
            .as_ref()
            .map(|a| a.id.clone())
            .unwrap_or(me.id);
        ctx.note(format!(
            "Nobody can message the bot yet: add it to a server you're in with {}",
            super::invite_link(&id)
        ));
    }
    ctx.announce();
    ctx.host.status(Status::Connected);
    chummy_log::system::info("discord", "Connected to Discord");
    tokio::spawn(backlog(ctx.clone()));
}

/// The commands people see in the bot's direct messages.
async fn set_commands(ctx: &Ctx, app: &str) {
    let plain = |name: &str, about: &str| json!({ "name": name, "description": about, "type": 1, "contexts": [1], "integration_types": [0] });
    let with = |name: &str, about: &str, opt: &str, opt_about: &str, required: bool| {
        let mut c = plain(name, about);
        c["options"] =
            json!([{ "type": 3, "name": opt, "description": opt_about, "required": required }]);
        c
    };
    let commands = json!([
        plain("new", "Start a fresh conversation"),
        plain("status", "What's going on"),
        plain("stop", "Stop the run going on"),
        plain("pause", "Pause it"),
        plain("resume", "Carry on the paused run"),
        plain("discard", "Throw away the paused run"),
        plain("jobs", "This agent's jobs"),
        with(
            "run",
            "Run one of this agent's jobs now",
            "job",
            "The job's name",
            true
        ),
        plain("allow", "Allow the tool call waiting for you"),
        with(
            "deny",
            "Refuse it",
            "reason",
            "Why (the agent is told)",
            false
        ),
        plain("help", "What you can send"),
    ]);
    if let Err(e) = ctx
        .api
        .put(&format!("/applications/{app}/commands"), &commands)
        .await
    {
        chummy_log::system::warn("discord", format!("Couldn't set the bot's commands: {e}"));
    }
}

/// Fetches what people let in sent while the program was off.
async fn backlog(ctx: Arc<Ctx>) {
    let since: Vec<(String, String)> = {
        let k = lock(&ctx.kept);
        k.people
            .iter()
            .filter_map(|p| k.dms.get(&p.id))
            .filter_map(|dm| k.last.get(dm).map(|l| (dm.clone(), l.clone())))
            .collect()
    };
    for (dm, last) in since {
        let r: Result<Vec<Message>, _> = ctx
            .api
            .get(&format!("/channels/{dm}/messages?after={last}&limit=50"))
            .await;
        let Ok(mut ms) = r else { continue };
        ms.sort_by_key(|m| m.id.parse::<u64>().unwrap_or(0));
        for m in ms {
            message(&ctx, m).await;
        }
    }
}

/// `!status` as `/status` (Discord's own `/` opens its command list).
fn typed_command(text: &str) -> String {
    let t = text.trim();
    if let Some(rest) = t.strip_prefix('!') {
        let candidate = format!("/{rest}");
        if matches!(crate::parse(&candidate), Some(Ok(_))) {
            return candidate;
        }
    }
    t.to_string()
}

async fn message(ctx: &Arc<Ctx>, m: Message) {
    // Direct messages only.
    if m.guild_id.is_some() {
        return;
    }
    let me = lock(&ctx.me).as_ref().map(|u| u.id.clone());
    if m.author.bot == Some(true) || Some(&m.author.id) == me.as_ref() {
        return;
    }
    lock(&ctx.dms).insert(m.author.id.clone(), m.channel_id.clone());
    let text = typed_command(&m.content);
    let Some(person) = ctx.person(&m.author.id) else {
        stranger(ctx, &m.author, &m.channel_id, &text);
        return;
    };
    ctx.keep(|k| {
        k.dms.insert(person.id.clone(), m.channel_id.clone());
        k.last.insert(m.channel_id.clone(), m.id.clone());
    });
    let age = (now_ms() / 1000).saturating_sub(m.sent_at());
    if age > STALE.as_secs() {
        ctx.notice(
            &person.id,
            format!(
                "That message came while {} was off, so it wasn't passed on. Send it again if you still want it.",
                ctx.cfg.name
            ),
        );
        return;
    }
    if !text.is_empty() {
        let answers = m
            .message_reference
            .as_ref()
            .and_then(|r| r.message_id.as_deref())
            .and_then(|to| {
                lock(&ctx.questions)
                    .values()
                    .find(|q| q.user == person.id && q.message_id == to)
                    .map(|q| q.ask_id.clone())
            });
        ctx.host.received(FromChannel::Message {
            conv: ctx.conv(&person),
            text,
            answers,
        });
    }
    for a in m.attachments {
        tokio::spawn(files::save(ctx.clone(), person.id.clone(), a));
    }
}

/// Someone not let in: only a right invitation code gets an answer.
fn stranger(ctx: &Ctx, from: &User, dm: &str, text: &str) {
    let code = text.strip_prefix("/start").unwrap_or(text).trim();
    if code.len() != 6 || !code.chars().all(|c| c.is_ascii_digit()) {
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
                ctx.note("Too many wrong codes were sent to the bot, so the code stopped working: make a new one.");
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
            user: from.id.clone(),
            name: name.clone(),
            dm: dm.to_string(),
        },
    );
    chummy_log::system::info(
        "discord",
        format!("Invitation {n}: someone sent the code and waits to be let in"),
    );
    let agent = ctx.agent_name();
    ctx.host.event(Event::Verify(Verify {
        n,
        who: format!("{name}, Discord id {}", from.id),
        ask: format!(
            "sent the invitation code and wants to talk to {agent} on Discord. Let them in only if it's the person \
             you gave it to."
        ),
        items: vec![],
        yes: "Let them in".into(),
        no: "Refuse".into(),
        confirm: Some(format!(
            "{name} will be able to talk to {agent} on Discord: each message they send starts a run of its loop, \
             in its folder, with the tools it has. Discord can read what they say.\n\nOnly let in someone you \
             gave the code to."
        )),
    }));
    ctx.notice(
        &from.id,
        format!(
            "Thanks. Now wait for whoever runs {} to let you in.",
            ctx.cfg.name
        ),
    );
}

/// The answer to an invitation: let them in or not.
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
            Some(n) => format!("There's no invitation {n} waiting."),
            None => "Say which invitation (several, or none, are waiting).".into(),
        });
        return;
    };
    ctx.host.event(Event::Over { n });
    if !yes {
        chummy_log::system::info("discord", format!("Invitation {n} refused"));
        ctx.notice(&p.user, "You weren't let in.".into());
        return;
    }
    ctx.keep(|k| {
        k.people.retain(|x| x.id != p.user);
        k.people.push(Person {
            id: p.user.clone(),
            name: p.name.clone(),
            since: now_ms(),
        });
        k.dms.insert(p.user.clone(), p.dm.clone());
    });
    chummy_log::system::warn(
        "discord",
        format!(
            "Let someone in to talk to {} (invitation {n})",
            ctx.cfg.agent
        ),
    );
    ctx.note(format!("{} can talk to {} now.", p.name, ctx.agent_name()));
    ctx.announce();
    ctx.notice(
        &p.user,
        format!(
            "You can talk to {} now. Send a message, or /help.",
            ctx.agent_name()
        ),
    );
}

/// Stops letting someone in.
pub(crate) async fn remove_person(ctx: &Ctx, id: &str) {
    let Some(p) = ctx.person(id) else {
        return;
    };
    chummy_log::system::warn(
        "discord",
        format!("Removed someone from talking to {}", ctx.cfg.agent),
    );
    ctx.note(format!(
        "{} can't talk to {} any more.",
        p.name,
        ctx.agent_name()
    ));
    // Told first, while their channel is still known.
    ctx.notice(
        &p.id,
        format!("You can't talk to {} here any more.", ctx.agent_name()),
    );
    ctx.keep(|k| {
        k.people.retain(|x| x.id != id);
        if let Some(dm) = k.dms.remove(id) {
            k.last.remove(&dm);
        }
    });
    ctx.announce();
}

/// A slash command used, or a button pressed.
async fn interaction(ctx: Arc<Ctx>, i: Interaction) {
    if i.guild_id.is_some() {
        return;
    }
    let Some(user) = i.user.clone() else { return };
    // Strangers get no answer, not even here.
    let Some(person) = ctx.person(&user.id) else {
        return;
    };
    if let Some(dm) = &i.channel_id {
        lock(&ctx.dms).insert(user.id.clone(), dm.clone());
    }
    let respond = |body: Value| {
        let ctx = ctx.clone();
        let path = format!("/interactions/{}/{}/callback", i.id, i.token);
        async move {
            if let Err(e) = ctx.api.post(&path, &body).await {
                chummy_log::system::warn("discord", format!("Couldn't answer Discord: {e}"));
            }
        }
    };
    let data = i.data.clone().unwrap_or(Value::Null);
    match i.kind {
        // A slash command: passed on like a typed one.
        2 => {
            let name = data["name"].as_str().unwrap_or("help");
            let args: Vec<String> = data["options"]
                .as_array()
                .map(|os| {
                    os.iter()
                        .filter_map(|o| o["value"].as_str().map(str::to_string))
                        .collect()
                })
                .unwrap_or_default();
            let text = format!("/{name} {}", args.join(" ")).trim().to_string();
            respond(json!({ "type": 4, "data": { "content": format!("`{text}`"), "allowed_mentions": { "parse": [] } } })).await;
            ctx.host.received(FromChannel::Message {
                conv: ctx.conv(&person),
                text,
                answers: None,
            });
        }
        // A button: Allow or Deny on an approval.
        3 => {
            let parsed = data["custom_id"].as_str().and_then(|d| {
                let mut it = d.split(':');
                match (it.next(), it.next(), it.next()) {
                    (Some("a"), Some(k), Some(y)) => Some((k.parse::<u32>().ok()?, y == "y")),
                    _ => None,
                }
            });
            let found = parsed.and_then(|(k, yes)| {
                lock(&ctx.questions)
                    .get(&k)
                    .filter(|q| q.user == person.id)
                    .map(|q| (q.ask_id.clone(), yes))
            });
            let content = i
                .message
                .as_ref()
                .map(|m| m.content.clone())
                .unwrap_or_default();
            match found {
                Some((ask_id, yes)) => {
                    let said = if yes { "Allowed." } else { "Denied." };
                    respond(json!({ "type": 7, "data": {
                        "content": format!("{content}\n\n**{said}**"), "components": [] } }))
                    .await;
                    ctx.host.received(FromChannel::Message {
                        conv: ctx.conv(&person),
                        text: if yes { "/allow" } else { "/deny" }.into(),
                        answers: Some(ask_id),
                    });
                }
                None => {
                    respond(json!({ "type": 7, "data": {
                        "content": format!("{content}\n\n*That doesn't wait any more.*"), "components": [] } }))
                    .await;
                }
            }
        }
        _ => {}
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn typed_commands_can_start_with_an_exclamation_mark() {
        assert_eq!(typed_command("!status"), "/status");
        assert_eq!(typed_command("!deny too risky"), "/deny too risky");
        // Not a command: left alone.
        assert_eq!(
            typed_command("!important: read this"),
            "!important: read this"
        );
        assert_eq!(typed_command(" /stop "), "/stop");
    }

    #[test]
    fn people_are_named_plainly() {
        let u = |g: Option<&str>, n: &str| User {
            id: "42".into(),
            username: n.into(),
            global_name: g.map(str::to_string),
            bot: None,
        };
        assert_eq!(u(Some("Alice"), "alice").name(), "Alice (@alice)");
        assert_eq!(u(Some("alice"), "alice").name(), "alice");
        assert_eq!(u(None, "alice").name(), "@alice");
        assert_eq!(u(None, "").name(), "42");
    }
}
