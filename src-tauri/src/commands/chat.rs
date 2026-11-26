//! Chats and loop runs: the app's commands over `chummy-session`, which
//! holds how conversations behave (shared with chummyd).
//!
//! Sending a message runs the agent's loop with the agent's model, inside the
//! agent's workspace. Progress streams to every window as `run:*` events; the
//! reply is saved to the chat it was sent from. Each chat runs one message at
//! a time; an agent's chats can run at once.

use std::collections::HashMap;

use chummy_flow::LoopFile;
use chummy_session::{self as session, chats};
use serde::Serialize;
use tauri::{AppHandle, Emitter, State, WebviewWindow};

use super::CmdResult;
use crate::events;
use crate::platform::AppPlatform;
use crate::state::{AppState, RunInfo, lock};

pub use chummy_session::{ChatIndex, ChatMessage, ChatMessageDeleted, ChatMeta, PausedInfo};

#[derive(Clone, Serialize)]
pub struct ChatMessageRestored {
    pub agent_id: String,
    pub chat_id: String,
    pub message: ChatMessage,
    pub position: usize,
}

/// Messages deleted while the app runs, newest last, so Undo can put them
/// back. Kept here (not sent to the window and back) so Undo can only
/// restore what was really deleted. Gone when the app quits.
struct Deleted {
    agent: String,
    chat: String,
    message: ChatMessage,
    position: usize,
}
static DELETED: std::sync::Mutex<Vec<Deleted>> = std::sync::Mutex::new(Vec::new());
const DELETED_KEPT: usize = 50;

pub fn emit_index(app: &AppHandle, agent_id: &str, index: ChatIndex) {
    session::emit_index(&AppPlatform::new(app), agent_id, index);
}

/// An agent's chats (at least one) and which is open.
#[tauri::command]
pub fn list_chats(state: State<'_, AppState>, agent_id: String) -> CmdResult<ChatIndex> {
    chats::index(&state.chats.root, &agent_id)
}

/// Starts a new chat with an agent and opens it.
#[tauri::command]
pub fn new_chat(
    app: AppHandle,
    state: State<'_, AppState>,
    agent_id: String,
) -> CmdResult<ChatMeta> {
    if state.settings().agent(&agent_id).is_none() {
        return Err("That agent no longer exists.".into());
    }
    let (chat, index) = chats::create(&state.chats.root, &agent_id)?;
    emit_index(&app, &agent_id, index);
    Ok(chat)
}

/// Opens one of an agent's chats.
#[tauri::command]
pub fn set_active_chat(
    app: AppHandle,
    state: State<'_, AppState>,
    agent_id: String,
    chat_id: String,
) -> CmdResult<()> {
    let index = chats::set_active(&state.chats.root, &agent_id, &chat_id)?;
    emit_index(&app, &agent_id, index);
    Ok(())
}

/// Deletes a chat that isn't working on a message, after asking if it has
/// any messages. Returns whether it was deleted.
#[tauri::command]
pub async fn delete_chat(
    app: AppHandle,
    window: WebviewWindow,
    state: State<'_, AppState>,
    agent_id: String,
    chat_id: String,
) -> CmdResult<bool> {
    if state.session.chat_busy(&chat_id) {
        return Err("That chat is still working. Stop it first.".into());
    }
    let index = chats::index(&state.chats.root, &agent_id)?;
    let Some(chat) = index.chats.iter().find(|c| c.id == chat_id) else {
        return Ok(false);
    };
    if chat.messages > 0
        && !crate::dialogs::confirm(
            &app,
            window.label(),
            "Delete this chat?",
            &format!(
                "“{}” and its {} messages will be deleted. Run logs are kept.",
                chat.title, chat.messages
            ),
            "Delete chat",
            true,
        )
        .await
    {
        return Ok(false);
    }
    session::paused::discard(&AppPlatform::new(&app), &agent_id, &chat_id)?;
    let index = chats::delete(&state.chats.root, &agent_id, &chat_id)?;
    emit_index(&app, &agent_id, index);
    Ok(true)
}

/// Deletes one message from a chat at the user's request (the window asks
/// first). It's gone from the chat and from what the agent's loop sees next
/// time. Not while the chat is working, since that run already has the
/// history. Returns what Undo needs to put it back.
#[tauri::command]
pub fn delete_chat_message(
    app: AppHandle,
    state: State<'_, AppState>,
    agent_id: String,
    chat_id: String,
    message_id: String,
) -> CmdResult<()> {
    if state.session.chat_busy(&chat_id) {
        return Err("Wait until this chat is done, then delete the message.".into());
    }
    let (index, message, position) =
        chats::delete_message(&state.chats.root, &agent_id, &chat_id, &message_id)?;
    {
        let mut kept = lock(&DELETED);
        kept.push(Deleted {
            agent: agent_id.clone(),
            chat: chat_id.clone(),
            message,
            position,
        });
        if kept.len() > DELETED_KEPT {
            kept.remove(0);
        }
    }
    let _ = app.emit(
        events::CHAT_MESSAGE_DELETED,
        ChatMessageDeleted {
            agent_id: agent_id.clone(),
            chat_id,
            message_id,
        },
    );
    emit_index(&app, &agent_id, index);
    Ok(())
}

