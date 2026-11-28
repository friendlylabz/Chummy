//! Everything sent to Discord, in order.
//!
//! - A run's answer is shown as it's written (when streaming is on): a
//!   message edited as it grows, no more often than [`EDIT_EVERY`] (Discord
//!   limits edits), split into more messages when it gets long. With
//!   streaming off, the answer goes out when it's done (or when the run
//!   asks something).
//! - While a run goes, the bot shows it's typing (Discord forgets that
//!   after about 10 seconds, so it's kept up), except while a question
//!   waits.
//! - Questions are messages of their own: approvals with Allow and Deny
//!   buttons, other questions answered by replying to them.
//! - Text is Discord's own Markdown, sent as it is, never pinging anyone.

use std::collections::HashMap;
use std::sync::Arc;
use std::time::{Duration, Instant};

use serde_json::{Value, json};
use tokio::sync::mpsc::UnboundedReceiver;

use super::api::ApiError;
use super::{Ctx, Question, lock};
use crate::link::ToChannel;

/// How often a run's answer is updated at most.
pub const EDIT_EVERY: Duration = Duration::from_millis(1500);

/// The longest message part, in bytes (Discord's limit is 2000
/// characters).
pub const MAX_PART: usize = 1900;

/// How often the typing indicator is sent again while a run goes.
const TYPING_EVERY: Duration = Duration::from_secs(8);

struct RunMsg {
    /// Who it's for (a user id).
    user: String,
    /// The messages sent for its parts so far ("" for one Discord
    /// refused), and what each says.
    messages: Vec<String>,
    sent: Vec<String>,
    want: String,
    done: bool,
    last: Option<Instant>,
    typing: bool,
    typed: Option<Instant>,
}

impl RunMsg {
    fn new(user: String) -> Self {
        Self {
            user,
            messages: vec![],
            sent: vec![],
            want: String::new(),
            done: false,
            last: None,
            typing: false,
            typed: None,
        }
    }

    fn parts(&self) -> Vec<String> {
        crate::split(&self.want, MAX_PART)
            .into_iter()
            .filter(|p| !p.trim().is_empty())
            .collect()
    }

    fn sent_all(&self) -> bool {
        self.sent == self.parts()
    }
}

pub async fn run(ctx: Arc<Ctx>, mut rx: UnboundedReceiver<ToChannel>) {
    let mut runs: HashMap<String, RunMsg> = HashMap::new();
    let mut tick = tokio::time::interval(Duration::from_millis(500));
    tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
    loop {
        tokio::select! {
            m = rx.recv() => match m {
                Some(m) => handle(&ctx, &mut runs, m).await,
                None => return,
            },
            _ = tick.tick() => {
                flush_due(&ctx, &mut runs, false).await;
                for r in runs.values_mut().filter(|r| r.typing) {
                    if r.typed.is_none_or(|t| t.elapsed() >= TYPING_EVERY) {
                        typing(&ctx, r).await;
                    }
                }
            }
        }
    }
}

async fn handle(ctx: &Arc<Ctx>, runs: &mut HashMap<String, RunMsg>, m: ToChannel) {
    match m {
        ToChannel::Notice { place, text } | ToChannel::Text { place, text } => {
            send_text(ctx, &place, &text, None).await;
        }
        ToChannel::Started { run_id, place } => {
            let r = runs.entry(run_id).or_insert_with(|| RunMsg::new(place));
            r.typing = true;
            typing(ctx, r).await;
        }
        ToChannel::Run {
            run_id,
            place,
            text,
            done,
        } => {
            let r = runs.entry(run_id).or_insert_with(|| RunMsg::new(place));
            let first = r.last.is_none();
            r.want = text;
            r.done = done;
            r.typing = !done;
            if first || done {
                flush(ctx, r, done).await;
            }
            runs.retain(|_, r| !(r.done && r.sent_all()));
        }
        ToChannel::Question {
            place,
            ask_id,
            from,
            text,
            question,
        } => {
            // What the run said before asking goes first; then it waits for
            // the person, so it isn't typing.
            for r in runs.values_mut().filter(|r| r.user == place) {
                flush(ctx, r, true).await;
                r.typing = false;
            }
            ask(ctx, &place, ask_id, &from, &text, question).await;
        }
        ToChannel::Closed { ask_id } => {
            let gone: Vec<Question> = {
                let mut qs = lock(&ctx.questions);
                let keys: Vec<u32> = qs
                    .iter()
                    .filter(|(_, q)| q.ask_id == ask_id)
                    .map(|(k, _)| *k)
                    .collect();
                keys.iter().filter_map(|k| qs.remove(k)).collect()
            };
            for q in gone.iter().filter(|q| q.buttons) {
                remove_buttons(ctx, &q.dm, &q.message_id).await;
            }
        }
        ToChannel::Answer { n, yes } => super::answer_pairing(ctx, n, yes).await,
        ToChannel::Invite(answer) => {
            let _ = answer.send(Ok(ctx.new_code()));
        }
        ToChannel::Remove { id } => super::remove_person(ctx, &id).await,
        ToChannel::Flush(done) => {
            flush_due(ctx, runs, true).await;
            let _ = done.send(());
        }
    }
}

