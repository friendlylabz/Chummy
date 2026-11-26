//! Everything chummyd sends to Matrix, in order. A run's response is one
//! message edited as it grows (at most every [`EDIT_EVERY`]), split into
//! several when it's long.

use std::collections::HashMap;
use std::sync::Arc;
use std::time::{Duration, Instant};

use matrix_sdk::Client;
use matrix_sdk::ruma::events::room::message::{ReplacementMetadata, RoomMessageEventContent};
use matrix_sdk::ruma::{OwnedEventId, RoomId};
use tokio::sync::mpsc::UnboundedReceiver;
use tokio::sync::watch;

use super::Ctx;
use crate::link::ToMatrix;

/// How often a run's message is edited at most.
pub const EDIT_EVERY: Duration = Duration::from_secs(2);

/// The longest message part, in bytes. Matrix events are at most 64 KiB,
/// and an edit carries the text twice, as Markdown and as HTML.
pub const MAX_PART: usize = 8000;

struct RunMsg {
    room: String,
    events: Vec<OwnedEventId>,
    sent: Vec<String>,
    want: String,
    done: bool,
    last: Option<Instant>,
}

pub async fn run(
    ctx: Arc<Ctx>,
    mut rx: UnboundedReceiver<ToMatrix>,
    mut client: watch::Receiver<Option<Client>>,
) {
    let mut runs: HashMap<String, RunMsg> = HashMap::new();
    let mut tick = tokio::time::interval(Duration::from_millis(500));
    tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
    loop {
        // Nothing goes out until there's a client.
        let current = client.borrow().clone();
        let c = match current {
            Some(c) => c,
            None => {
                if client.changed().await.is_err() {
                    return;
                }
                continue;
            }
        };
        tokio::select! {
            m = rx.recv() => match m {
                Some(m) => handle(&ctx, &c, &mut runs, m).await,
                None => return,
            },
            _ = tick.tick() => flush_due(&c, &mut runs, false).await,
        }
    }
}

async fn handle(ctx: &Ctx, c: &Client, runs: &mut HashMap<String, RunMsg>, m: ToMatrix) {
    match m {
        ToMatrix::Notice { room, text } => {
            send_parts(c, &room, &text, true).await;
        }
        ToMatrix::Text { room, text } => {
            send_parts(c, &room, &text, false).await;
        }
        ToMatrix::Run {
            run_id,
            room,
            text,
            done,
        } => {
            let r = runs.entry(run_id).or_insert_with(|| RunMsg {
                room: room.clone(),
                events: vec![],
                sent: vec![],
                want: String::new(),
                done: false,
                last: None,
            });
            let first = r.last.is_none();
            r.want = text;
            r.done = done;
            if first {
                typing(c, &room, true).await;
            }
            // The first text goes out at once, the last too.
            if first || done {
                flush(c, r).await;
            }
            if done {
                typing(c, &room, false).await;
            }
            runs.retain(|_, r| !(r.done && r.sent_all()));
        }
        ToMatrix::Question { room, ask_id, text } => {
            // What the run said before asking goes first.
            for r in runs.values_mut().filter(|r| r.room == room) {
                flush(c, r).await;
            }
            if let Some(ev) = send_parts(c, &room, &text, true).await {
                ctx.remember_question(ev, ask_id);
            }
        }
        ToMatrix::Closed { ask_id } => ctx.forget_question(&ask_id),
        ToMatrix::Verify { n, matches } => super::verify::answer(ctx, n, matches).await,
        ToMatrix::Flush(done) => {
            flush_due(c, runs, true).await;
            let _ = done.send(());
        }
    }
}

impl RunMsg {
    fn parts(&self) -> Vec<String> {
        chummy_channel::split(&self.want, MAX_PART)
    }

    fn sent_all(&self) -> bool {
        self.sent == self.parts()
    }
}

async fn flush_due(c: &Client, runs: &mut HashMap<String, RunMsg>, all: bool) {
    for r in runs.values_mut() {
        let due = r.last.is_none_or(|t| t.elapsed() >= EDIT_EVERY);
        if (all || due) && !r.sent_all() {
            flush(c, r).await;
        }
    }
    runs.retain(|_, r| !(r.done && r.sent_all()));
}

/// Sends what changed in a run's message: edits to parts already sent,
/// new parts as new messages.
async fn flush(c: &Client, r: &mut RunMsg) {
    r.last = Some(Instant::now());
    let Some(room) = room(c, &r.room) else {
        // The room is gone: nothing more can be sent there.
        r.sent = r.parts();
        return;
    };
    for (i, part) in r.parts().into_iter().enumerate() {
        if part.trim().is_empty() {
            continue;
        }
        if r.sent.get(i) == Some(&part) {
            continue;
        }
        let content = RoomMessageEventContent::text_markdown(&part);
        if let Some(ev) = r.events.get(i) {
            let edit = content.make_replacement(ReplacementMetadata::new(ev.clone(), None));
            if let Err(e) = room.send(edit).await {
                chummy_log::system::warn("matrix", format!("Couldn't edit a message: {e}"));
                return;
            }
            r.sent[i] = part;
        } else {
            match room.send(content).await {
                Ok(resp) => {
                    r.events.push(resp.response.event_id);
                    r.sent.push(part);
                }
                Err(e) => {
                    chummy_log::system::warn("matrix", format!("Couldn't send a message: {e}"));
                    return;
                }
            }
        }
    }
}

fn room(c: &Client, id: &str) -> Option<matrix_sdk::Room> {
    RoomId::parse(id).ok().and_then(|id| c.get_room(&id))
}

async fn typing(c: &Client, id: &str, on: bool) {
    if let Some(room) = room(c, id) {
        let _ = room.typing_notice(on).await;
    }
}

/// Sends a message (split when long); returns the last part's event.
async fn send_parts(c: &Client, id: &str, text: &str, notice: bool) -> Option<OwnedEventId> {
    let room = room(c, id)?;
    let mut last = None;
    for part in chummy_channel::split(text, MAX_PART) {
        let content = if notice {
            RoomMessageEventContent::notice_markdown(&part)
        } else {
            RoomMessageEventContent::text_markdown(&part)
        };
        match room.send(content).await {
            Ok(resp) => last = Some(resp.response.event_id),
            Err(e) => {
                chummy_log::system::warn("matrix", format!("Couldn't send a message: {e}"));
                return None;
            }
        }
    }
    last
}
