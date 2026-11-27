//! chummyd as a `chummy-session` [`Platform`]: the bundle for the
//! configuration, memory (saved in the sealed state) for chats and paused
//! runs, the WebAssembly sandbox for tools, and the router for updates.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

use base64::Engine as _;
use base64::engine::general_purpose::STANDARD as B64;
use chummy_agent::{Agent, AgentConfig, Permissions, Workspace};
use chummy_bundle::Config;
use chummy_flow::LoopFile;
use chummy_llm::ProviderConfig;
use chummy_log::runs::LogOptions;
use chummy_runner::Services;
use chummy_sandbox::{Jail, Protected};
use chummy_session::{
    ChatStore, LoopProblem, MemoryChats, PausedOpenError, PausedStore, Platform, Session, Update,
};
use chummy_tools::sandbox::NativeSandbox;
use chummy_tools::{Health, SandboxBackend, ToolHost, ToolManifest, ToolPaths};
use tokio::sync::mpsc::UnboundedSender;
use zeroize::Zeroizing;

use crate::router::Input;

/// The configuration as agents run under it (each agent's folder is its
/// jail).
#[derive(Clone)]
pub struct DaemonConfig {
    pub agents: Vec<Agent>,
    pub providers: Vec<ProviderConfig>,
    pub default_provider: Option<String>,
    pub permissions: Permissions,
}

impl DaemonConfig {
    pub fn new(c: &Config, jails: &BTreeMap<String, PathBuf>) -> Self {
        Self {
            agents: c
                .agents
                .iter()
                .map(|a| Agent {
                    id: a.id.clone(),
                    name: a.name.clone(),
                    loop_id: a.loop_id.clone(),
                    model: a.model.clone(),
                    workspace: Workspace {
                        path: jails.get(&a.id).cloned().unwrap_or_default(),
                        confirmed_at: 0,
                    },
                    created_at: 0,
                    tools: a.tools.clone(),
                    loop_config: a.loop_config.clone(),
                    model_options: a.model_options.clone(),
                })
                .collect(),
            providers: c.providers.clone(),
            default_provider: c.default_provider.clone(),
            permissions: c.permissions.clone(),
        }
    }
}

impl AgentConfig for DaemonConfig {
    fn agents(&self) -> &[Agent] {
        &self.agents
    }
    fn providers(&self) -> &[ProviderConfig] {
        &self.providers
    }
    fn default_provider(&self) -> Option<&str> {
        self.default_provider.as_deref()
    }
    fn permissions(&self) -> &Permissions {
        &self.permissions
    }
}

/// Paused runs, kept in memory and saved (sealed) with the state.
#[derive(Default)]
pub struct MemoryPaused(Mutex<BTreeMap<(String, String), Vec<u8>>>);

impl MemoryPaused {
    fn lock(&self) -> std::sync::MutexGuard<'_, BTreeMap<(String, String), Vec<u8>>> {
        self.0.lock().unwrap_or_else(|e| e.into_inner())
    }

    pub fn from_snapshot(s: &BTreeMap<String, String>) -> Self {
        let m = Self::default();
        for (k, v) in s {
            if let (Some((a, c)), Ok(bytes)) = (k.split_once('/'), B64.decode(v)) {
                m.lock().insert((a.into(), c.into()), bytes);
            }
        }
        m
    }

    pub fn snapshot(&self) -> BTreeMap<String, String> {
        self.lock()
            .iter()
            .map(|((a, c), b)| (format!("{a}/{c}"), B64.encode(b)))
            .collect()
    }
}

impl PausedStore for MemoryPaused {
    fn seal(&self, agent: &str, chat: &str, bytes: &[u8]) -> Result<(), String> {
        if !chummy_agent::valid_id(agent) || !chummy_agent::valid_id(chat) {
            return Err("invalid id".into());
        }
        self.lock()
            .insert((agent.into(), chat.into()), bytes.to_vec());
        Ok(())
    }
    fn open(&self, agent: &str, chat: &str) -> Result<Option<Zeroizing<Vec<u8>>>, PausedOpenError> {
        Ok(self
            .lock()
            .get(&(agent.into(), chat.into()))
            .cloned()
            .map(Zeroizing::new))
    }
    fn take(&self, agent: &str, chat: &str) -> Result<Option<Zeroizing<Vec<u8>>>, String> {
        Ok(self
            .lock()
            .remove(&(agent.into(), chat.into()))
            .map(Zeroizing::new))
    }
    fn discard(&self, agent: &str, chat: &str) -> Result<(), String> {
        self.lock().remove(&(agent.into(), chat.into()));
        Ok(())
    }
    fn discard_agent(&self, agent: &str) {
        self.lock().retain(|(a, _), _| a != agent);
    }
    fn list(&self) -> Vec<(String, String)> {
        self.lock().keys().cloned().collect()
    }
}

