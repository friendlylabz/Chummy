//! The loop library and the editor's helpers.

use chummy_flow::{
    ASSISTANT, Analysis, BlockInfo, LoopFile, Template, analyze, catalog, templates,
};
use tauri::{AppHandle, Emitter, State, WebviewWindow};

use super::CmdResult;
use super::review;
use crate::dialogs;
use crate::events;
use crate::state::AppState;
use chummy_agent::library::LoopSummary;

/// Largest loop file Chummy will import.
const MAX_IMPORT_BYTES: u64 = 2 * 1024 * 1024;

fn changed(app: &AppHandle, state: &AppState) {
    let _ = app.emit(events::LOOPS_CHANGED, state.loops());
    state.broadcast_setup(app);
}

#[tauri::command]
pub fn block_catalog() -> Vec<BlockInfo> {
    catalog()
}

#[tauri::command]
pub fn loop_templates() -> Vec<Template> {
    templates()
}

#[tauri::command]
pub fn list_loops(state: State<'_, AppState>) -> Vec<LoopSummary> {
    state.loops()
}

/// Opens a loop for the editor. Loops that aren't trusted are refused: they
/// get a review first (see `review_library_loop`).
#[tauri::command]
pub fn load_loop(state: State<'_, AppState>, id: String) -> CmdResult<LoopFile> {
    state.trusted_loop(&id)
}

/// Checks a loop and returns every block's ports. The editor calls this on
/// each change, so port layouts come from one place: the Rust catalog.
#[tauri::command]
pub fn analyze_loop(file: LoopFile) -> Analysis {
    analyze(&file)
}

/// Saves a loop from the editor. With no `id`, a new one is made from the
/// loop's name.
///
/// What the editor holds came from a trusted loop (the editor can only open
/// trusted loops), a template or a blank canvas, plus the user's own edits,
/// so the saved version is trusted too, unless it replaces a loop in the
/// library that isn't trusted (then it stays that way until reviewed).
#[tauri::command]
pub fn save_loop(
    app: AppHandle,
    state: State<'_, AppState>,
    id: Option<String>,
    file: LoopFile,
) -> CmdResult<String> {
    if file.name.trim().is_empty() {
        return Err("Give the loop a name.".into());
    }
    let id = id
        .filter(|i| !i.is_empty())
        .unwrap_or_else(|| state.library.new_id(&file.name));
    let replaces_untrusted = state.library.list().iter().any(|l| l.id == id)
        && !state.library.load(&id).is_ok_and(|old| state.trusted(&old));
    state.library.save(&id, &file)?;
    if !replaces_untrusted {
        state.trust.trust(&chummy_flow::fingerprint(&file));
    }
    changed(&app, &state);
    Ok(id)
}

/// Adds the recommended loop (the Chummy assistant template) to the library,
/// or finds the copy already there. Returns its id.
#[tauri::command]
pub fn add_recommended_loop(app: AppHandle, state: State<'_, AppState>) -> CmdResult<String> {
    let id = recommended_loop(&state)?;
    changed(&app, &state);
    Ok(id)
}

pub fn recommended_loop(state: &AppState) -> CmdResult<String> {
    let t = templates()
        .into_iter()
        .find(|t| t.id == ASSISTANT)
        .ok_or("The recommended loop is missing.")?;
    if let Some(found) = state
        .library
        .list()
        .into_iter()
        .find(|l| l.name == t.file.name)
    {
        return Ok(found.id);
    }
    let id = state.library.new_id(&t.file.name);
    state.library.save(&id, &t.file)?;
    Ok(id)
}

/// Deletes a loop after a native confirmation that names the agents using it.
#[tauri::command]
pub async fn delete_loop(
    app: AppHandle,
    window: WebviewWindow,
    state: State<'_, AppState>,
    id: String,
) -> CmdResult<bool> {
    let name = state
        .library
        .load(&id)
        .map(|l| l.name)
        .unwrap_or_else(|_| id.clone());
    let users: Vec<String> = state
        .settings()
        .agents
        .iter()
        .filter(|a| a.loop_id == id)
        .map(|a| a.name.clone())
        .collect();
    let mut msg = format!("“{name}” will be deleted. Export it first if you might want it again.");
    if !users.is_empty() {
        msg.push_str(&format!(
            "\n\nThese agents use it and won't be able to chat until you choose another loop: {}.",
            users.join(", ")
        ));
    }
    if !dialogs::confirm(
        &app,
        window.label(),
        "Delete this loop?",
        &msg,
        "Delete loop",
        true,
    )
    .await
    {
        return Ok(false);
    }
    state.library.delete(&id)?;
    changed(&app, &state);
    Ok(true)
}

/// Imports a loop file chosen in a native dialog. Returns the new loop's id,
/// or none if it was cancelled or the loop needs a review first (the Review
/// window opens; trusting the loop there imports it).
#[tauri::command]
pub async fn import_loop(
    app: AppHandle,
    window: WebviewWindow,
    state: State<'_, AppState>,
) -> CmdResult<Option<String>> {
    let Some(path) = dialogs::pick_loop_file(&app, window.label()).await else {
        return Ok(None);
    };
    let result = (|| {
        let len = std::fs::metadata(&path).map_err(|e| e.to_string())?.len();
        if len > MAX_IMPORT_BYTES {
            return Err("That file is too big to be a loop.".to_string());
        }
        let text = std::fs::read_to_string(&path).map_err(|e| e.to_string())?;
        LoopFile::from_json(&text).map_err(|e| e.to_string())
    })();
    let file = match result {
        Ok(f) => f,
        Err(e) => {
            dialogs::error(
                &app,
                window.label(),
                "Couldn't import that loop",
                &format!("{}\n\n{e}", path.display()),
            )
            .await;
            return Ok(None);
        }
    };
    // A loop Chummy already trusts (one exported from here, say) comes
    // straight in; anything else is reviewed first, in its own window.
    if state.trusted(&file) {
        let id = state.library.new_id(&file.name);
        state.library.save(&id, &file)?;
        changed(&app, &state);
        return Ok(Some(id));
    }
    let file_name = path
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default();
    review::start(
        &app,
        &state,
        file,
        review::ReviewSource::Import { file_name },
    );
    Ok(None)
}

/// Exports a loop to a file chosen in a native dialog.
#[tauri::command]
pub async fn export_loop(
    app: AppHandle,
    window: WebviewWindow,
    state: State<'_, AppState>,
    id: String,
) -> CmdResult<bool> {
    let file = state.library.load(&id)?;
    let Some(path) = dialogs::save_loop_file(&app, window.label(), &format!("{id}.json")).await
    else {
        return Ok(false);
    };
    std::fs::write(&path, file.to_json())
        .map_err(|e| format!("Couldn't save {}: {e}", path.display()))?;
    Ok(true)
}
