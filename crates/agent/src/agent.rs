//! What an agent is, and the switches that decide what it may do.
//!
//! These types are stored in the app's settings and, later, in chummyd's
//! bundle, so their JSON shape is part of both formats: every field has a
//! default, and a change of meaning needs a migration in the format that
//! holds them.

use std::collections::BTreeMap;
use std::path::PathBuf;

use serde::{Deserialize, Serialize};

/// The folder an agent is locked to.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Workspace {
    /// Canonical path, validated when chosen and again whenever it is opened.
    pub path: PathBuf,
    /// Unix time the user confirmed it.
    pub confirmed_at: u64,
}

/// An agent: a loop, a model, and a workspace folder. Agents may share a
/// folder (or have one inside another's) after the user confirmed a warning.
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
    /// The tools this agent may use, by name, each with its own switches.
    /// Each also needs Tools on in Security and Privacy; a loop block can
    /// only turn a tool off for itself.
    #[serde(default)]
    pub tools: BTreeMap<String, AgentTool>,
    /// Changed block settings, per loop id (Agent Loop Configuration).
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub loop_config: BTreeMap<String, chummy_flow::Overrides>,
    /// This agent's changes to its model's settings (temperature, longest
    /// answer, reasoning effort, sampling…); anything not set inherits the
    /// model connection's. Applies to every model call the agent makes; a
    /// model block's own setting still wins.
    #[serde(default, skip_serializing_if = "chummy_llm::ModelOverrides::is_empty")]
    pub model_options: chummy_llm::ModelOverrides,
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

/// How an agent may use one of its tools. Both switches are off until the
/// user turns them on.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct AgentTool {
    /// Ask the user before every call.
    pub ask: bool,
    /// It may use the internet (only for tools that ask for it; also needs
    /// Internet access in Security and Privacy).
    pub network: bool,
}
