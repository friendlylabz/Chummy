//! The app's settings document (`settings.json` in the Chummy folder).
//!
//! Rules for changing this schema:
//! - every field has a default (`#[serde(default)]`), so partial files load;
//! - renames or meaning changes bump [`VERSION`]. Until 1.0 there are no
//!   migrations: a file from an older version that no longer reads is set
//!   aside and Chummy starts fresh.

use std::collections::BTreeMap;

use chummy_llm::ProviderConfig;
use serde::{Deserialize, Serialize};

pub const VERSION: u32 = 4;

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Theme {
    #[default]
    System,
    Dark,
    Light,
}

pub use chummy_agent::{Agent, AgentConfig, AgentTool, Permissions, Workspace};

/// What closing the main window does.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CloseAction {
    /// Ask: quit, or keep running in the tray.
    #[default]
    Ask,
    /// Quit (still asking first if anything is running).
    Quit,
    /// Keep running in the tray.
    Tray,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct General {
    pub close: CloseAction,
    /// Desktop notifications for questions and finished jobs.
    pub notifications: bool,
    /// Most scheduled runs going at once (1–4).
    pub max_job_runs: u32,
    /// Show replies with Markdown formatting (headings, lists, code…).
    pub markdown: bool,
    /// Show LaTeX math in replies.
    pub math: bool,
}

