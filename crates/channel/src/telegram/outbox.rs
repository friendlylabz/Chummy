//! Everything sent to Telegram, in order.
//!
//! - A run's answer is shown as it's written (when streaming is on): the
//!   part being written as a draft (sendMessageDraft, a preview that only
//!   private chats have), the parts before it as messages. Where drafts
//!   don't work, it falls back to one message edited as it grows; with
//!   streaming off, the answer goes out when it's done (or when the run
//!   asks something).
//! - While a run goes, the bot shows it's typing (Telegram forgets that
//!   after 5 seconds, so it's kept up), except while a question waits.
//! - Questions are messages of their own: approvals with Allow and Deny
//!   buttons, other questions asking for a reply.

use std::collections::HashMap;
use std::sync::Arc;
use std::sync::atomic::Ordering;
use std::time::{Duration, Instant};

use serde_json::{Value, json};
use tokio::sync::mpsc::UnboundedReceiver;

use super::api::{ApiError, Sent};
use super::format::html;
use super::{Ctx, Question, lock};
use crate::link::ToChannel;

/// How often a run's answer is updated at most.
pub const EDIT_EVERY: Duration = Duration::from_millis(1500);

/// The longest message part, in bytes (Telegram's limit is 4096
/// characters).
pub const MAX_PART: usize = 4000;

/// How often the typing indicator and a draft are sent again while
/// nothing changes (Telegram drops them after 5 and about 30 seconds).
const TYPING_EVERY: Duration = Duration::from_secs(4);
const DRAFT_EVERY: Duration = Duration::from_secs(20);

struct RunMsg {
    chat: i64,
    /// The messages sent for its parts so far, and what each says.
    messages: Vec<i64>,
    sent: Vec<String>,
    want: String,
    done: bool,
    last: Option<Instant>,
    typing: bool,
    typed: Option<Instant>,
    /// The draft shown for the part being written.
    draft_id: i64,
    draft: Option<(String, Instant)>,
}

