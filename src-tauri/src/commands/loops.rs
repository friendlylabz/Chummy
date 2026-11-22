//! The loop library and the editor's helpers.

use chummy_flow::{Analysis, BlockInfo, LoopFile, Template, analyze, catalog, templates};
use tauri::{AppHandle, Emitter, State, WebviewWindow};

use super::CmdResult;
use crate::dialogs;
use crate::events;
use crate::library::LoopSummary;
use crate::state::AppState;

/// Largest loop file Chummy will import.
const MAX_IMPORT_BYTES: u64 = 2 * 1024 * 1024;

fn changed(app: &AppHandle, state: &AppState) {
    let _ = app.emit(events::LOOPS_CHANGED, state.library.list());
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
    state.library.list()
}

#[tauri::command]
pub fn load_loop(state: State<'_, AppState>, id: String) -> CmdResult<LoopFile> {
    state.library.load(&id)
}

/// Checks a loop and returns every block's ports. The editor calls this on
/// each change, so port layouts come from one place: the Rust catalog.
#[tauri::command]
pub fn analyze_loop(file: LoopFile) -> Analysis {
    analyze(&file)
}

/// Saves a loop. With no `id`, a new one is made from the loop's name.
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
    state.library.save(&id, &file)?;
    changed(&app, &state);
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

/// Imports a loop file chosen in a native dialog. Returns the new loop's id.
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
    let id = state.library.new_id(&file.name);
    state.library.save(&id, &file)?;
    changed(&app, &state);
    Ok(Some(id))
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
