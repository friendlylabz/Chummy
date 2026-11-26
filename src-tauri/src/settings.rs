//! The app's settings document (`settings.json` in the Chummy folder).
//!
//! Rules for changing this schema:
//! - every field has a default (`#[serde(default)]`), so older files load;
//! - renames or meaning changes bump [`VERSION`] and add a step to [`migrate`].

use std::collections::BTreeMap;

use chummy_llm::ProviderConfig;
use serde::{Deserialize, Serialize};

pub const VERSION: u32 = 3;

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Theme {
    #[default]
    System,
    Dark,
    Light,
}

pub use chummy_agent::{Agent, AgentConfig, Permissions, ToolPref, Workspace};

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
    /// Per-tool switches, by tool name.
    pub tools: BTreeMap<String, ToolPref>,
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
            tools: BTreeMap::new(),
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
    fn tool_prefs(&self) -> &BTreeMap<String, ToolPref> {
        &self.tools
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

    pub fn tool(&self, name: &str) -> ToolPref {
        AgentConfig::tool(self, name)
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

/// Upgrades older documents step by step.
pub fn migrate(mut v: serde_json::Value) -> serde_json::Value {
    let version = v["version"].as_u64().unwrap_or(0);
    if version < 1 {
        // 0 → 1: first versioned schema; nothing to move.
        v["version"] = 1.into();
    }
    if version < 2 {
        // 1 → 2: the single global workspace became one workspace per agent.
        // There were no agents in v1, so the old folder is dropped; it is
        // chosen again when the first agent is set up.
        if let Some(o) = v.as_object_mut() {
            o.remove("workspace");
        }
        v["version"] = 2.into();
    }
    if version < 3 {
        // 2 → 3: tools and internet became per agent too. Existing agents keep
        // what they could do: every tool switched on, and internet access if
        // it was on.
        let tools: Vec<serde_json::Value> = v["tools"]
            .as_object()
            .map(|t| {
                t.iter()
                    .filter(|(_, p)| p["enabled"].as_bool() == Some(true))
                    .map(|(n, _)| n.clone().into())
                    .collect()
            })
            .unwrap_or_default();
        let network = v["permissions"]["network"].as_bool().unwrap_or(false);
        if let Some(agents) = v.get_mut("agents").and_then(|a| a.as_array_mut()) {
            for a in agents {
                if let Some(o) = a.as_object_mut() {
                    o.entry("tools").or_insert_with(|| tools.clone().into());
                    o.entry("network").or_insert(network.into());
                }
            }
        }
        v["version"] = 3.into();
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
    let names: std::collections::BTreeSet<&String> =
        old.tools.keys().chain(new.tools.keys()).collect();
    for name in names {
        let (x, y) = (
            old.tools.get(name).cloned().unwrap_or_default(),
            new.tools.get(name).cloned().unwrap_or_default(),
        );
        if x.enabled != y.enabled {
            changes.push((Level::Info, format!("Tool {name} turned {}", on(y.enabled))));
        }
        if x.network != y.network {
            changes.push((
                Level::Warn,
                format!("Internet for tool {name} turned {}", on(y.network)),
            ));
        }
        if x.ask != y.ask {
            changes.push((
                Level::Info,
                format!("Ask before each use of {name} turned {}", on(y.ask)),
            ));
        }
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
                if prev.tools != agent.tools {
                    let list: Vec<&str> = agent.tools.iter().map(|t| t.as_str()).collect();
                    changes.push((
                        Level::Info,
                        format!(
                            "Agent {} may use: {}",
                            agent.name,
                            if list.is_empty() {
                                "no tools".into()
                            } else {
                                list.join(", ")
                            }
                        ),
                    ));
                }
                if prev.network != agent.network {
                    changes.push((
                        Level::Warn,
                        format!(
                            "Internet for agent {} turned {}",
                            agent.name,
                            on(agent.network)
                        ),
                    ));
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
        new.tools.insert(
            "calculator".into(),
            ToolPref {
                enabled: true,
                ..Default::default()
            },
        );
        log_changes(&old, &new);
        let said: Vec<String> = log.recent().into_iter().map(|e| e.message).collect();
        assert!(
            said.contains(&"Internet access turned on".to_string()),
            "{said:?}"
        );
        assert!(
            said.contains(&"Tool calculator turned on".to_string()),
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
        // Older files without the new switch get the safe default.
        let s: Settings = serde_json::from_value(migrate(
            serde_json::json!({ "version": 2, "permissions": { "network": true } }),
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

    #[test]
    fn older_settings_are_migrated() {
        let v1 = serde_json::json!({
            "version": 1, "theme": "light",
            "workspace": { "path": "/tmp/x", "confirmed_at": 1 }
        });
        let v = migrate(v1);
        assert!(v.get("workspace").is_none());
        let s: Settings = serde_json::from_value(v).unwrap();
        assert_eq!((s.version, s.theme), (VERSION, Theme::Light));

        // (was v2_agents_keep_their_tools)
        {
            let v2 = serde_json::json!({
                "version": 2,
                "permissions": { "network": true, "tools": true },
                "tools": { "calculator": { "enabled": true }, "fetch_page": { "enabled": false, "network": true } },
                "agents": [{ "id": "a", "name": "A", "loop_id": "l",
                             "workspace": { "path": "/tmp/a", "confirmed_at": 1 }, "created_at": 1 }]
            });
            let s: Settings = serde_json::from_value(migrate(v2)).unwrap();
            let a = &s.agents[0];
            assert_eq!(a.tools.iter().collect::<Vec<_>>(), ["calculator"]);
            assert!(a.network);
        }
    }
}
