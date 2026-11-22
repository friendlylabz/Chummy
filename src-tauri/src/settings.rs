//! The app's settings document (`settings.json` in the Chummy folder).
//!
//! Rules for changing this schema:
//! - every field has a default (`#[serde(default)]`), so older files load;
//! - renames or meaning changes bump [`VERSION`] and add a step to [`migrate`].

use std::path::PathBuf;

use chummy_llm::ProviderConfig;
use serde::{Deserialize, Serialize};

pub const VERSION: u32 = 2;

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
}

/// Everything beyond the workspace is off until the user turns it on.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Permissions {
    /// Agents and tools may use the internet.
    pub network: bool,
    /// Agents may run installed tools.
    pub tools: bool,
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

    /// The model an agent uses: its own choice if that still exists, else the default.
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
    v
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_and_partial_documents_load_with_defaults() {
        let s: Settings = serde_json::from_value(migrate(serde_json::json!({}))).unwrap();
        assert_eq!(s.version, VERSION);
        assert!(!s.permissions.network && !s.permissions.tools);
        assert_eq!(s.local.device, "auto");

        let s: Settings = serde_json::from_value(migrate(
            serde_json::json!({ "theme": "dark", "future_field": 1 }),
        ))
        .unwrap();
        assert_eq!(s.theme, Theme::Dark);
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
        assert_eq!((s.version, s.theme), (2, Theme::Light));
    }
}