/// Undo for `delete_chat_message`: puts the message back where it was.
#[tauri::command]
pub fn restore_chat_message(
    app: AppHandle,
    state: State<'_, AppState>,
    agent_id: String,
    chat_id: String,
    message_id: String,
) -> CmdResult<()> {
    if state.session.chat_busy(&chat_id) {
        return Err("Wait until this chat is done, then undo.".into());
    }
    let d = {
        let mut kept = lock(&DELETED);
        let i = kept
            .iter()
            .rposition(|d| d.agent == agent_id && d.chat == chat_id && d.message.id == message_id)
            .ok_or("That message can't be brought back any more.")?;
        kept.remove(i)
    };
    let (index, position) = match chats::restore_message(
        &state.chats.root,
        &agent_id,
        &chat_id,
        d.message.clone(),
        d.position,
    ) {
        Ok(r) => r,
        Err(e) => {
            lock(&DELETED).push(d);
            return Err(e);
        }
    };
    let _ = app.emit(
        events::CHAT_MESSAGE_RESTORED,
        ChatMessageRestored {
            agent_id: agent_id.clone(),
            chat_id,
            message: d.message,
            position,
        },
    );
    emit_index(&app, &agent_id, index);
    Ok(())
}

#[tauri::command]
pub fn get_chat(
    state: State<'_, AppState>,
    agent_id: String,
    chat_id: String,
) -> CmdResult<Vec<ChatMessage>> {
    chats::load(&state.chats.root, &agent_id, &chat_id)
}

#[tauri::command]
pub fn list_runs(state: State<'_, AppState>) -> Vec<RunInfo> {
    state.session.runs()
}

/// What a running run's Pane output blocks have shown so far, newest first
/// (empty for a run that has finished or never used one).
#[tauri::command]
pub fn run_pane(state: State<'_, AppState>, run_id: String) -> Vec<String> {
    state.session.pane(&run_id)
}

#[tauri::command]
pub fn stop_run(state: State<'_, AppState>, run_id: String) {
    state.session.stop(&run_id);
}

/// Pauses a chat run after its current block (or right away while it
/// waits on you, if it can run that block again). Its state is kept until
/// the chat resumes or discards it.
#[tauri::command]
pub fn pause_run(app: AppHandle, run_id: String) -> CmdResult<()> {
    session::pause_run(&AppPlatform::new(&app), &run_id)
}

/// Every paused chat run.
#[tauri::command]
pub fn list_paused(state: State<'_, AppState>) -> Vec<PausedInfo> {
    state.session.paused()
}

/// Carries on a chat's paused run, where it stopped. Returns its id (the
/// same run as before it paused).
#[tauri::command]
pub fn resume_run(app: AppHandle, agent_id: String, chat_id: String) -> CmdResult<String> {
    session::resume_run(&AppPlatform::new(&app), &agent_id, &chat_id)
}

/// Throws away a chat's paused run, after asking. Returns whether it did.
#[tauri::command]
pub async fn discard_paused(
    app: AppHandle,
    window: WebviewWindow,
    state: State<'_, AppState>,
    agent_id: String,
    chat_id: String,
) -> CmdResult<bool> {
    if !state.session.is_paused(&agent_id, &chat_id) {
        return Ok(false);
    }
    if !crate::dialogs::confirm(
        &app,
        window.label(),
        "Discard the paused run?",
        "It can't be resumed after this. What it already said stays in the chat.",
        "Discard",
        true,
    )
    .await
    {
        return Ok(false);
    }
    session::discard_paused(&AppPlatform::new(&app), &agent_id, &chat_id)?;
    Ok(true)
}

/// Sends a chat message to an agent and starts its loop. Returns the run id.
#[tauri::command]
pub fn send_message(
    app: AppHandle,
    agent_id: String,
    chat_id: String,
    text: String,
) -> CmdResult<String> {
    session::send_message(&AppPlatform::new(&app), &agent_id, &chat_id, &text)
}

/// Runs a loop from the editor without saving anything to chat. With an
/// agent, the run uses its model and workspace; without one, the default
/// model and no workspace (file blocks will report an error).
#[tauri::command]
pub fn test_run(
    app: AppHandle,
    file: LoopFile,
    agent_id: Option<String>,
    message: String,
    overrides: HashMap<String, String>,
) -> CmdResult<String> {
    session::test_run(
        &AppPlatform::new(&app),
        file,
        agent_id.as_deref(),
        message,
        overrides,
    )
}