impl Default for General {
    fn default() -> Self {
        Self {
            close: CloseAction::Ask,
            notifications: true,
            max_job_runs: 1,
            markdown: true,
            math: true,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct LocalSettings {
    /// "auto", "cuda", "mps" or "cpu".
    pub device: String,
}

impl Default for LocalSettings {
    fn default() -> Self {
        Self {
            device: "auto".into(),
        }
    }
}

/// How much the system log records.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum SystemLogLevel {
    Off,
    Errors,
    Warnings,
    #[default]
    Info,
    Debug,
}

impl SystemLogLevel {
    pub fn min(self) -> Option<chummy_log::system::Level> {
        use chummy_log::system::Level;
        match self {
            SystemLogLevel::Off => None,
            SystemLogLevel::Errors => Some(Level::Error),
            SystemLogLevel::Warnings => Some(Level::Warn),
            SystemLogLevel::Info => Some(Level::Info),
            SystemLogLevel::Debug => Some(Level::Debug),
        }
    }
}

/// How much each run's log records.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum RunLogLevel {
    /// No run logs.
    Off,
    Summary,
    Standard,
    #[default]
    Verbose,
}

impl RunLogLevel {
    pub fn detail(self) -> Option<chummy_log::runs::Detail> {
        use chummy_log::runs::Detail;
        match self {
            RunLogLevel::Off => None,
            RunLogLevel::Summary => Some(Detail::Summary),
            RunLogLevel::Standard => Some(Detail::Standard),
            RunLogLevel::Verbose => Some(Detail::Verbose),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Logging {
    pub runs: RunLogLevel,
    /// Encrypt run logs with the log key in the OS keychain.
    pub encrypt: bool,
    pub system: SystemLogLevel,
}

impl Default for Logging {
    fn default() -> Self {
        Self {
            runs: RunLogLevel::Verbose,
            encrypt: true,
            system: SystemLogLevel::Info,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Settings {
    pub version: u32,
    pub theme: Theme,
    pub permissions: Permissions,
    pub providers: Vec<ProviderConfig>,
    /// Id of the provider used unless an agent or block says otherwise.
    pub default_provider: Option<String>,
    pub agents: Vec<Agent>,
    /// The agent shown in the chat window.
    pub active_agent: Option<String>,
    pub local: LocalSettings,
    pub general: General,
    pub logging: Logging,
    /// Where tool calls run (Security and Privacy → Sandbox).
    pub sandbox: chummy_tools::SandboxSettings,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            version: VERSION,
            theme: Theme::System,
            permissions: Permissions::default(),
            providers: Vec::new(),
            default_provider: None,
            agents: Vec::new(),
            active_agent: None,
            local: LocalSettings::default(),
            general: General::default(),
            logging: Logging::default(),
            sandbox: chummy_tools::SandboxSettings::default(),
        }
    }
}

impl AgentConfig for Settings {
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

/// The shared rules (see [`AgentConfig`]), callable without importing it.
impl Settings {
    pub fn provider(&self, id: &str) -> Option<&ProviderConfig> {
        AgentConfig::provider(self, id)
    }

    pub fn agent(&self, id: &str) -> Option<&Agent> {
        AgentConfig::agent(self, id)
    }

    pub fn default_model(&self) -> Option<&ProviderConfig> {
        AgentConfig::default_model(self)
    }

    pub fn agent_model(&self, agent: &Agent) -> Option<&ProviderConfig> {
        AgentConfig::agent_model(self, agent)
    }

    /// The agent's model, if Chummy may use it for its own small jobs
    /// outside loops ("Use model within app", off by default). **Every model
    /// call that doesn't come from a loop must go through this** (see
    /// `crate::in_app`).
    pub fn in_app_model(&self, agent: &Agent) -> Option<&ProviderConfig> {
        self.agent_model(agent).filter(|p| p.use_in_app)
    }
}

/// Reads a settings document as it is: there are no migrations before 1.0
/// (see the module notes). Only the version is brought up to date.
pub fn migrate(mut v: serde_json::Value) -> serde_json::Value {
    if v.get("version").is_none() && v.is_object() {
        v["version"] = VERSION.into();
    }
    v
}

/// Writes what changed between two versions of the settings to the system
/// log: permissions, tools, agents and their access, and model connections.
/// Never secrets (they aren't in settings anyway) or folder contents.
pub fn log_changes(old: &Settings, new: &Settings) {
    use chummy_log::system::{Level, log};
    let on = |b: bool| if b { "on" } else { "off" };
    let mut changes: Vec<(Level, String)> = Vec::new();
    let (a, b) = (&old.permissions, &new.permissions);
    if a.network != b.network {
        changes.push((
            Level::Warn,
            format!("Internet access turned {}", on(b.network)),
        ));
    }
    if a.tools != b.tools {
        changes.push((Level::Warn, format!("Tools turned {}", on(b.tools))));
    }
    if a.confirm_online_tools != b.confirm_online_tools {
        changes.push((
            Level::Warn,
            format!(
                "Ask before tools go online turned {}",
                on(b.confirm_online_tools)
            ),
        ));
    }
    let (x, y) = (&old.sandbox, &new.sandbox);
    if x.kind != y.kind {
        changes.push((
            Level::Warn,
            format!("Tools now run in the {} sandbox", y.kind.label()),
        ));
    }
    if x.container.approved_id != y.container.approved_id
        && let Some(id) = &y.container.approved_id
    {
        changes.push((
            Level::Warn,
            format!("Container image {} approved ({id})", y.container.image),
        ));
    }
    if x.container.engine != y.container.engine || x.container.program != y.container.program {
        changes.push((
            Level::Warn,
            format!(
                "Container engine set to {}{}",
                y.container.engine.label(),
                if y.container.program.is_empty() {
                    String::new()
                } else {
                    format!(" at {}", y.container.program)
                }
            ),
        ));
    }
    if x.container.memory_mb != y.container.memory_mb || x.container.cpus != y.container.cpus {
        changes.push((
            Level::Info,
            format!(
                "Container limits set to {} MB and {} CPUs",
                y.container.memory_mb, y.container.cpus
            ),
        ));
    }
    if x.openshell != y.openshell {
        changes.push((
            Level::Warn,
            format!(
                "OpenShell set to {} with image {}",
                if y.openshell.program.is_empty() {
                    "the openshell program on this computer"
                } else {
                    &y.openshell.program
                },
                if y.openshell.image.is_empty() {
                    "(OpenShell's default)"
                } else {
                    &y.openshell.image
                }
            ),
        ));
    }
    for agent in &new.agents {
        match old.agents.iter().find(|a| a.id == agent.id) {
            None => changes.push((
                Level::Info,
                format!(
                    "Agent {} created, working in {}",
                    agent.name,
                    agent.workspace.path.display()
                ),
            )),
            Some(prev) => {
                if prev.workspace.path != agent.workspace.path {
                    changes.push((
                        Level::Warn,
                        format!(
                            "Agent {} now works in {}",
                            agent.name,
                            agent.workspace.path.display()
                        ),
                    ));
                }
                if prev.loop_id != agent.loop_id {
                    changes.push((
                        Level::Info,
                        format!("Agent {} now uses loop {}", agent.name, agent.loop_id),
                    ));
                }
                for change in tool_changes(&prev.tools, &agent.tools) {
                    changes.push((change.0, format!("Agent {}: {}", agent.name, change.1)));
                }
                if prev.model_options != agent.model_options {
                    changes.push((
                        Level::Info,
                        format!("Agent {}'s model settings changed", agent.name),
                    ));
                }
                if prev.loop_config != agent.loop_config {
                    changes.push((
                        Level::Info,
                        format!("Agent {}'s loop configuration changed", agent.name),
                    ));
                }
            }
        }
    }
    for agent in &old.agents {
        if !new.agents.iter().any(|a| a.id == agent.id) {
            changes.push((Level::Info, format!("Agent {} deleted", agent.name)));
        }
    }
    for p in &new.providers {
        match old.providers.iter().find(|x| x.id == p.id) {
            None => changes.push((
                Level::Info,
                format!("Model connection {} added ({})", p.name, p.model),
            )),
            Some(prev) if prev != p => {
                changes.push((Level::Info, format!("Model connection {} changed", p.name)))
            }
            _ => {}
        }
    }
    for p in &old.providers {
        if !new.providers.iter().any(|x| x.id == p.id) {
            changes.push((Level::Info, format!("Model connection {} removed", p.name)));
        }
    }
    let (a, b) = (&old.logging, &new.logging);
    if a.encrypt != b.encrypt {
        changes.push((
            Level::Warn,
            format!("Run log encryption turned {}", on(b.encrypt)),
        ));
    }
    if a.runs != b.runs {
        changes.push((
            Level::Info,
            format!("Run logs set to {:?}", b.runs).to_lowercase(),
        ));
    }
    if a.system != b.system {
        changes.push((
            Level::Warn,
            format!("System log set to {:?}", b.system).to_lowercase(),
        ));
    }
    for (level, message) in changes {
        log(level, "settings", message, &[]);
    }
}

/// What changed in an agent's tool switches, one line per change.
fn tool_changes(
    old: &BTreeMap<String, AgentTool>,
    new: &BTreeMap<String, AgentTool>,
) -> Vec<(chummy_log::system::Level, String)> {
    use chummy_log::system::Level;
    let on = |b: bool| if b { "on" } else { "off" };
    let names: std::collections::BTreeSet<&String> = old.keys().chain(new.keys()).collect();
    let mut out = Vec::new();
    for name in names {
        match (old.get(name), new.get(name)) {
            (None, Some(t)) => out.push((
                Level::Info,
                format!(
                    "tool {name} turned on{}{}",
                    if t.ask { ", asks first" } else { "" },
                    if t.network { ", with internet" } else { "" }
                ),
            )),
            (Some(_), None) => out.push((Level::Info, format!("tool {name} turned off"))),
            (Some(x), Some(y)) => {
                if x.network != y.network {
                    out.push((
                        Level::Warn,
                        format!("internet for tool {name} turned {}", on(y.network)),
                    ));
                }
                if x.ask != y.ask {
                    out.push((
                        Level::Info,
                        format!("ask before each use of {name} turned {}", on(y.ask)),
                    ));
                }
            }
            (None, None) => {}
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn changes_go_to_the_system_log() {
        let tmp = tempfile::tempdir().unwrap();
        let log = chummy_log::system::init(tmp.path());
        let old = Settings::default();
        let mut new = old.clone();
        new.permissions.network = true;
        let agent = |tools: BTreeMap<String, AgentTool>| Agent {
            id: "a".into(),
            name: "A".into(),
            tools,
            ..serde_json::from_value(serde_json::json!({
                "id": "a", "name": "A", "loop_id": "l",
                "workspace": { "path": "/tmp/w", "confirmed_at": 1 }, "created_at": 1
            }))
            .unwrap()
        };
        let mut old = old;
        old.agents.push(agent(BTreeMap::new()));
        new.agents.push(agent(
            [(
                "calculator".to_string(),
                AgentTool {
                    ask: true,
                    network: false,
                },
            )]
            .into(),
        ));
        log_changes(&old, &new);
        let said: Vec<String> = log.recent().into_iter().map(|e| e.message).collect();
        assert!(
            said.contains(&"Internet access turned on".to_string()),
            "{said:?}"
        );
        assert!(
            said.contains(&"Agent A: tool calculator turned on, asks first".to_string()),
            "{said:?}"
        );
    }

    #[test]
    fn empty_and_partial_documents_load_with_defaults() {
        let s: Settings = serde_json::from_value(migrate(serde_json::json!({}))).unwrap();
        assert_eq!(s.version, VERSION);
        assert!(!s.permissions.network && !s.permissions.tools);
        assert!(
            s.permissions.confirm_online_tools,
            "online tools ask by default"
        );
        assert_eq!(s.general.close, CloseAction::Ask);
        // Tools run in the built-in sandbox unless the user picks another,
        // and no container image is approved.
        assert_eq!(s.sandbox.kind, chummy_tools::SandboxKind::Native);
        assert!(s.sandbox.container.approved_id.is_none());
        // Files without the newer switches get the safe defaults.
        let s: Settings = serde_json::from_value(migrate(
            serde_json::json!({ "version": VERSION, "permissions": { "network": true } }),
        ))
        .unwrap();
        assert!(s.permissions.network && s.permissions.confirm_online_tools);
        assert_eq!(s.local.device, "auto");

        let s: Settings = serde_json::from_value(migrate(
            serde_json::json!({ "theme": "dark", "future_field": 1 }),
        ))
        .unwrap();
        assert_eq!(s.theme, Theme::Dark);
    }

    #[test]
    fn models_are_used_in_app_only_when_switched_on() {
        let s: Settings = serde_json::from_value(serde_json::json!({
            "version": VERSION,
            "providers": [{ "id": "m", "name": "M", "kind": "open_ai_compatible",
                            "base_url": "http://localhost:1/v1", "model": "x" }],
            "default_provider": "m",
            "agents": [{ "id": "a", "name": "A", "loop_id": "l", "model": null,
                         "workspace": { "path": "/tmp/w", "confirmed_at": 1 }, "created_at": 1 }]
        }))
        .unwrap();
        let agent = s.agent("a").unwrap().clone();
        // Off by default, and missing in older files means off.
        assert!(!s.providers[0].use_in_app);
        assert!(s.agent_model(&agent).is_some());
        assert!(s.in_app_model(&agent).is_none());
        let mut s = s;
        s.providers[0].use_in_app = true;
        assert!(s.in_app_model(&agent).is_some());
    }
}
