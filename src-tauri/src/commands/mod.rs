//! IPC commands. Every command must also be listed in `build.rs` and allowed in
//! the capability file of each window that may call it.

use chummy_store::{Settings, Theme};
use tauri::{AppHandle, Emitter, State};

use crate::events;
use crate::state::{AppState, SetupStatus, lock};
use crate::windows::{self, AppWindow};

/// Error type returned to the frontend as a plain string.
#[derive(Debug, thiserror::Error)]
pub enum CommandError {
    #[error(transparent)]
    Tauri(#[from] tauri::Error),
}

impl serde::Serialize for CommandError {
    fn serialize<S: serde::Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        s.serialize_str(&self.to_string())
    }
}

type CmdResult<T> = Result<T, CommandError>;

/// Whether a model and a loop are configured. The main window keeps chat
/// disabled until both are true.
#[tauri::command]
pub fn setup_status(state: State<'_, AppState>) -> SetupStatus {
    state.setup_status()
}

/// Opens (or focuses) a secondary window.
///
/// Async on purpose: creating windows from a synchronous command can deadlock
/// on Windows.
#[tauri::command]
pub async fn open_window(app: AppHandle, window: AppWindow) -> CmdResult<()> {
    windows::open(&app, window)?;
    Ok(())
}

#[tauri::command]
pub fn get_settings(state: State<'_, AppState>) -> Settings {
    lock(&state.store).settings.clone()
}

/// Changes the theme and tells every window.
#[tauri::command]
pub fn set_theme(app: AppHandle, state: State<'_, AppState>, theme: Theme) -> CmdResult<()> {
    let settings = {
        let mut store = lock(&state.store);
        store.settings.theme = theme;
        store.settings.clone()
    };
    app.emit(events::SETTINGS_CHANGED, settings)?;
    Ok(())
}
