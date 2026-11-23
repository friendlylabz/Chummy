//! Shared app state. Rust is the single source of truth; windows read it
//! through commands and hear about changes through events.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use chummy_log::crypt::LogKey;
use chummy_log::runs::LogOptions;
use std::sync::atomic::AtomicBool;
use std::sync::{Arc, Mutex, OnceLock};

use chummy_local::engine::Engine;
use chummy_local::runtime::RuntimePaths;
use chummy_sandbox::Protected;
use chummy_scheduler::{AskBroker, Manager};
use chummy_store::secrets::{Keychain, SecretStore};
use chummy_store::{ChummyDirs, JsonDoc, Recovery};
use chummy_tools::{ToolHost, ToolPaths};
use serde::Serialize;
use tauri::{AppHandle, Emitter};

use crate::commands::jobs::AppJobs;
use crate::events;
use crate::library::Library;
use crate::settings::{self, Settings};

/// Keychain service name for all of Chummy's secrets.
pub const KEYCHAIN_SERVICE: &str = "com.friendlylabz.chummy";
/// The keychain entry holding the run log key.
pub const LOG_KEY_SECRET: &str = "log-key";
/// Keychain entry for the Hugging Face token.
pub const HF_TOKEN_KEY: &str = "huggingface";

/// A loop run in progress.
#[derive(Debug, Clone, Serialize)]
pub struct RunInfo {
    pub run_id: String,
    pub agent_id: Option<String>,
    pub title: String,
    /// "chat", "test" or "job".
    pub source: &'static str,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub job_id: Option<String>,
    pub started_at: u64,
    #[serde(skip)]
    pub cancel: Arc<AtomicBool>,
}

pub struct AppState {
    pub dirs: ChummyDirs,
    pub protected: Protected,
    pub secrets: Box<dyn SecretStore>,
    pub runtime: RuntimePaths,
    pub engine: Engine,
    pub library: Library,
    settings: Mutex<Settings>,
    doc: JsonDoc<Settings>,
    /// Set if settings had to be recovered at startup.
    pub recovery: Option<Recovery>,
    /// Cancel flags for downloads in progress, by repo.
    pub downloads: Mutex<HashMap<String, Arc<AtomicBool>>>,
    /// Cancel flag for a runtime install in progress.
    pub runtime_install: Mutex<Option<Arc<AtomicBool>>>,
    /// Folders the user picked and confirmed, waiting to be attached to an
    /// agent, by one-time token. The UI only ever holds the token.
    pub picked_workspaces: Mutex<HashMap<String, PathBuf>>,
    /// Runs in progress, by run id.
    pub runs: Mutex<HashMap<String, RunInfo>>,
    pub tool_paths: ToolPaths,
    /// The tool sandbox, created (Python compiled) on first use.
    tool_host: tokio::sync::Mutex<Option<Arc<ToolHost>>>,
    /// Set while the tool runtime is being set up.
    pub tools_installing: Mutex<bool>,
    /// Questions and tool approvals waiting for the user, from every run.
    pub asks: AskBroker,
    /// Tokens used per model connection per day, for daily limits.
    pub usage: Arc<chummy_runner::usage::UsageLedger>,
    /// `logs/runs`: one log per run of a loop.
    pub run_logs: PathBuf,
    /// The run log key, once read from (or made in) the keychain.
    log_key: Mutex<Option<chummy_log::crypt::LogKey>>,
    /// The job scheduler, started once the app is up.
    pub jobs: OnceLock<Manager<AppJobs>>,
    /// Set once the user chose to quit, so closing windows really quits.
    pub quitting: AtomicBool,
    /// Whether the tray icon could be created (some Linux desktops have none).
    pub tray_ready: AtomicBool,
    /// Held while this copy of Chummy runs, so a second copy can't run the
    /// same jobs or write the same files.
    _instance: std::fs::File,
}

/// Returned by [`AppState::open`] when another copy of Chummy is running.
pub const ALREADY_RUNNING: &str = "Chummy is already running.";

/// What the main window needs to know before it unlocks chat.
#[derive(Debug, Clone, Copy, Serialize)]
pub struct SetupStatus {
    pub model_configured: bool,
    pub loop_configured: bool,
    pub agent_configured: bool,
}

