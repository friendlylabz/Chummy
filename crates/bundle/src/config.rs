//! What a bundle holds: chummyd's whole configuration (`chummyd.config`
//! v1), and the checks it must pass before it's sealed or run.

use std::collections::{BTreeMap, BTreeSet};

use base64::Engine as _;
use base64::engine::general_purpose::STANDARD as B64;
use chummy_agent::{AgentTool, Permissions};
use chummy_flow::{LoopFile, Severity, analyze};
use chummy_llm::{ProviderConfig, ProviderKind};
use chummy_scheduler::Job;
use serde::{Deserialize, Serialize};

pub const CONFIG_FORMAT: &str = "chummyd.config";
/// The version new bundles are written as. Version 1 (before the Matrix
/// channel) still opens.
pub const CONFIG_VERSION: u32 = 3;
/// Most job runs going at once.
pub const MAX_RUNS: u32 = 8;
/// The most messages a chat can keep (`limits.chat_messages`).
pub const MAX_CHAT_MESSAGES: u32 = 100_000;
/// The most chats an agent can keep (`limits.chats`).
pub const MAX_CHATS: u32 = 10_000;

/// chummyd's configuration. It can't change while chummyd runs: changing it
/// means making a new bundle and starting again.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Config {
    pub format: String,
    pub version: u32,
    pub instance: Instance,
    #[serde(default)]
    pub limits: Limits,
    /// The switches every agent's tools also need (as in the app).
    #[serde(default)]
    pub permissions: Permissions,
    #[serde(default)]
    pub providers: Vec<ProviderConfig>,
    #[serde(default)]
    pub default_provider: Option<String>,
    /// Secrets by name (`provider:<id>` for a model connection's key).
    #[serde(default)]
    pub secrets: BTreeMap<String, String>,
    pub agents: Vec<BundleAgent>,
    /// The loops the agents and jobs run, by id. They're trusted: the
    /// bundle can't be changed without its password.
    #[serde(default)]
    pub loops: BTreeMap<String, LoopFile>,
    #[serde(default)]
    pub jobs: Vec<Job>,
    #[serde(default)]
    pub channels: Channels,
    #[serde(default)]
    pub logging: Logging,
    /// The key chummyd's state folder is sealed with (32 bytes, base64). It
    /// stays the same across new bundles of the same instance.
    pub state_key: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Instance {
    /// Random; ties the state folder to this instance.
    pub id: String,
    pub name: String,
    /// IANA time zone for job schedules and tools (`TZ`).
    pub time_zone: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields, default)]
pub struct Limits {
    /// Most job runs going at once (1 to [`MAX_RUNS`]).
    pub max_runs: u32,
    /// Messages each chat keeps; older ones are dropped (10 to
    /// [`MAX_CHAT_MESSAGES`]). The app keeps 2,000.
    pub chat_messages: u32,
    /// Chats each agent keeps; the least recently used go first (1 to
    /// [`MAX_CHATS`]).
    pub chats: u32,
}

impl Default for Limits {
    fn default() -> Self {
        Self {
            max_runs: 1,
            chat_messages: 2000,
            chats: 50,
        }
    }
}

/// An agent in chummyd. Its folder is `<jail dir>/<id>/`, chosen when
/// chummyd starts.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BundleAgent {
    pub id: String,
    pub name: String,
    pub loop_id: String,
    #[serde(default)]
    pub model: Option<String>,
    /// Its tools, each with its own switches (as in the app).
    #[serde(default)]
    pub tools: BTreeMap<String, AgentTool>,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub loop_config: BTreeMap<String, chummy_flow::Overrides>,
    #[serde(default, skip_serializing_if = "chummy_llm::ModelOverrides::is_empty")]
    pub model_options: chummy_llm::ModelOverrides,
    /// Tool approvals and daily-limit questions may be answered over a
    /// channel. Off: they're refused there.
    #[serde(default)]
    pub channel_approvals: bool,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields, default)]
pub struct Channels {
    pub terminal: TerminalChannel,
    /// Talking to agents in Matrix (end-to-end encrypted rooms).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub matrix: Option<MatrixChannel>,
}

