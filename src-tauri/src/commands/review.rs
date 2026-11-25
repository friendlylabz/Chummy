//! Reviewing a loop before Chummy trusts it: a loop file being imported, a
//! loop in the library that isn't on the trusted list (one copied into the
//! loops folder by hand, say), or a loop configuration that changes an
//! agent's loop. The scan itself is `chummy_flow::review`; this keeps the
//! loop waiting while the Review window shows it, and does what the user
//! chooses (after a native confirmation).

use std::collections::HashMap;
use std::time::Instant;

use chummy_flow::{FlagLevel, LoopFile, LoopReview, Overrides, find_block, review};
use chummy_log::system::{Level, log};
use serde::Serialize;
use tauri::{AppHandle, Emitter, State, WebviewWindow};

use super::CmdResult;
use crate::dialogs;
use crate::events;
use crate::state::AppState;
use crate::windows::{self, AppWindow};

/// Most reviews waiting at once; the oldest is dropped.
const MAX_PENDING: usize = 8;

/// What a review is about, and what trusting it does.
#[derive(Debug, Clone, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum ReviewSource {
    /// A loop file being imported. Trusting it adds it to the library.
    Import { file_name: String },
    /// A loop already in the library. Trusting it lets it run and open.
    Library { id: String },
    /// A loop configuration being imported for an agent. Trusting it
    /// applies the changes to the agent (the review is of the loop as the
    /// agent would run it).
    LoopConfig {
        agent_id: String,
        agent_name: String,
        loop_id: String,
        changes: Vec<ConfigChange>,
        #[serde(skip)]
        overrides: Overrides,
        #[serde(skip)]
        base_print: String,
    },
}

/// One setting a loop configuration changes.
#[derive(Debug, Clone, Serialize)]
pub struct ConfigChange {
    pub block: String,
    pub label: String,
    pub param: String,
    /// The loop's own value (JSON), if it sets one.
    pub from: Option<String>,
    /// The new value (JSON); none removes the loop's.
    pub to: Option<String>,
}

pub struct PendingReview {
    file: LoopFile,
    print: String,
    review: LoopReview,
    source: ReviewSource,
    created: Instant,
}

