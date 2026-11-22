//! Shared app state. Rust is the single source of truth; windows read it
//! through commands and hear about changes through events.

use std::collections::HashMap;
use std::path::Path;
use std::sync::atomic::AtomicBool;
use std::sync::{Arc, Mutex};

use chummy_local::engine::Engine;
use chummy_local::runtime::RuntimePaths;
use chummy_sandbox::Protected;
use chummy_store::secrets::{Keychain, SecretStore};
use chummy_store::{ChummyDirs, JsonDoc, Recovery};
use serde::Serialize;
use tauri::{AppHandle, Emitter};

use crate::events;
use crate::settings::{self, Settings};

/// Keychain service name for all of Chummy's secrets.
pub const KEYCHAIN_SERVICE: &str = "com.friendlylabz.chummy";
/// Keychain entry for the Hugging Face token.
pub const HF_TOKEN_KEY: &str = "huggingface";

pub struct AppState {
    pub dirs: ChummyDirs,
    pub protected: Protected,
    pub secrets: Box<dyn SecretStore>,
    pub runtime: RuntimePaths,
    pub engine: Engine,
    settings: Mutex<Settings>,
    doc: JsonDoc<Settings>,
    /// Set if settings had to be recovered at startup.
    pub recovery: Option<Recovery>,
    /// Cancel flags for downloads in progress, by repo.
    pub downloads: Mutex<HashMap<String, Arc<AtomicBool>>>,
    /// Cancel flag for a runtime install in progress.
    pub runtime_install: Mutex<Option<Arc<AtomicBool>>>,
}

/// What the main window needs to know before it unlocks chat.
#[derive(Debug, Clone, Copy, Serialize)]
pub struct SetupStatus {
    pub workspace_configured: bool,
    pub model_configured: bool,
    pub loop_configured: bool,
}

impl AppState {
    /// Opens (creating if needed) the Chummy folder and loads settings.
    pub fn open(data_dir: &Path, extra_protected: &[&Path]) -> Result<Self, String> {
        let dirs = ChummyDirs::at(data_dir);
        dirs.create()
            .map_err(|e| format!("couldn't create the Chummy folder: {e}"))?;

        let mut chummy: Vec<&Path> = vec![&dirs.root];
        chummy.extend_from_slice(extra_protected);
        let protected = Protected::for_current_user(&chummy);

        let doc = JsonDoc::<Settings>::new(&dirs.settings_file);
        let loaded = doc.load(settings::migrate).map_err(|e| e.to_string())?;
        let mut s = loaded.value;
        // A workspace that is no longer valid (deleted, or the rules got
        // stricter) is dropped rather than trusted.
        if let Some(ws) = &s.workspace
            && chummy_sandbox::validate_workspace(&ws.path, &protected).is_err()
        {
            s.workspace = None;
        }
        if loaded.recovery.is_some() || s.version != settings::VERSION {
            s.version = settings::VERSION;
            let _ = doc.save(&s);
        }

        let runtime = RuntimePaths::new(&dirs.runtime);
        let engine = Engine::new(&runtime.python, &runtime.engine_dir);
        Ok(Self {
            secrets: Box::new(Keychain::new(KEYCHAIN_SERVICE)),
            engine,
            runtime,
            protected,
            settings: Mutex::new(s),
            doc,
            recovery: loaded.recovery,
            downloads: Mutex::new(HashMap::new()),
            runtime_install: Mutex::new(None),
            dirs,
        })
    }

    pub fn settings(&self) -> Settings {
        lock(&self.settings).clone()
    }

    /// Applies `f` to the settings, saves them atomically, and tells every window.
    pub fn update<R>(
        &self,
        app: &AppHandle,
        f: impl FnOnce(&mut Settings) -> R,
    ) -> Result<R, String> {
        let (r, snapshot) = {
            let mut guard = lock(&self.settings);
            let mut next = guard.clone();
            let r = f(&mut next);
            self.doc.save(&next).map_err(|e| e.to_string())?;
            *guard = next.clone();
            (r, next)
        };
        let _ = app.emit(
            events::SETTINGS_CHANGED,
            crate::commands::general::settings_view(self, &snapshot),
        );
        let _ = app.emit(events::SETUP_CHANGED, self.setup_status());
        Ok(r)
    }

    pub fn setup_status(&self) -> SetupStatus {
        let s = lock(&self.settings);
        SetupStatus {
            workspace_configured: s.workspace.is_some(),
            model_configured: !s.providers.is_empty(),
            loop_configured: std::fs::read_dir(&self.dirs.loops).is_ok_and(|d| {
                d.flatten()
                    .any(|e| e.path().extension().is_some_and(|x| x == "json"))
            }),
        }
    }

    pub fn has_secret(&self, key: &str) -> bool {
        matches!(self.secrets.get(key), Ok(Some(_)))
    }
}

/// Locks a mutex, recovering the data if a previous holder panicked.
pub fn lock<T>(m: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|e| e.into_inner())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn first_launch_is_not_ready_and_creates_the_folder() {
        let tmp = tempfile::tempdir().unwrap();
        let state = AppState::open(&tmp.path().join("chummy"), &[]).unwrap();
        let status = state.setup_status();
        assert!(
            !status.workspace_configured && !status.model_configured && !status.loop_configured
        );
        assert!(tmp.path().join("chummy/models").is_dir());
        assert!(state.recovery.is_none());
    }

    #[test]
    fn invalid_saved_workspace_is_dropped() {
        let tmp = tempfile::tempdir().unwrap();
        let data = tmp.path().join("chummy");
        std::fs::create_dir_all(&data).unwrap();
        // A workspace pointing inside the Chummy folder must never load.
        let bad = serde_json::json!({ "version": 1, "workspace": { "path": data.join("models"), "confirmed_at": 1 } });
        std::fs::write(data.join("settings.json"), bad.to_string()).unwrap();
        let state = AppState::open(&data, &[]).unwrap();
        assert!(state.settings().workspace.is_none());
    }
}