impl Channels {
    pub fn any(&self) -> bool {
        self.terminal.enabled || self.matrix.is_some()
    }
}

/// Most a file sent over a channel may be, in MB.
pub const MAX_UPLOAD_MB: u32 = 1024;

/// The Matrix channel: chummyd logs in as its own account (the bot) and
/// makes one encrypted room for each agent and each person allowed to talk
/// to it.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MatrixChannel {
    /// The homeserver's address (https, or http only on this computer or
    /// the local network).
    pub homeserver: String,
    /// The bot's Matrix ID (`@name:server`).
    pub user_id: String,
    /// The bot's password: to log in the first time, and to set up its
    /// cross-signing.
    pub password: String,
    /// Who may talk to each agent, by agent id (Matrix IDs).
    #[serde(default)]
    pub users: BTreeMap<String, Vec<String>>,
    /// Who each job reports to, by job id (one of its agent's users).
    #[serde(default)]
    pub report_to: BTreeMap<String, String>,
    /// Largest file accepted, in MB.
    #[serde(default = "default_upload_mb")]
    pub upload_mb: u32,
}

fn default_upload_mb() -> u32 {
    100
}

/// A Matrix ID (`@localpart:server`): its rough shape, checked before it's
/// used anywhere.
pub fn valid_matrix_id(id: &str) -> bool {
    id.len() <= 255
        && id.starts_with('@')
        && id[1..].split_once(':').is_some_and(|(local, server)| {
            !local.is_empty()
                && !server.is_empty()
                && local
                    .chars()
                    .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || "._=-/+".contains(c))
                && server
                    .chars()
                    .all(|c| c.is_ascii_alphanumeric() || ".-:[]".contains(c))
        })
}

/// Talking to agents in the terminal chummyd runs in.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields, default)]
pub struct TerminalChannel {
    pub enabled: bool,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields, default)]
pub struct Logging {
    /// Logs may include message text (off: only ids, statuses and counts).
    pub message_text: bool,
}

/// What the checks found.
#[derive(Debug, Clone, Default, PartialEq, Serialize)]
pub struct Report {
    /// The bundle can't be made or run until these are fixed.
    pub errors: Vec<String>,
    /// Worth knowing, but allowed.
    pub warnings: Vec<String>,
}

impl Report {
    pub fn ok(&self) -> bool {
        self.errors.is_empty()
    }
}

/// Ids name folders and files, so they're kept to a safe alphabet.
pub fn valid_id(id: &str) -> bool {
    chummy_agent::valid_id(id)
}

impl Config {
    /// An empty configuration for a new instance, with a fresh id and state
    /// key.
    pub fn new_instance(name: &str, time_zone: &str) -> Result<Self, String> {
        Ok(Self {
            format: CONFIG_FORMAT.into(),
            version: CONFIG_VERSION,
            instance: Instance {
                id: format!("i-{}", hex(&crate::random::<8>()?)),
                name: name.into(),
                time_zone: time_zone.into(),
            },
            limits: Limits::default(),
            permissions: Permissions::default(),
            providers: vec![],
            default_provider: None,
            secrets: BTreeMap::new(),
            agents: vec![],
            loops: BTreeMap::new(),
            jobs: vec![],
            channels: Channels::default(),
            logging: Logging::default(),
            state_key: B64.encode(crate::random::<32>()?),
        })
    }

    /// The state key's bytes, if it's a proper key.
    pub fn state_key_bytes(&self) -> Option<zeroize::Zeroizing<[u8; 32]>> {
        let raw = zeroize::Zeroizing::new(B64.decode(self.state_key.trim()).ok()?);
        let key: [u8; 32] = raw.as_slice().try_into().ok()?;
        Some(zeroize::Zeroizing::new(key))
    }

    pub fn agent(&self, id: &str) -> Option<&BundleAgent> {
        self.agents.iter().find(|a| a.id == id)
    }

