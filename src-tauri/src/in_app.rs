//! Chummy's own small jobs for a model, outside any loop (today: chat
//! titles).
//!
//! The rule (the user's): a model is the user's, for their loops. Chummy
//! calls it for anything else only when its connection has **"Use model
//! within app"** switched on, which is off by default. Every such call goes
//! through [`Settings::in_app_model`](crate::settings::Settings::in_app_model);
//! without it, Chummy does the job without a model or not at all (titles
//! then come from the first message). "Test connection" is the user's own
//! explicit request, so it isn't one of these.

use chummy_flow::{Budget, ExMessage, Host, LlmCall, Role, ToolChoice, strip_thinking};
use chummy_log::system::{self as syslog, Level};
use tauri::{AppHandle, Manager};

use crate::host::agent_host;
use crate::state::AppState;
use chummy_session::chats;

const TITLE_SYSTEM: &str = "You name conversations. Reply with only a short title (2 to 6 words), no quotes, no full stop.";

/// A model's answer cleaned into a title: its first line, without quotes,
/// a "Title:" prefix or a final full stop. `None` if nothing usable is left.
pub fn clean_title(answer: &str) -> Option<String> {
    let line = strip_thinking(answer)
        .lines()
        .map(str::trim)
        .find(|l| !l.is_empty())?
        .to_string();
    let line = line
        .strip_prefix("Title:")
        .or_else(|| line.strip_prefix("title:"))
        .unwrap_or(&line)
        .trim()
        .trim_matches(|c: char| matches!(c, '"' | '\'' | '“' | '”' | '*' | '`'))
        .trim_end_matches('.')
        .trim()
        .to_string();
    (!line.is_empty()).then_some(line)
}

/// After a chat's first reply: names the chat with the agent's model, if
/// "Use model within app" is on for it. Otherwise (or if anything fails, or
/// the day's tokens are used up) the title from the first message stays.
pub async fn title_chat(
    app: AppHandle,
    agent_id: String,
    chat_id: String,
    message: String,
    reply: String,
) {
    let state = app.state::<AppState>();
    let s = state.settings();
    let Some(agent) = s.agent(&agent_id).cloned() else {
        return;
    };
    let Some(model) = s.in_app_model(&agent).cloned() else {
        return;
    };
    let host = agent_host(&app, &s, Some(&agent), Some(model.clone()), None, None);
    if !matches!(host.budget(None), Budget::Ok) {
        return;
    }
    let cut = |t: &str| t.chars().take(1500).collect::<String>();
    let call = LlmCall {
        block: "in-app/title".into(),
        messages: vec![
            ExMessage::new(Role::System, TITLE_SYSTEM),
            ExMessage::new(
                Role::User,
                format!(
                    "The user wrote:\n{}\n\nThe assistant replied:\n{}\n\nTitle for this conversation:",
                    cut(&message),
                    cut(&reply)
                ),
            ),
        ],
        tools: vec![],
        tool_choice: ToolChoice::Auto,
        generation: Default::default(),
        temperature: Some(0.2),
        max_tokens: Some(48),
        model: None,
    };
    let Ok(answer) = host.llm(call, &mut |_| {}).await else {
        return;
    };
    let Some(title) = clean_title(&answer.text) else {
        return;
    };
    if let Ok(index) = chats::rename(&state.chats.root, &agent_id, &chat_id, &title) {
        crate::commands::chat::emit_index(&app, &agent_id, index);
        syslog::log(
            Level::Info,
            "models",
            format!("Named a chat with {} (Use model within app)", model.name),
            &[],
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn titles_are_cleaned() {
        assert_eq!(
            clean_title("\"Trip to Lisbon.\"\n").as_deref(),
            Some("Trip to Lisbon")
        );
        assert_eq!(
            clean_title("<think>hm</think>Title: **Fix the build**").as_deref(),
            Some("Fix the build")
        );
        assert_eq!(clean_title("  \n\n"), None);
    }
}