impl AppState {
    /// Opens (creating if needed) the Chummy folder and loads settings.
    pub fn open(data_dir: &Path, extra_protected: &[&Path]) -> Result<Self, String> {
        let dirs = ChummyDirs::at(data_dir);
        dirs.create()
            .map_err(|e| format!("couldn't create the Chummy folder: {e}"))?;

        // The single-instance plugin normally stops a second copy first;
        // this lock is the backstop (e.g. without a session bus on Linux).
        let instance = std::fs::OpenOptions::new()
            .create(true)
            .truncate(false)
            .write(true)
            .open(dirs.root.join("chummy.lock"))
            .map_err(|e| format!("couldn't open the Chummy folder's lock: {e}"))?;
        if let Err(e) = instance.try_lock() {
            return Err(match e {
                std::fs::TryLockError::WouldBlock => ALREADY_RUNNING.to_string(),
                std::fs::TryLockError::Error(e) => format!("couldn't lock the Chummy folder: {e}"),
            });
        }

        let mut chummy: Vec<&Path> = vec![&dirs.root];
        chummy.extend_from_slice(extra_protected);
        let protected = Protected::for_current_user(&chummy);

        let doc = JsonDoc::<Settings>::new(&dirs.settings_file);
        // Version on disk before migration, so an upgraded file is rewritten.
        let disk_version = std::fs::read(&dirs.settings_file)
            .ok()
            .and_then(|b| serde_json::from_slice::<serde_json::Value>(&b).ok())
            .and_then(|v| v["version"].as_u64());
        let loaded = doc.load(settings::migrate).map_err(|e| e.to_string())?;
        let mut s = loaded.value;
        if s.active_agent
            .as_ref()
            .is_some_and(|a| s.agent(a).is_none())
        {
            s.active_agent = s.agents.first().map(|a| a.id.clone());
        }
        let outdated = disk_version.is_some_and(|v| v < settings::VERSION as u64);
        if loaded.recovery.is_some() || outdated || s.version != settings::VERSION {
            s.version = settings::VERSION;
            let _ = doc.save(&s);
        }

        let runtime = RuntimePaths::new(&dirs.runtime);
        let engine = Engine::new(&runtime.python, &runtime.engine_dir);
        let library = Library::new(&dirs.loops);
        library.upgrade_builtins();
        Ok(Self {
            secrets: Box::new(Keychain::new(KEYCHAIN_SERVICE)),
            library,
            engine,
            runtime,
            protected,
            settings: Mutex::new(s),
            doc,
            recovery: loaded.recovery,
            downloads: Mutex::new(HashMap::new()),
            runtime_install: Mutex::new(None),
            picked_workspaces: Mutex::new(HashMap::new()),
            runs: Mutex::new(HashMap::new()),
            tool_paths: ToolPaths::new(&dirs.tools),
            tool_host: tokio::sync::Mutex::new(None),
            tools_installing: Mutex::new(false),
            asks: AskBroker::new(),
            usage: Arc::new(chummy_runner::usage::UsageLedger::open(
                &dirs.root.join("usage.json"),
            )),
            run_logs: dirs.logs.join("runs"),
            log_key: Mutex::new(None),
            jobs: OnceLock::new(),
            quitting: AtomicBool::new(false),
            tray_ready: AtomicBool::new(false),
            _instance: instance,
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
            crate::settings::log_changes(&guard, &next);
            *guard = next.clone();
            (r, next)
        };
        let _ = app.emit(
            events::SETTINGS_CHANGED,
            crate::commands::general::settings_view(self, &snapshot),
        );
        self.broadcast_setup(app);
        Ok(r)
    }

    pub fn broadcast_setup(&self, app: &AppHandle) {
        let _ = app.emit(events::SETUP_CHANGED, self.setup_status());
    }

    pub fn setup_status(&self) -> SetupStatus {
        let s = lock(&self.settings);
        SetupStatus {
            model_configured: !s.providers.is_empty(),
            loop_configured: !self.library.list().is_empty(),
            agent_configured: !s.agents.is_empty(),
        }
    }

    /// The tool sandbox. The first call compiles the WebAssembly Python
    /// (cached on disk, so later starts are quick).
    pub async fn tool_host(&self) -> Result<Arc<ToolHost>, String> {
        let mut guard = self.tool_host.lock().await;
        if let Some(h) = guard.as_ref() {
            return Ok(h.clone());
        }
        if chummy_tools::python_status(&self.tool_paths).is_none() {
            return Err("Tools aren't set up yet. Set them up in Settings → Tools.".into());
        }
        let paths = self.tool_paths.clone();
        let host = tokio::task::spawn_blocking(move || ToolHost::new(&paths, tools_http()))
            .await
            .map_err(|e| e.to_string())?
            .map_err(|e| format!("The tool sandbox couldn't start: {e}"))?;
        let host = Arc::new(host);
        *guard = Some(host.clone());
        Ok(host)
    }

    pub fn has_secret(&self, key: &str) -> bool {
        matches!(self.secrets.get(key), Ok(Some(_)))
    }

    /// The run log key from the keychain. With `create`, a new one is made
    /// (and saved there) if there's none yet.
    pub fn log_key(&self, create: bool) -> Result<Option<LogKey>, String> {
        let mut cached = lock(&self.log_key);
        if let Some(k) = cached.as_ref() {
            return Ok(Some(k.clone()));
        }
        let stored = self
            .secrets
            .get(LOG_KEY_SECRET)
            .map_err(|e| format!("The keychain couldn't be read: {e}"))?;
        let key = match stored {
            Some(s) => LogKey::from_base64(&s)?,
            None if create => {
                let k = LogKey::generate()?;
                self.secrets
                    .set(LOG_KEY_SECRET, &k.to_base64())
                    .map_err(|e| format!("The log key couldn't be saved in the keychain: {e}"))?;
                chummy_log::system::log(
                    chummy_log::system::Level::Info,
                    "security",
                    "Made a new log key and saved it in the keychain",
                    &[("fingerprint", &k.fingerprint())],
                );
                k
            }
            None => return Ok(None),
        };
        *cached = Some(key.clone());
        Ok(Some(key))
    }

    /// Replaces the run log key (after an import).
    pub fn set_log_key(&self, key: LogKey) -> Result<(), String> {
        self.secrets
            .set(LOG_KEY_SECRET, &key.to_base64())
            .map_err(|e| format!("The log key couldn't be saved in the keychain: {e}"))?;
        *lock(&self.log_key) = Some(key);
        Ok(())
    }

    /// How to write a new run's log, or `None` for no log. A log that should
    /// be encrypted but can't be (no keychain) isn't written at all.
    pub fn run_log_options(&self) -> Option<LogOptions> {
        let logging = self.settings().logging;
        let detail = logging.runs.detail()?;
        let key = if logging.encrypt {
            match self.log_key(true) {
                Ok(k) => k,
                Err(e) => {
                    chummy_log::system::log(
                        chummy_log::system::Level::Warn,
                        "security",
                        "This run isn't logged: its log should be encrypted, but there's no log key",
                        &[("error", &e)],
                    );
                    return None;
                }
            }
        } else {
            None
        };
        Some(LogOptions { detail, key })
    }
}

/// The HTTP client for the tool runtime's downloads.
pub fn tools_http() -> reqwest::Client {
    reqwest::Client::builder()
        .connect_timeout(std::time::Duration::from_secs(15))
        .user_agent(concat!("Chummy/", env!("CARGO_PKG_VERSION")))
        .build()
        .unwrap_or_default()
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
        assert!(!status.agent_configured && !status.model_configured && !status.loop_configured);
        assert!(tmp.path().join("chummy/models").is_dir());
        assert!(tmp.path().join("chummy/chats").is_dir());
        assert!(state.recovery.is_none());
    }

    #[test]
    fn only_one_copy_opens_the_folder() {
        let tmp = tempfile::tempdir().unwrap();
        let data = tmp.path().join("chummy");
        let first = AppState::open(&data, &[]).unwrap();
        assert_eq!(
            AppState::open(&data, &[]).err().as_deref(),
            Some(ALREADY_RUNNING)
        );
        drop(first);
        assert!(AppState::open(&data, &[]).is_ok());
    }

    #[test]
    fn v1_settings_upgrade_on_open() {
        let tmp = tempfile::tempdir().unwrap();
        let data = tmp.path().join("chummy");
        std::fs::create_dir_all(&data).unwrap();
        let v1 =
            serde_json::json!({ "version": 1, "workspace": { "path": "/tmp", "confirmed_at": 1 } });
        std::fs::write(data.join("settings.json"), v1.to_string()).unwrap();
        let state = AppState::open(&data, &[]).unwrap();
        assert_eq!(state.settings().version, settings::VERSION);
        let saved = std::fs::read_to_string(data.join("settings.json")).unwrap();
        assert!(!saved.contains("workspace"));
    }
}