    /// Everything chummyd checks before it runs (and the app before it
    /// seals): errors stop it, warnings are worth knowing.
    pub fn check(&self) -> Report {
        let mut r = Report::default();
        let mut err = |s: String| r.errors.push(s);
        if self.format != CONFIG_FORMAT || self.version != CONFIG_VERSION {
            err("This configuration is from a newer or different version of Chummy.".into());
        }
        if !valid_id(&self.instance.id) {
            err("The instance's id isn't valid.".into());
        }
        let name = self.instance.name.trim();
        if name.is_empty() || name.chars().count() > 80 {
            err("Give the instance a name (up to 80 characters).".into());
        }
        if jiff::tz::TimeZone::get(&self.instance.time_zone).is_err() {
            err(format!(
                "“{}” isn't a time zone chummyd knows.",
                self.instance.time_zone
            ));
        }
        if !(1..=MAX_RUNS).contains(&self.limits.max_runs) {
            err(format!(
                "Job runs at once must be between 1 and {MAX_RUNS}."
            ));
        }
        if !(10..=MAX_CHAT_MESSAGES).contains(&self.limits.chat_messages) {
            err(format!(
                "Messages kept per chat must be between 10 and {MAX_CHAT_MESSAGES}."
            ));
        }
        if !(1..=MAX_CHATS).contains(&self.limits.chats) {
            err(format!(
                "Chats kept per agent must be between 1 and {MAX_CHATS}."
            ));
        }
        if self.state_key_bytes().is_none() {
            err("The state key isn't valid.".into());
        }

        // Model connections.
        let mut seen = BTreeSet::new();
        for p in &self.providers {
            if !seen.insert(p.id.as_str()) {
                err(format!("Two model connections have the id “{}”.", p.id));
            }
            if p.kind == ProviderKind::Local {
                err(format!(
                    "“{}” is Chummy's built-in engine, which chummyd doesn't have. Connect Ollama or another server instead.",
                    p.name
                ));
            } else if let Some(problem) = chummy_llm::endpoint_problem(&p.base_url) {
                err(format!("“{}”: {problem}", p.name));
            }
        }
        if let Some(d) = &self.default_provider
            && !seen.contains(d.as_str())
        {
            err("The default model connection isn't in the bundle.".into());
        }
        for name in self.secrets.keys() {
            if name.trim().is_empty() || name.len() > 200 {
                err("A secret's name isn't valid.".into());
            }
        }

        // Agents.
        if self.agents.is_empty() {
            err("Include at least one agent.".into());
        }
        let mut ids = BTreeSet::new();
        for a in &self.agents {
            if !valid_id(&a.id) {
                err(format!("The agent id “{}” isn't valid.", a.id));
            }
            if !ids.insert(a.id.as_str()) {
                err(format!("Two agents have the id “{}”.", a.id));
            }
            if a.name.trim().is_empty() {
                err(format!("The agent “{}” has no name.", a.id));
            }
            match self.loops.get(&a.loop_id) {
                None => err(format!("“{}”'s loop isn't in the bundle.", a.name)),
                Some(l) => {
                    let (file, _) = l.with_overrides(
                        &a.loop_config.get(&a.loop_id).cloned().unwrap_or_default(),
                    );
                    if let Some(e) = first_error(&file) {
                        err(format!(
                            "“{}”'s loop “{}” has a problem: {e}",
                            a.name, l.name
                        ));
                    }
                }
            }
            match &a.model {
                Some(m) if !seen.contains(m.as_str()) => err(format!(
                    "“{}”'s model connection isn't in the bundle.",
                    a.name
                )),
                None if self.providers.is_empty() => {
                    err(format!("“{}” has no model connection.", a.name))
                }
                _ => {}
            }
        }
        for (id, l) in &self.loops {
            if !valid_id(id) {
                err(format!("The loop id “{id}” isn't valid."));
            }
            if let Some(e) = first_error(l) {
                err(format!("The loop “{}” has a problem: {e}", l.name));
            }
        }

        // Jobs.
        let mut jobs = BTreeSet::new();
        for j in &self.jobs {
            if let Err(e) = j.check() {
                err(format!("The job “{}”: {e}", j.name));
            }
            if !jobs.insert(j.id.as_str()) {
                err(format!("Two jobs have the id “{}”.", j.id));
            }
            if !ids.contains(j.agent.as_str()) {
                err(format!(
                    "The job “{}” runs as an agent that isn't in the bundle.",
                    j.name
                ));
            }
            if let Some(l) = &j.loop_id
                && !self.loops.contains_key(l)
            {
                err(format!("The job “{}”'s loop isn't in the bundle.", j.name));
            }
        }

        // The Matrix channel.
        let mut matrix_warnings = vec![];
        if let Some(m) = &self.channels.matrix {
            match chummy_llm::endpoint_problem(&m.homeserver) {
                Some(p) => err(format!("The Matrix homeserver: {p}")),
                None if chummy_llm::unencrypted_remote(&m.homeserver) => err(
                    "The Matrix homeserver must use https (plain http only on this computer or your local network)."
                        .into(),
                ),
                None => {}
            }
            if !valid_matrix_id(&m.user_id) {
                err("chummyd's Matrix ID isn't valid (it looks like @name:server).".into());
            }
            if m.password.is_empty() {
                err("Enter chummyd's Matrix password.".into());
            }
            if !(1..=MAX_UPLOAD_MB).contains(&m.upload_mb) {
                err(format!(
                    "The largest upload must be between 1 and {MAX_UPLOAD_MB} MB."
                ));
            }
            for (agent, users) in &m.users {
                if !ids.contains(agent.as_str()) {
                    err("People are allowed to talk to an agent that isn't in the bundle.".into());
                }
                for u in users {
                    if !valid_matrix_id(u) {
                        err(format!(
                            "“{u}” isn't a valid Matrix ID (it looks like @name:server)."
                        ));
                    } else if *u == m.user_id {
                        err("chummyd's own Matrix ID can't be one of the people allowed to talk to agents.".into());
                    }
                }
            }
            for a in &self.agents {
                if m.users.get(&a.id).is_none_or(|u| u.is_empty()) {
                    matrix_warnings.push(format!("Nobody may talk to “{}” in Matrix.", a.name));
                }
            }
            for (job, user) in &m.report_to {
                let Some(j) = self.jobs.iter().find(|j| j.id == *job) else {
                    err("A job that isn't in the bundle reports to someone.".into());
                    continue;
                };
                if !m.users.get(&j.agent).is_some_and(|u| u.contains(user)) {
                    err(format!(
                        "The job “{}” reports to {user}, who isn't allowed to talk to its agent.",
                        j.name
                    ));
                }
            }
            for j in &self.jobs {
                if !m.report_to.contains_key(&j.id) {
                    matrix_warnings.push(format!(
                        "The job “{}” doesn't report to anyone in Matrix.",
                        j.name
                    ));
                }
            }
        }

        // Worth knowing.
        let warn = &mut r.warnings;
        warn.extend(matrix_warnings);
        if !self.channels.any() {
            warn.push("No channel is on, so nobody can talk to the agents: only jobs run.".into());
        }
        for p in &self.providers {
            let needs_key = matches!(
                p.kind,
                ProviderKind::OpenAi | ProviderKind::Anthropic | ProviderKind::Gemini
            );
            if needs_key && !self.secrets.contains_key(&p.secret_key()) {
                warn.push(format!("“{}” has no API key in the bundle.", p.name));
            }
        }
        for a in &self.agents {
            if a.channel_approvals || !self.permissions.tools || a.tools.is_empty() {
                continue;
            }
            let asks = a.tools.values().any(|t| {
                t.ask
                    || (self.permissions.confirm_online_tools
                        && self.permissions.network
                        && t.network)
            });
            if asks {
                warn.push(format!(
                    "Some of “{}”'s tools ask before they're used, and approvals aren't allowed over channels for it: those calls will always be refused.",
                    a.name
                ));
            }
        }
        r
    }
}

fn first_error(l: &LoopFile) -> Option<String> {
    analyze(l)
        .issues
        .into_iter()
        .find(|i| i.severity == Severity::Error)
        .map(|i| i.message)
}

pub(crate) fn hex(bytes: &[u8]) -> String {
    use std::fmt::Write as _;
    bytes.iter().fold(String::new(), |mut s, b| {
        let _ = write!(s, "{b:02x}");
        s
    })
}
