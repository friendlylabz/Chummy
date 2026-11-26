//! Questions and tool approvals from runs, answered in chat (or in the
//! editor's run panel, for test runs).
//!
//! Answers only come from these commands, which only Chummy's own windows
//! may call; nothing a loop or a model does can answer for the user.

use chummy_flow::{Answer, AskKind};
use chummy_scheduler::{AskEvent, Pending};
use tauri::{AppHandle, Emitter, Manager, State};

use super::CmdResult;
use crate::events;
use crate::notify;
use crate::platform::AppPlatform;
use crate::state::AppState;

#[tauri::command]
pub fn list_asks(state: State<'_, AppState>) -> Vec<Pending> {
    state.session.asks.pending()
}

#[tauri::command]
pub fn answer_ask(app: AppHandle, id: String, answer: Answer) -> CmdResult<()> {
    chummy_session::answer(&AppPlatform::new(&app), &id, answer)
}

/// Forwards question events to every window, the tray and notifications.
pub fn start_forwarding(app: &AppHandle) {
    let app = app.clone();
    let mut rx = app.state::<AppState>().session.asks.subscribe();
    tauri::async_runtime::spawn(async move {
        loop {
            let event = match rx.recv().await {
                Ok(e) => Some(e),
                Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => None,
                Err(tokio::sync::broadcast::error::RecvError::Closed) => return,
            };
            let state = app.state::<AppState>();
            let _ = app.emit(events::ASKS_CHANGED, state.session.asks.pending());
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
