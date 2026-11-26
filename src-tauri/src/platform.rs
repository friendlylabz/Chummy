//! The app as a [`Platform`] for `chummy-session`: settings for the
//! configuration, the Chummy folder for chats and (sealed) paused runs, the
//! app's tool sandbox, and Tauri events for updates. How conversations
//! behave lives in `chummy-session`, shared with chummyd.

use std::future::Future;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use chummy_flow::LoopFile;
use chummy_log::runs::LogOptions;
use chummy_sandbox::Jail;
use chummy_session::{ChatStore, LoopProblem, PausedStore, Platform, Session, Update};
use chummy_tools::{Health, ToolManifest};
use tauri::{AppHandle, Emitter, Manager};

use crate::events;
use crate::host::AppServices;
use crate::settings::Settings;
use crate::state::AppState;

#[derive(Clone)]
pub struct AppPlatform {
    pub app: AppHandle,
}

impl AppPlatform {
    pub fn new(app: &AppHandle) -> Self {
        Self { app: app.clone() }
    }

    fn state(&self) -> &AppState {
        self.app.state::<AppState>().inner()
    }
}

impl Platform for AppPlatform {
    type Services = AppServices;
    type Config = Settings;

    fn session(&self) -> &Session {
        &self.state().session
    }

    fn config(&self) -> Settings {
        self.state().settings()
    }

    fn chats(&self) -> &dyn ChatStore {
        &self.state().chats
    }

    fn paused_store(&self) -> &dyn PausedStore {
        self.state()
    }

    fn runnable_loop(&self, id: &str) -> Result<LoopFile, LoopProblem> {
        let state = self.state();
        let file = state.library.load(id).map_err(|_| LoopProblem::Missing)?;
        // Only reviewed (or built-in, or user-made) loops run.
        if !state.trusted(&file) {
            return Err(LoopProblem::Untrusted(file.name));
        }
        Ok(file)
    }

    fn open_jail(&self, path: &Path) -> Result<Jail, String> {
        Jail::open(path, &self.state().protected).map_err(|e| e.to_string())
    }

    fn services(&self) -> Arc<AppServices> {
        Arc::new(AppServices {
            app: self.app.clone(),
        })
    }

    fn installed_tools(&self) -> Vec<ToolManifest> {
        chummy_tools::installed(&self.state().tool_paths)
    }

    fn sandbox_health(&self, config: &Settings) -> Option<Health> {
        Some(self.state().sandbox_health(&config.sandbox))
    }

    fn time_zone(&self) -> Option<String> {
        iana_time_zone::get_timezone().ok()
    }

    fn run_logs(&self) -> Option<(PathBuf, LogOptions)> {
        let state = self.state();
        state
            .run_log_options()
            .map(|opts| (state.run_logs.clone(), opts))
    }

    fn spawn(&self, task: impl Future<Output = ()> + Send + 'static) {
        tauri::async_runtime::spawn(task);
    }

    fn emit(&self, update: Update) {
        let app = &self.app;
        let _ = match update {
            Update::RunStarted(info) => app.emit(events::RUN_STARTED, info),
            Update::RunEvent(e) => app.emit(events::RUN_EVENT, e),
            Update::RunFinished(f) => {
                let r = app.emit(events::RUN_FINISHED, f);
                // A job may have been waiting for this agent.
                if let Some(jobs) = self.state().jobs.get() {
                    jobs.wake();
                }
                r
            }
            Update::RunPausing(p) => app.emit(events::RUN_PAUSING, p),
            Update::RunSaid(s) => app.emit(events::RUN_SAID, s),
            Update::PausedChanged(list) => app.emit(events::PAUSED_CHANGED, list),
            Update::ChatMessage(m) => app.emit(events::CHAT_MESSAGE, m),
            Update::ChatMessageDeleted(d) => app.emit(events::CHAT_MESSAGE_DELETED, d),
            Update::ChatRunUpdated(u) => app.emit(events::CHAT_RUN_UPDATED, u),
            Update::ChatsChanged(c) => app.emit(events::CHATS_CHANGED, c),
            Update::FirstReply(f) => {
                tauri::async_runtime::spawn(crate::in_app::title_chat(
                    app.clone(),
                    f.agent_id,
                    f.chat_id,
                    f.message,
                    f.reply,
                ));
                Ok(())
            }
        };
    }
}