async fn flush_due(ctx: &Ctx, runs: &mut HashMap<String, RunMsg>, all: bool) {
    for r in runs.values_mut() {
        let due = r.last.is_none_or(|t| t.elapsed() >= EDIT_EVERY);
        if all || (due && !r.sent_all()) {
            flush(ctx, r, all || r.done).await;
        }
    }
    runs.retain(|_, r| !(r.done && r.sent_all()));
}

/// Shows what changed in a run's answer. `whole`: it's done (or asks
/// something), so everything goes out even when not streaming.
async fn flush(ctx: &Ctx, r: &mut RunMsg, whole: bool) {
    r.last = Some(Instant::now());
    if !whole && !ctx.cfg.stream {
        return;
    }
    let Some(dm) = ctx.dm(&r.user).await else {
        return;
    };
    for (i, part) in r.parts().iter().enumerate() {
        if r.sent.get(i) == Some(part) {
            continue;
        }
        match r.messages.get(i) {
            // A part Discord refused: not tried again.
            Some(id) if id.is_empty() => r.sent[i] = part.clone(),
            Some(id) => match edit(ctx, &dm, id, part).await {
                Ok(()) => r.sent[i] = part.clone(),
                Err(e) => {
                    chummy_log::system::warn("discord", format!("Couldn't edit a message: {e}"));
                    return;
                }
            },
            None => match send_one(ctx, &dm, part, None).await {
                Ok(id) => {
                    r.messages.push(id);
                    r.sent.push(part.clone());
                }
                Err(e) => {
                    chummy_log::system::warn("discord", format!("Couldn't send a message: {e}"));
                    if !matches!(e, ApiError::Net(_)) && e.status() != Some(429) {
                        // It never will be: don't try again and again.
                        r.messages.push(String::new());
                        r.sent.push(part.clone());
                    }
                    return;
                }
            },
        }
    }
}

async fn typing(ctx: &Ctx, r: &mut RunMsg) {
    r.typed = Some(Instant::now());
    if let Some(dm) = ctx.dm(&r.user).await {
        let _ = ctx
            .api
            .post(&format!("/channels/{dm}/typing"), &json!({}))
            .await;
    }
}

async fn send_one(
    ctx: &Ctx,
    dm: &str,
    text: &str,
    components: Option<Value>,
) -> Result<String, ApiError> {
    let mut body = json!({ "content": text, "allowed_mentions": { "parse": [] } });
    if let Some(c) = components {
        body["components"] = c;
    }
    let v = ctx
        .api
        .post(&format!("/channels/{dm}/messages"), &body)
        .await?;
    v["id"]
        .as_str()
        .map(str::to_string)
        .ok_or_else(|| ApiError::Net("an answer that isn't a message".into()))
}

async fn edit(ctx: &Ctx, dm: &str, id: &str, text: &str) -> Result<(), ApiError> {
    ctx.api
        .patch(
            &format!("/channels/{dm}/messages/{id}"),
            &json!({ "content": text, "allowed_mentions": { "parse": [] } }),
        )
        .await
        .map(|_| ())
}

/// Sends a message to someone (split when long); returns the last part's
/// id and their direct-message channel.
pub async fn send_text(
    ctx: &Ctx,
    user: &str,
    text: &str,
    components: Option<Value>,
) -> Option<(String, String)> {
    let Some(dm) = ctx.dm(user).await else {
        chummy_log::system::warn("discord", "Couldn't open a direct message with someone");
        return None;
    };
    let parts = crate::split(text, MAX_PART);
    let n = parts.len();
    let mut last = None;
    let mut components = components;
    for (i, part) in parts.into_iter().enumerate() {
        // Buttons go on the last part.
        let c = if i + 1 == n { components.take() } else { None };
        match send_one(ctx, &dm, &part, c).await {
            Ok(id) => last = Some(id),
            Err(e) => {
                chummy_log::system::warn("discord", format!("Couldn't send a message: {e}"));
                return None;
            }
        }
    }
    last.map(|id| (id, dm))
}

/// A question (or approval) as its own message.
async fn ask(ctx: &Ctx, user: &str, ask_id: String, from: &str, text: &str, question: bool) {
    let key = crate::next_number();
    let (how, components) = if question {
        ("Reply to this message to answer.", None)
    } else {
        (
            "Or send /deny with a reason.",
            Some(json!([{ "type": 1, "components": [
                { "type": 2, "style": 3, "label": "Allow", "custom_id": format!("a:{key}:y") },
                { "type": 2, "style": 4, "label": "Deny", "custom_id": format!("a:{key}:n") },
            ] }])),
        )
    };
    let md = format!("**{}:** {text}\n\n{how}", from.replace('*', ""));
    if let Some((id, dm)) = send_text(ctx, user, &md, components).await {
        lock(&ctx.questions).insert(
            key,
            Question {
                ask_id,
                user: user.to_string(),
                dm,
                message_id: id,
                buttons: !question,
            },
        );
    }
}

/// Takes the buttons off a message (its question doesn't wait any more).
pub async fn remove_buttons(ctx: &Ctx, dm: &str, id: &str) {
    let _ = ctx
        .api
        .patch(
            &format!("/channels/{dm}/messages/{id}"),
            &json!({ "components": [] }),
        )
        .await;
}