/// Secrets from the bundle, the WebAssembly sandbox, no built-in engine.
pub struct DaemonServices {
    secrets: BTreeMap<String, String>,
    paths: ToolPaths,
    host: tokio::sync::OnceCell<Arc<ToolHost>>,
}

impl DaemonServices {
    pub fn new(secrets: BTreeMap<String, String>, paths: ToolPaths) -> Self {
        Self {
            secrets,
            paths,
            host: tokio::sync::OnceCell::new(),
        }
    }

    /// The tool sandbox, made (Python compiled) on first use.
    pub async fn tool_host(&self) -> Result<Arc<ToolHost>, String> {
        self.host
            .get_or_try_init(|| async {
                let paths = self.paths.clone();
                tokio::task::spawn_blocking(move || {
                    ToolHost::new(&paths, tools_http())
                        .map(Arc::new)
                        .map_err(|e| format!("The tool sandbox couldn't start: {e}"))
                })
                .await
                .map_err(|e| e.to_string())?
            })
            .await
            .cloned()
    }
}

impl Services for DaemonServices {
    fn secret(&self, key: &str) -> Option<String> {
        self.secrets.get(key).cloned()
    }

    async fn sandbox(&self) -> Result<Arc<dyn SandboxBackend>, String> {
        Ok(Arc::new(NativeSandbox::new(self.tool_host().await?)))
    }

    fn tool_paths(&self) -> ToolPaths {
        self.paths.clone()
    }

    async fn local_engine(&self, _: &ProviderConfig) -> Result<(String, String), String> {
        Err("chummyd has no built-in engine: connect Ollama or another server instead.".into())
    }
}

/// The HTTP client for tools' web requests.
pub fn tools_http() -> reqwest::Client {
    chummy_http::builder()
        .connect_timeout(std::time::Duration::from_secs(15))
        .user_agent(concat!("chummyd/", env!("CARGO_PKG_VERSION")))
        .build()
        .unwrap_or_default()
}

pub struct Inner {
    pub session: Session,
    pub bundle: Arc<Config>,
    pub config: DaemonConfig,
    pub chats: MemoryChats,
    pub paused: MemoryPaused,
    pub protected: Protected,
    pub services: Arc<DaemonServices>,
    pub tool_paths: ToolPaths,
    pub inbox: UnboundedSender<Input>,
    /// Something changed since the state was last saved.
    pub dirty: AtomicBool,
}

#[derive(Clone)]
pub struct DaemonPlatform(pub Arc<Inner>);

impl DaemonPlatform {
    pub fn mark_dirty(&self) {
        self.0.dirty.store(true, Ordering::Relaxed);
    }
}

impl Platform for DaemonPlatform {
    type Services = DaemonServices;
    type Config = DaemonConfig;

    fn session(&self) -> &Session {
        &self.0.session
    }
    fn config(&self) -> DaemonConfig {
        self.0.config.clone()
    }
    fn chats(&self) -> &dyn ChatStore {
        &self.0.chats
    }
    fn paused_store(&self) -> &dyn PausedStore {
        &self.0.paused
    }
    fn runnable_loop(&self, id: &str) -> Result<LoopFile, LoopProblem> {
        // The bundle's loops are trusted: it can't change without its password.
        self.0
            .bundle
            .loops
            .get(id)
            .cloned()
            .ok_or(LoopProblem::Missing)
    }
    fn open_jail(&self, path: &Path) -> Result<Jail, String> {
        Jail::open(path, &self.0.protected).map_err(|e| e.to_string())
    }
    fn services(&self) -> Arc<DaemonServices> {
        self.0.services.clone()
    }
    fn installed_tools(&self) -> Vec<ToolManifest> {
        chummy_tools::installed(&self.0.tool_paths)
    }
    fn sandbox_health(&self, _: &DaemonConfig) -> Option<Health> {
        Some(NativeSandbox::health(&self.0.tool_paths))
    }
    fn time_zone(&self) -> Option<String> {
        Some(self.0.bundle.instance.time_zone.clone())
    }
    fn job_time_zone(&self) -> jiff::tz::TimeZone {
        jiff::tz::TimeZone::get(&self.0.bundle.instance.time_zone)
            .unwrap_or_else(|_| jiff::tz::TimeZone::system())
    }
    fn run_logs(&self) -> Option<(PathBuf, LogOptions)> {
        None
    }
    fn spawn(&self, task: impl std::future::Future<Output = ()> + Send + 'static) {
        tokio::spawn(task);
    }
    fn emit(&self, update: Update) {
        self.mark_dirty();
        let _ = self.0.inbox.send(Input::Update(Box::new(update)));
    }
}