/// A model connection a loop names, as the review shows it.
#[derive(Debug, Clone, Serialize)]
pub struct NamedModel {
    pub id: String,
    /// The connection's name, if it's one of the user's.
    pub name: Option<String>,
    /// Its address (so "cloud or this computer" can be seen).
    pub base_url: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct ReviewView {
    pub token: String,
    pub source: ReviewSource,
    pub review: LoopReview,
    pub models: Vec<NamedModel>,
}

/// Scans a loop, keeps it waiting under a new token and opens the Review
/// window on it.
pub fn start(app: &AppHandle, state: &AppState, file: LoopFile, source: ReviewSource) -> String {
    let review = review(&file);
    let print = review.fingerprint.clone();
    let token = chummy_store::random_id(16);
    {
        let mut pending = state.lock_reviews();
        while pending.len() >= MAX_PENDING {
            let Some(oldest) = pending
                .iter()
                .min_by_key(|(_, p)| p.created)
                .map(|(k, _)| k.clone())
            else {
                break;
            };
            pending.remove(&oldest);
        }
        pending.insert(
            token.clone(),
            PendingReview {
                file,
                print,
                review,
                source,
                created: Instant::now(),
            },
        );
    }
    if let Err(e) = windows::open(app, AppWindow::Review, Some(&token)) {
        log(
            Level::Error,
            "app",
            "Couldn't open the Review window",
            &[("error", &e.to_string())],
        );
    }
    token
}

/// What a loop configuration changes in a loop, setting by setting.
pub fn config_changes(file: &LoopFile, overrides: &Overrides) -> Vec<ConfigChange> {
    let mut out = Vec::new();
    for (key, params) in overrides {
        let Some(b) = find_block(file, key) else {
            continue;
        };
        for (param, value) in params {
            out.push(ConfigChange {
                block: key.clone(),
                label: if b.label.is_empty() {
                    b.id.clone()
                } else {
                    b.label.clone()
                },
                param: param.clone(),
                from: b.params.get(param).map(|v| v.to_string()),
                to: (!value.is_null()).then(|| value.to_string()),
            });
        }
    }
    out
}

#[tauri::command]
pub fn get_review(state: State<'_, AppState>, token: String) -> CmdResult<ReviewView> {
    let pending = state.lock_reviews();
    let p = pending
        .get(&token)
        .ok_or("This review has ended. Import the loop again to review it.")?;
    let s = state.settings();
    let mut ids: Vec<String> = p
        .review
        .models
        .iter()
        .filter_map(|m| m.model.clone())
        .collect();
    ids.sort();
    ids.dedup();
    let models = ids
        .into_iter()
        .map(|id| {
            let found = s.providers.iter().find(|c| c.id == id);
            NamedModel {
                name: found.map(|c| c.name.clone()),
                base_url: found.map(|c| c.base_url.clone()),
                id,
            }
        })
        .collect();
    Ok(ReviewView {
        token,
        source: p.source.clone(),
        review: p.review.clone(),
        models,
    })
}

/// Drops a review without trusting anything, and closes the window.
#[tauri::command]
pub fn discard_review(window: WebviewWindow, state: State<'_, AppState>, token: String) {
    state.lock_reviews().remove(&token);
    let _ = window.close();
}

/// Shows the review of a loop in the library that isn't trusted.
#[tauri::command]
pub fn review_library_loop(
    app: AppHandle,
    state: State<'_, AppState>,
    id: String,
) -> CmdResult<()> {
    let file = state.library.load(&id)?;
    start(&app, &state, file, ReviewSource::Library { id });
    Ok(())
}

fn counts(r: &LoopReview) -> String {
    let n = |level, one: &str, many: &str| match r.count(level) {
        0 => None,
        1 => Some(format!("1 {one}")),
        k => Some(format!("{k} {many}")),
    };
    let parts: Vec<String> = [
        n(FlagLevel::Serious, "serious flag", "serious flags"),
        n(FlagLevel::Warning, "warning", "warnings"),
    ]
    .into_iter()
    .flatten()
    .collect();
    if parts.is_empty() {
        "The review raised no serious flags or warnings, but that isn't a guarantee.".into()
    } else {
        format!("The review raised {}.", parts.join(" and "))
    }
}

/// Trusts what's being reviewed, after a native confirmation, and closes
/// the window. Returns false with nothing done if the user said no.
#[tauri::command]
pub async fn accept_review(
    app: AppHandle,
    window: WebviewWindow,
    state: State<'_, AppState>,
    token: String,
) -> CmdResult<bool> {
    let done = accept(&app, &window, &state, &token).await?;
    if done {
        let _ = window.close();
    }
    Ok(done)
}

async fn accept(
    app: &AppHandle,
    window: &WebviewWindow,
    state: &AppState,
    token: &str,
) -> CmdResult<bool> {
    let (name, source, summary, print) = {
        let pending = state.lock_reviews();
        let p = pending.get(token).ok_or("This review has ended.")?;
        (
            p.file.name.clone(),
            p.source.clone(),
            counts(&p.review),
            p.print.clone(),
        )
    };
    let (title, what, button) = match &source {
        ReviewSource::Import { .. } => (
            format!("Import and trust “{name}”?"),
            "It will be added to your loops and trusted: it can run with everything an agent using it may do, and it opens in the editor without another review.",
            "Import and trust",
        ),
        ReviewSource::Library { .. } => (
            format!("Trust “{name}”?"),
            "It will be trusted: it can run with everything an agent using it may do, and it opens in the editor without another review.",
            "Trust loop",
        ),
        ReviewSource::LoopConfig { agent_name, .. } => (
            format!("Change {agent_name}'s loop settings?"),
            "The imported settings will change how this agent's loop works, as shown in the review.",
            "Use these settings",
        ),
    };
    let message = format!(
        "{what}\n\n{summary}\n\nOnly continue if it comes from someone you trust. A loop is a program: Chummy's check can miss things."
    );
    if !dialogs::confirm(app, window.label(), &title, &message, button, true).await {
        return Ok(false);
    }
    let Some(p) = state.lock_reviews().remove(token) else {
        return Err("This review has ended.".into());
    };
    let fields = |extra: &[(&'static str, String)]| {
        let mut v = vec![
            ("loop", name.clone()),
            ("fingerprint", print.clone()),
            ("serious", p.review.count(FlagLevel::Serious).to_string()),
            ("warnings", p.review.count(FlagLevel::Warning).to_string()),
        ];
        v.extend(extra.iter().cloned());
        v
    };
    let logged = |msg: &str, f: Vec<(&'static str, String)>| {
        let f: Vec<(&str, &str)> = f.iter().map(|(k, v)| (*k, v.as_str())).collect();
        log(Level::Info, "security", msg, &f);
    };
    match p.source {
        ReviewSource::Import { file_name } => {
            let id = state.library.new_id(&p.file.name);
            state.library.save(&id, &p.file)?;
            state.trust.trust(&print);
            logged(
                "Imported a loop and trusted it after a review",
                fields(&[("file", file_name)]),
            );
            loops_changed(app, state);
            let _ = windows::open(app, AppWindow::Editor, Some(&id));
            Ok(true)
        }
        ReviewSource::Library { id } => {
            let now = state.library.load(&id)?;
            if chummy_flow::fingerprint(&now) != print {
                return Err("The loop has changed since this review. Review it again.".into());
            }
            state.trust.trust(&print);
            logged(
                "Trusted a loop after a review",
                fields(&[("id", id.clone())]),
            );
            loops_changed(app, state);
            let _ = windows::open(app, AppWindow::Editor, Some(&id));
            Ok(true)
        }
        ReviewSource::LoopConfig {
            agent_id,
            loop_id,
            overrides,
            base_print,
            ..
        } => {
            let s = state.settings();
            let agent = s.agent(&agent_id).ok_or("That agent no longer exists.")?;
            let base = state.library.load(&loop_id)?;
            if agent.loop_id != loop_id || chummy_flow::fingerprint(&base) != base_print {
                return Err(
                    "The agent's loop has changed since this review. Import the settings again."
                        .into(),
                );
            }
            state.update(app, |s| {
                if let Some(a) = s.agents.iter_mut().find(|a| a.id == agent_id) {
                    a.loop_config.insert(loop_id.clone(), overrides.clone());
                }
            })?;
            logged(
                "Applied imported loop settings after a review",
                fields(&[("agent", agent_id.clone())]),
            );
            Ok(true)
        }
    }
}

fn loops_changed(app: &AppHandle, state: &AppState) {
    let _ = app.emit(events::LOOPS_CHANGED, state.loops());
    state.broadcast_setup(app);
}

/// The reviews waiting, by token.
pub type Reviews = HashMap<String, PendingReview>;
