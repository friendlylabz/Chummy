//! The app's settings document (`settings.json` in the Chummy folder).
//!
//! Rules for changing this schema:
//! - every field has a default (`#[serde(default)]`), so older files load;
//! - renames or meaning changes bump [`VERSION`] and add a step to [`migrate`].

use std::collections::{BTreeMap, BTreeSet};
use std::path::PathBuf;

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

/// The folder an agent is locked to.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Workspace {
    /// Canonical path, validated when chosen and again whenever it is opened.
    pub path: PathBuf,
    /// Unix time the user confirmed it.
    pub confirmed_at: u64,
}

/// An agent: a loop, a model, and its own workspace folder. Each workspace
/// belongs to exactly one agent, and no two agents' workspaces overlap.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Agent {
    pub id: String,
    pub name: String,
    /// Loop id (file name in `loops/`).
    pub loop_id: String,
    /// Model connection id; `None` uses the default model.
    #[serde(default)]
    pub model: Option<String>,
    pub workspace: Workspace,
    pub created_at: u64,
    /// Tools this agent may use (each also needs Tools on in Permissions and
    /// the tool switched on in Settings → Tools).
    #[serde(default)]
    pub tools: BTreeSet<String>,
    /// This agent's tools may use the internet (also needs Internet access
    /// in Permissions and the tool's own internet switch).
    #[serde(default)]
    pub network: bool,
    /// Changed block settings, per loop id (Agent Loop Configuration).
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub loop_config: BTreeMap<String, chummy_flow::Overrides>,
}

impl Agent {
    /// This agent's changes to the loop `loop_id`.
    pub fn overrides(&self, loop_id: &str) -> chummy_flow::Overrides {
        self.loop_config.get(loop_id).cloned().unwrap_or_default()
    }
}

/// Everything beyond the workspace is off until the user turns it on.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Permissions {
    /// Agents and tools may use the internet.
    pub network: bool,
    /// Agents may run installed tools.
    pub tools: bool,
    /// Every call to a tool that can go online waits for the user's OK. On
    /// unless the user turns it off (which asks for confirmation).
    pub confirm_online_tools: bool,
}

impl Default for Permissions {
    fn default() -> Self {
        Self {
            network: false,
            tools: false,
            confirm_online_tools: true,
        }
    }
}

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

/// A tool's switches. Both are off until the user turns them on, and both
/// also need the matching permission (`tools`, `network`) to be on.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct ToolPref {
    /// Agents may call this tool.
    pub enabled: bool,
    /// This tool may use the internet (only for tools that ask for it).
    pub network: bool,
    /// Ask before every call to this tool.
    pub ask: bool,
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

impl Settings {
    pub fn provider(&self, id: &str) -> Option<&ProviderConfig> {
        self.providers.iter().find(|p| p.id == id)
    }

    pub fn agent(&self, id: &str) -> Option<&Agent> {
        self.agents.iter().find(|a| a.id == id)
    }

    /// The default model: the chosen default if it still exists, else the first.
    pub fn default_model(&self) -> Option<&ProviderConfig> {
        self.default_provider
            .as_deref()
            .and_then(|d| self.provider(d))
            .or_else(|| self.providers.first())
    }

    pub fn tool(&self, name: &str) -> ToolPref {
        self.tools.get(name).copied().unwrap_or_default()
    }

    /// The model an agent uses: its own choice if that still exists, else the default.
    /// The agent's model, if Chummy may use it for its own small jobs
    /// outside loops ("Use model within app", off by default). **Every model
    /// call that doesn't come from a loop must go through this** (see
    /// `crate::in_app`).
    pub fn in_app_model(&self, agent: &Agent) -> Option<&ProviderConfig> {
        self.agent_model(agent).filter(|p| p.use_in_app)
    }

    pub fn agent_model(&self, agent: &Agent) -> Option<&ProviderConfig> {
        agent
            .model
            .as_deref()
            .and_then(|m| self.provider(m))
            .or_else(|| self.default_model())
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
    fn v1_workspace_is_migrated_away() {
        let v1 = serde_json::json!({
            "version": 1, "theme": "light",
            "workspace": { "path": "/tmp/x", "confirmed_at": 1 }
        });
        let v = migrate(v1);
        assert!(v.get("workspace").is_none());
        let s: Settings = serde_json::from_value(v).unwrap();
        assert_eq!((s.version, s.theme), (VERSION, Theme::Light));
    }

    #[test]
    fn v2_agents_keep_their_tools() {
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
