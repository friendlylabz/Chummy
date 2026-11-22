//! Questions and tool approvals from runs, answered in chat (or in the
//! editor's run panel, for test runs).
//!
//! Answers only come from these commands, which only Chummy's own windows
//! may call; nothing a loop or a model does can answer for the user.

use chummy_flow::{Answer, AskKind};
use chummy_scheduler::{AskEvent, Pending, RunKind};
use tauri::{AppHandle, Emitter, Manager, State};

use super::chat::{ChatMessage, append, msg_id};
use super::{CmdResult, err, now};
use crate::events;
use crate::notify;
use crate::state::AppState;

#[tauri::command]
pub fn list_asks(state: State<'_, AppState>) -> Vec<Pending> {
    state.asks.pending()
}

/// What an answer looks like in the chat.
fn answer_text(a: &Answer) -> String {
    match a {
        Answer::Text(t) => t.clone(),
        Answer::Allow => "Allowed.".into(),
        Answer::Deny(why) if why.trim().is_empty() => "Not allowed.".into(),
        Answer::Deny(why) => format!("Not allowed: {}", why.trim()),
    }
}

#[tauri::command]
pub fn answer_ask(
    app: AppHandle,
    state: State<'_, AppState>,
    id: String,
    answer: Answer,
) -> CmdResult<()> {
    let pending = state
        .asks
        .pending()
        .into_iter()
        .find(|p| p.id == id)
        .ok_or("That question was already answered, or its run stopped.")?;
    state.asks.answer(&id, answer.clone()).map_err(|e| {
        let e = err(e);
        let mut c = e.chars();
        c.next()
            .map(|f| f.to_uppercase().chain(c).collect())
            .unwrap_or(e)
    })?;
    // Keep a record in the agent's chat. Test runs aren't part of a chat.
    if pending.origin.kind != RunKind::Test
        && let Some(agent) = &pending.origin.agent_id
    {
        let from = (pending.origin.kind == RunKind::Job).then(|| pending.origin.title.clone());
        let question = pending.ask.text.clone();
        for (role, text) in [("question", question), ("answer", answer_text(&answer))] {
            let _ = append(
                &app,
                &state,
                agent,
                ChatMessage {
                    id: msg_id(),
                    role: role.into(),
                    text,
                    at: now(),
                    from: from.clone(),
                    run: None,
                },
            );
        }
    }
    Ok(())
}

/// Forwards question events to every window, the tray and notifications.
pub fn start_forwarding(app: &AppHandle) {
    let app = app.clone();
    let mut rx = app.state::<AppState>().asks.subscribe();
    tauri::async_runtime::spawn(async move {
        loop {
            let event = match rx.recv().await {
                Ok(e) => Some(e),
                Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => None,
                Err(tokio::sync::broadcast::error::RecvError::Closed) => return,
            };
            let state = app.state::<AppState>();
            let _ = app.emit(events::ASKS_CHANGED, state.asks.pending());
            crate::tray::refresh(&app);
            match &event {
                Some(AskEvent::Asked { pending }) => {
                    let what = match &pending.ask.kind {
                        AskKind::Question => {
                            format!("{} asked you a question", pending.origin.title)
                        }
                        AskKind::ToolApproval { tool, .. } => {
                            format!("{} asked to run the tool {tool}", pending.origin.title)
                        }
                        AskKind::Budget { model, .. } => {
                            format!(
                                "{} asked for more daily tokens for {model}",
                                pending.origin.title
                            )
                        }
                    };
                    chummy_log::system::log(
                        chummy_log::system::Level::Info,
                        "questions",
                        what,
                        &[("run", &pending.origin.run_id)],
                    );
                }
                Some(AskEvent::Closed { answered, .. }) => chummy_log::system::info(
                    "questions",
                    if *answered {
                        "A question was answered"
                    } else {
                        "A question was withdrawn (its run stopped or it timed out)"
                    },
                ),
                None => {}
            }
            if let Some(AskEvent::Asked { pending }) = event
                && !notify::any_window_focused(&app)
            {
                // Only who's asking: the question itself may be private, and
                // notifications show on lock screens.
                let what = match &pending.ask.kind {
                    AskKind::Question => "has a question for you",
                    AskKind::ToolApproval { .. } => "wants to run a tool",
                    AskKind::Budget { .. } => "needs more of its daily tokens",
                };
                notify::show(
                    &app,
                    &format!("{} {what}", pending.origin.title),
                    "Open Chummy to answer.",
                );
            }
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn answers_read_well() {
        assert_eq!(answer_text(&Answer::Allow), "Allowed.");
        assert_eq!(answer_text(&Answer::Deny(" ".into())), "Not allowed.");
        assert_eq!(answer_text(&Answer::Deny("no".into())), "Not allowed: no");
        assert_eq!(answer_text(&Answer::Text("x".into())), "x");
    }
}