impl RunMsg {
    fn new(chat: i64) -> Self {
        Self {
            chat,
            messages: vec![],
            sent: vec![],
            want: String::new(),
            done: false,
            last: None,
            typing: false,
            typed: None,
            draft_id: i64::from(crate::next_number()),
            draft: None,
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

fn chat(place: &str) -> Option<i64> {
    place.parse().ok()
}

async fn handle(ctx: &Arc<Ctx>, runs: &mut HashMap<String, RunMsg>, m: ToChannel) {
    match m {
        ToChannel::Notice { place, text } | ToChannel::Text { place, text } => {
            if let Some(c) = chat(&place) {
                send_text(ctx, c, &text, None).await;
            }
        }
        ToChannel::Started { run_id, place } => {
            let Some(c) = chat(&place) else { return };
            let r = runs.entry(run_id).or_insert_with(|| RunMsg::new(c));
            r.typing = true;
            typing(ctx, r).await;
        }
        ToChannel::Run {
            run_id,
            place,
            text,
            done,
        } => {
            let Some(c) = chat(&place) else { return };
            let r = runs.entry(run_id).or_insert_with(|| RunMsg::new(c));
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
            let Some(c) = chat(&place) else { return };
            // What the run said before asking goes first, as messages; then
            // it waits for the person, so it isn't typing.
            for r in runs.values_mut().filter(|r| r.chat == c) {
                flush(ctx, r, true).await;
                r.typing = false;
            }
            ask(ctx, c, ask_id, &from, &text, question).await;
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
                remove_buttons(ctx, q.chat, q.message_id).await;
            }
        }
        ToChannel::Answer { n, yes } => super::answer_pairing(ctx, n, yes).await,
        ToChannel::NewCode(answer) => {
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
        let keep_draft = r
            .draft
            .as_ref()
            .is_some_and(|(_, at)| at.elapsed() >= DRAFT_EVERY);
        if all || (due && !r.sent_all()) || (keep_draft && !r.done) {
            flush(ctx, r, all || r.done).await;
        }
    }
    runs.retain(|_, r| !(r.done && r.sent_all()));
}

/// Shows what changed in a run's answer. `whole`: everything as messages
/// (it's done, or asks something); otherwise, while streaming, the part
/// being written may be a draft.
async fn flush(ctx: &Ctx, r: &mut RunMsg, whole: bool) {
    r.last = Some(Instant::now());
    let stream = ctx.cfg.stream;
    if !whole && !stream {
        return;
    }
    let parts = r.parts();
    let drafts = stream && ctx.drafts.load(Ordering::Relaxed);
    for (i, part) in parts.iter().enumerate() {
        let writing = i + 1 == parts.len() && !whole;
        // The part being written, not yet a message: a draft.
        if writing && drafts && r.messages.get(i).is_none() {
            let stale = r
                .draft
                .as_ref()
                .is_none_or(|(t, at)| t != part || at.elapsed() >= DRAFT_EVERY);
            if stale {
                match draft(ctx, r.chat, r.draft_id, part).await {
                    Ok(()) => r.draft = Some((part.clone(), Instant::now())),
                    Err(e) if e.code() == Some(429) => return,
                    Err(e) => {
                        // No drafts here: one message, edited as it grows.
                        chummy_log::system::info(
                            "telegram",
                            format!("Drafts don't work ({e}): editing messages instead"),
                        );
                        ctx.drafts.store(false, Ordering::Relaxed);
                        Box::pin(flush(ctx, r, whole)).await;
                        return;
                    }
                }
            }
            continue;
        }
        if r.sent.get(i) == Some(part) {
            continue;
        }
        match r.messages.get(i) {
            // A part Telegram refused: not tried again.
            Some(0) => r.sent[i] = part.clone(),
            Some(&id) => match edit(ctx, r.chat, id, part).await {
                Ok(()) => r.sent[i] = part.clone(),
                Err(e) if e.not_modified() => r.sent[i] = part.clone(),
                Err(e) => {
                    chummy_log::system::warn("telegram", format!("Couldn't edit a message: {e}"));
                    return;
                }
            },
            None => match send_one(ctx, r.chat, part, None).await {
                Ok(id) => {
                    r.messages.push(id);
                    r.sent.push(part.clone());
                }
                Err(e) => {
                    chummy_log::system::warn("telegram", format!("Couldn't send a message: {e}"));
                    if !matches!(e, ApiError::Net(_)) && e.code() != Some(429) {
                        // It never will be: don't try again and again.
                        r.messages.push(0);
                        r.sent.push(part.clone());
                    }
                    return;
                }
            },
        }
    }
    if whole {
        r.draft = None;
    }
}

async fn typing(ctx: &Ctx, r: &mut RunMsg) {
    r.typed = Some(Instant::now());
    let _ = ctx
        .api
        .send(
            "sendChatAction",
            &json!({ "chat_id": r.chat, "action": "typing" }),
        )
        .await;
}

/// Sends HTML, and the plain text if Telegram can't read the HTML.
async fn with_fallback(
    ctx: &Ctx,
    method: &str,
    mut body: Value,
    md: &str,
) -> Result<Value, ApiError> {
    body["text"] = Value::String(html(md));
    body["parse_mode"] = Value::String("HTML".into());
    match ctx.api.send(method, &body).await {
        Err(e) if e.bad_markup() => {
            body["text"] = Value::String(md.to_string());
            if let Some(o) = body.as_object_mut() {
                o.remove("parse_mode");
            }
            ctx.api.send(method, &body).await
        }
        Err(ApiError::Api {
            code: 429,
            retry_after,
            ..
        }) => {
            tokio::time::sleep(Duration::from_secs(retry_after.unwrap_or(3).min(30))).await;
            ctx.api.send(method, &body).await
        }
        r => r,
    }
}

async fn send_one(ctx: &Ctx, chat: i64, md: &str, markup: Option<Value>) -> Result<i64, ApiError> {
    let mut body = json!({ "chat_id": chat, "link_preview_options": { "is_disabled": true } });
    if let Some(m) = markup {
        body["reply_markup"] = m;
    }
    let v = with_fallback(ctx, "sendMessage", body, md).await?;
    serde_json::from_value::<Sent>(v)
        .map(|s| s.message_id)
        .map_err(|_| ApiError::Net("an answer that isn't a message".into()))
}

async fn edit(ctx: &Ctx, chat: i64, id: i64, md: &str) -> Result<(), ApiError> {
    let body = json!({ "chat_id": chat, "message_id": id, "link_preview_options": { "is_disabled": true } });
    with_fallback(ctx, "editMessageText", body, md)
        .await
        .map(|_| ())
}

async fn draft(ctx: &Ctx, chat: i64, id: i64, md: &str) -> Result<(), ApiError> {
    let body = json!({ "chat_id": chat, "draft_id": id });
    with_fallback(ctx, "sendMessageDraft", body, md)
        .await
        .map(|_| ())
}

/// Sends a message (split when long); returns the last part's id.
pub async fn send_text(ctx: &Ctx, chat: i64, md: &str, markup: Option<Value>) -> Option<i64> {
    let parts = crate::split(md, MAX_PART);
    let n = parts.len();
    let mut last = None;
    let mut markup = markup;
    for (i, part) in parts.into_iter().enumerate() {
        // Buttons go on the last part.
        let m = if i + 1 == n { markup.take() } else { None };
        match send_one(ctx, chat, &part, m).await {
            Ok(id) => last = Some(id),
            Err(e) => {
                chummy_log::system::warn("telegram", format!("Couldn't send a message: {e}"));
                return None;
            }
        }
    }
    last
}

/// A question (or approval) as its own message.
async fn ask(ctx: &Ctx, chat: i64, ask_id: String, from: &str, text: &str, question: bool) {
    let key = crate::next_number();
    let (how, markup) = if question {
        (
            "Reply to this message to answer.",
            json!({ "force_reply": true, "input_field_placeholder": "Your answer" }),
        )
    } else {
        (
            "Or send /deny with a reason.",
            json!({ "inline_keyboard": [[
                { "text": "Allow", "callback_data": format!("a:{key}:y") },
                { "text": "Deny", "callback_data": format!("a:{key}:n") },
            ]] }),
        )
    };
    // The agent's name is escaped Markdown-wise by being bold text.
    let md = format!("**{}:** {text}\n\n{how}", from.replace('*', ""));
    if let Some(id) = send_text(ctx, chat, &md, Some(markup)).await {
        lock(&ctx.questions).insert(
            key,
            Question {
                ask_id,
                chat,
                message_id: id,
                buttons: !question,
            },
        );
    }
}

/// Takes the buttons off a message (its question doesn't wait any more).
pub async fn remove_buttons(ctx: &Ctx, chat: i64, id: i64) {
    let _ = ctx
        .api
        .send(
            "editMessageReplyMarkup",
            &json!({ "chat_id": chat, "message_id": id, "reply_markup": { "inline_keyboard": [] } }),
        )
        .await;
}
