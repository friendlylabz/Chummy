//! # chummy-runner
//!
//! How an agent's loop actually runs: [`AgentHost`] connects the loop engine
//! to models, to sandboxed tools, to the agent's folder (through its jail)
//! and to the user (through the [`AskBroker`]). [`run_job`] runs one
//! scheduled job with it.
//!
//! The Chummy app and chummyd both use this crate. What differs between
//! them (where secrets live, how the tool sandbox and the built-in engine
//! are started) is behind [`Services`].
//!
//! Permissions are applied here, on every call, from an [`AgentSetup`] that
//! the host builds from its own settings. Nothing a loop or a job says can
//! widen them.

use std::collections::{BTreeMap, HashMap};
use std::future::Future;
use std::sync::Arc;

use chummy_flow::{
    Answer, Ask, Event, ExMessage, Host, LlmAnswer, LlmCall, LoopFile, MalformedCall, Role,
    RunInputs, RunOutcome, RunStatus, ToolInfo, ToolOutput, run,
};
use chummy_llm::{ChatEvent, ChatRequest, Client, Message, ProviderConfig, ProviderKind, ToolSpec};
use chummy_sandbox::Jail;
use chummy_scheduler::{AskBroker, Cause, Origin, RecordStatus, RunRequest, RunResult, Snapshot};
use chummy_tools::{NetPolicy, ToolHost, ToolManifest};
use jiff::tz::TimeZone;

/// Largest file a Read file block reads.
pub const READ_LIMIT: u64 = 1024 * 1024;
/// Largest file a job includes in its message.
pub const JOB_FILE_LIMIT: u64 = 256 * 1024;

/// What the host provides.
pub trait Services: Send + Sync + 'static {
    /// A stored secret, e.g. a model connection's API key.
    fn secret(&self, key: &str) -> Option<String>;
    /// The tool sandbox (set up and ready).
    fn tool_host(&self) -> impl Future<Output = Result<Arc<ToolHost>, String>> + Send;
    /// Starts the built-in engine for `p`; returns its address and token.
    fn local_engine(
        &self,
        p: &ProviderConfig,
    ) -> impl Future<Output = Result<(String, String), String>> + Send;
}

/// The switches that decide what an agent may do. All default to off.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Permissions {
    /// Agents may run tools at all.
    pub tools: bool,
    /// Tools may use the internet at all.
    pub network: bool,
    /// Every call to a tool that can go online waits for the user's OK.
    pub confirm_online_tools: bool,
}

/// One tool's switches.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct ToolGrant {
    pub enabled: bool,
    pub network: bool,
    /// Ask the user before every call.
    pub ask: bool,
}

/// Everything one run of an agent may use.
pub struct AgentSetup {
    /// Every model connection (blocks may pick one).
    pub providers: Vec<ProviderConfig>,
    /// The agent's model.
    pub model: Option<ProviderConfig>,
    pub permissions: Permissions,
    pub tool_grants: BTreeMap<String, ToolGrant>,
    /// Tools installed now.
    pub installed: Vec<ToolManifest>,
    /// The agent's folder; `None` for editor test runs without an agent.
    pub jail: Option<Jail>,
    /// Who to ask, and how the questions are labelled. `None`: this run
    /// can't ask (Ask me blocks and approvals stop it).
    pub asks: Option<(AskBroker, Origin)>,
    /// IANA zone name handed to tools as `TZ`.
    pub time_zone: Option<String>,
}

/// Connects the engine to the world for one run.
pub struct AgentHost<S: Services> {
    services: Arc<S>,
    providers: Vec<ProviderConfig>,
    model: Option<ProviderConfig>,
    permissions: Permissions,
    grants: BTreeMap<String, ToolGrant>,
    /// Tools this run may call: installed, switched on, tools allowed.
    tools: HashMap<String, ToolManifest>,
    jail: Option<Jail>,
    asks: Option<(AskBroker, Origin)>,
    time_zone: Option<String>,
    clients: tokio::sync::Mutex<HashMap<String, Client>>,
}

/// Upper-cases the first letter, for errors shown as sentences.
fn sentence(s: String) -> String {
    let mut c = s.chars();
    match c.next() {
        Some(f) => f.to_uppercase().chain(c).collect(),
        None => s,
    }
}

pub fn to_llm_message(m: &ExMessage) -> Message {
    let mut out = match m.role {
        Role::System => Message::system(&m.text),
        Role::User => Message::user(&m.text),
        Role::Assistant => Message::assistant(&m.text),
        Role::Tool => {
            let call = chummy_llm::ToolCall {
                id: m.call_id.clone().unwrap_or_default(),
                name: m.name.clone().unwrap_or_default(),
                arguments: serde_json::json!({}),
            };
            Message::tool_result(&call, &m.text)
        }
    };
    out.tool_calls = m
        .calls
        .iter()
        .map(|c| chummy_llm::ToolCall {
            id: c.id.clone(),
            name: c.name.clone(),
            arguments: c.arguments.clone(),
        })
        .collect();
    out
}

impl<S: Services> AgentHost<S> {
    pub fn new(services: Arc<S>, setup: AgentSetup) -> Self {
        let tools = if setup.permissions.tools {
            setup
                .installed
                .into_iter()
                .filter(|m| setup.tool_grants.get(&m.name).is_some_and(|g| g.enabled))
                .map(|m| (m.name.clone(), m))
                .collect()
        } else {
            HashMap::new()
        };
        Self {
            services,
            providers: setup.providers,
            model: setup.model,
            permissions: setup.permissions,
            grants: setup.tool_grants,
            tools,
            jail: setup.jail,
            asks: setup.asks,
            time_zone: setup.time_zone,
            clients: Default::default(),
        }
    }

    pub fn jail(&self) -> Result<&Jail, String> {
        self.jail
            .as_ref()
            .ok_or_else(|| "This test run has no workspace. Pick an agent to run as.".to_string())
    }

    fn grant(&self, name: &str) -> ToolGrant {
        self.grants.get(name).copied().unwrap_or_default()
    }

    /// Whether `m` may use the internet now, and if not, why (for the model).
    pub fn net_policy(&self, m: &ToolManifest) -> NetPolicy {
        if !m.network {
            NetPolicy::off(format!("{} doesn't use the internet.", m.title))
        } else if !self.permissions.network {
            NetPolicy::off("Internet access is off in Chummy, so this tool can't go online.")
        } else if !self.grant(&m.name).network {
            NetPolicy::off(format!(
                "Internet access is off for “{}”, so it can't go online.",
                m.title
            ))
        } else {
            NetPolicy::on()
        }
    }

    async fn client(&self, p: &ProviderConfig) -> Result<Client, String> {
        if let Some(c) = self.clients.lock().await.get(&p.id) {
            return Ok(c.clone());
        }
        let client = if p.kind == ProviderKind::Local {
            let (base_url, token) = self.services.local_engine(p).await?;
            let mut cfg = p.clone();
            cfg.base_url = base_url;
            Client::new(&cfg, Some(token)).map_err(|e| e.to_string())?
        } else {
            Client::new(p, self.services.secret(&p.secret_key())).map_err(|e| e.to_string())?
        };
        self.clients
            .lock()
            .await
            .insert(p.id.clone(), client.clone());
        Ok(client)
    }
}

impl<S: Services> Host for AgentHost<S> {
    async fn llm(
        &self,
        call: LlmCall,
        on_text: &mut (dyn FnMut(&str) + Send),
    ) -> Result<LlmAnswer, String> {
        let provider = match &call.model {
            Some(id) => self.providers.iter().find(|p| &p.id == id).cloned().ok_or(
                "This block's model connection no longer exists. Pick another in the loop editor.",
            )?,
            None => self
                .model
                .clone()
                .ok_or("No model is connected. Add one in Settings → Models.")?,
        };
        let client = self.client(&provider).await?;
        let req = ChatRequest {
            messages: call.messages.iter().map(to_llm_message).collect(),
            tools: call
                .tools
                .iter()
                .map(|t| ToolSpec {
                    name: t.name.clone(),
                    description: t.description.clone(),
                    parameters: t.parameters.clone(),
                })
                .collect(),
            tool_choice: match call.tool_choice {
                chummy_flow::ToolChoice::Auto => chummy_llm::ToolChoice::Auto,
                chummy_flow::ToolChoice::Required => chummy_llm::ToolChoice::Required,
            },
            temperature: call.temperature,
            max_tokens: Some(call.max_tokens),
        };
        let mut malformed = Vec::new();
        let resp = client
            .chat(&req, |ev| match ev {
                ChatEvent::Text { text } => on_text(text),
                ChatEvent::MalformedToolCall {
                    name, raw, error, ..
                } => malformed.push(MalformedCall {
                    name: name.clone(),
                    raw: raw.clone(),
                    error: error.clone(),
                }),
                _ => {}
            })
            .await
            .map_err(|e| format!("{}: {e}", provider.name))?;
        Ok(LlmAnswer {
            text: resp.text,
            reasoning: resp.reasoning,
            calls: resp
                .tool_calls
                .into_iter()
                .map(|c| chummy_flow::ToolCall {
                    id: c.id,
                    name: c.name,
                    arguments: c.arguments,
                })
                .collect(),
            malformed,
        })
    }

    fn tool_info(&self, name: &str) -> Option<ToolInfo> {
        self.tools.get(name).map(|m| ToolInfo {
            name: m.name.clone(),
            description: m.description.clone(),
            parameters: m.parameters.clone(),
        })
    }

    async fn run_tool(
        &self,
        name: &str,
        arguments: serde_json::Value,
    ) -> Result<ToolOutput, String> {
        let Some(m) = self.tools.get(name) else {
            return Ok(ToolOutput {
                text: format!("Error: there's no tool called “{name}” switched on."),
                is_error: true,
            });
        };
        let host = self.services.tool_host().await?;
        let env = self
            .time_zone
            .iter()
            .map(|tz| ("TZ".to_string(), tz.clone()))
            .collect();
        let workspace = self.jail.as_ref().map(|j| j.root().to_path_buf());
        let r = host
            .run(
                name,
                arguments,
                workspace.as_deref(),
                self.net_policy(m),
                env,
            )
            .await
            .map_err(|e| sentence(e.to_string()))?;
        Ok(ToolOutput {
            text: r.text,
            is_error: r.is_error,
        })
    }

    fn read_file(&self, path: &str) -> Result<String, String> {
        self.jail()?
            .read_text(path, READ_LIMIT)
            .map_err(|e| e.to_string())
    }

    fn write_file(&self, path: &str, content: &str, append: bool) -> Result<(), String> {
        let jail = self.jail()?;
        let data = if append && jail.exists(path).map_err(|e| e.to_string())? {
            let mut existing = jail
                .read_bytes(path, 64 * READ_LIMIT)
                .map_err(|e| e.to_string())?;
            existing.extend_from_slice(content.as_bytes());
            existing
        } else {
            content.as_bytes().to_vec()
        };
        jail.write(path, &data).map_err(|e| e.to_string())
    }

    fn list_files(&self, path: &str) -> Result<Vec<String>, String> {
        Ok(self
            .jail()?
            .list(path)
            .map_err(|e| e.to_string())?
            .into_iter()
            .map(|e| {
                if e.is_dir {
                    format!("{}/", e.name)
                } else {
                    e.name
                }
            })
            .collect())
    }

    async fn ask(&self, ask: Ask) -> Result<Answer, String> {
        match &self.asks {
            Some((broker, origin)) => broker.ask(origin.clone(), ask).await,
            None => Err("This run can't ask you anything.".into()),
        }
    }

    fn must_confirm(&self, tool: &str) -> bool {
        let Some(m) = self.tools.get(tool) else {
            return false;
        };
        self.grant(tool).ask
            || (self.permissions.confirm_online_tools && self.net_policy(m).allowed)
    }
}

/// The files directly in `folder` of a jail, for folder triggers.
pub fn snapshot(jail: &Jail, folder: &str) -> Result<Snapshot, String> {
    let entries = jail.list(folder).map_err(|e| e.to_string())?;
    Ok(entries
        .into_iter()
        .filter(|e| e.is_file && !chummy_scheduler::watch::ignored(&e.name))
        .take(chummy_scheduler::watch::MAX_FILES)
        .map(|e| {
            (
                e.name,
                chummy_scheduler::FileStamp {
                    size: e.size,
                    modified_ms: e.modified_ms,
                },
            )
        })
        .collect())
}

/// How a finished loop run is recorded.
pub fn result_of(o: &RunOutcome) -> RunResult {
    let (status, note) = match &o.status {
        RunStatus::Completed if o.replies.is_empty() => (
            RecordStatus::Completed,
            "The loop finished without replying.".to_string(),
        ),
        RunStatus::Completed => (RecordStatus::Completed, String::new()),
        RunStatus::Stopped => (RecordStatus::Stopped, "Stopped.".into()),
        RunStatus::LimitReached { message } => (RecordStatus::Limit, message.clone()),
        RunStatus::Failed { error, .. } => (RecordStatus::Failed, error.clone()),
    };
    RunResult {
        status,
        reply: o.replies.join("\n\n"),
        note,
        llm_calls: o.llm_calls,
        tool_calls: o.tool_calls,
    }
}

/// Runs one job: builds its message (reading its file through the jail),
/// runs the loop, and saves the reply if the job asks to. The outcome is
/// `None` if the loop never started.
pub async fn run_job<S: Services>(
    req: &RunRequest,
    file: &LoopFile,
    host: &AgentHost<S>,
    tz: &TimeZone,
    on_event: impl FnMut(Event) + Send,
) -> (RunResult, Option<RunOutcome>) {
    let now = jiff::Timestamp::now().to_zoned(tz.clone());
    let job = &req.job;
    let path = match &req.cause {
        Cause::File { path } => Some(path.clone()),
        _ => job.file.clone(),
    };
    let text = match &path {
        Some(p) => match host
            .jail()
            .and_then(|j| j.read_text(p, JOB_FILE_LIMIT).map_err(|e| e.to_string()))
        {
            Ok(t) => Some(t),
            // A file that triggered a run but isn't text (or is too big) is
            // still passed by path; the job's own file must be readable.
            Err(_) if matches!(req.cause, Cause::File { .. }) => None,
            Err(e) => {
                return (
                    RunResult::failed(format!("The job's file couldn't be read: {e}")),
                    None,
                );
            }
        },
        None => None,
    };
    let message = job.message_for(&now, path.as_deref().map(|p| (p, text.as_deref())));
    let inputs = RunInputs {
        message,
        history: vec![],
        overrides: HashMap::new(),
    };
    let outcome = run(file, host, &inputs, &req.cancel, on_event).await;
    let mut result = result_of(&outcome);
    if result.status == RecordStatus::Completed
        && !result.reply.is_empty()
        && let Some(target) = job.reply_path(&now)
    {
        let saved = host.jail().and_then(|j| {
            j.write(&target, result.reply.as_bytes())
                .map_err(|e| e.to_string())
        });
        result.note = match saved {
            Ok(()) => format!("Reply saved to {target}."),
            Err(e) => format!("The reply couldn't be saved to {target}: {e}"),
        };
    }
    (result, Some(outcome))
}

#[cfg(test)]
mod tests {
    use super::*;
    use chummy_flow::{AskKind, Block, Link, PortRef, Position};
    use chummy_sandbox::Protected;
    use chummy_scheduler::{Job, RunKind};
    use serde_json::json;
    use std::sync::atomic::AtomicBool;

    struct NoServices;

    impl Services for NoServices {
        fn secret(&self, _: &str) -> Option<String> {
            None
        }
        async fn tool_host(&self) -> Result<Arc<ToolHost>, String> {
            Err("no tools here".into())
        }
        async fn local_engine(&self, _: &ProviderConfig) -> Result<(String, String), String> {
            Err("no engine".into())
        }
    }

    fn manifest(name: &str, network: bool) -> ToolManifest {
        ToolManifest {
            name: name.into(),
            title: name.into(),
            version: "1".into(),
            description: "d".into(),
            parameters: json!({ "type": "object", "properties": {} }),
            network,
            timeout_secs: 5,
            dependencies: vec![],
            packages: vec![],
            python: chummy_tools::PYTHON_VERSION.into(),
            source_sha256: String::new(),
            builtin: true,
            installed_at: 0,
        }
    }

    fn setup(jail: Option<Jail>) -> AgentSetup {
        AgentSetup {
            providers: vec![],
            model: None,
            permissions: Permissions::default(),
            tool_grants: BTreeMap::new(),
            installed: vec![manifest("calc", false), manifest("fetch", true)],
            jail,
            asks: None,
            time_zone: None,
        }
    }

    #[test]
    fn tools_need_every_switch_and_online_tools_ask() {
        // Tools off: nothing offered, whatever the per-tool switches say.
        let mut s = setup(None);
        s.tool_grants.insert(
            "calc".into(),
            ToolGrant {
                enabled: true,
                ..Default::default()
            },
        );
        let h = AgentHost::new(Arc::new(NoServices), s);
        assert!(h.tool_info("calc").is_none());

        let mut s = setup(None);
        s.permissions = Permissions {
            tools: true,
            network: true,
            confirm_online_tools: true,
        };
        s.tool_grants.insert(
            "calc".into(),
            ToolGrant {
                enabled: true,
                ..Default::default()
            },
        );
        s.tool_grants.insert(
            "fetch".into(),
            ToolGrant {
                enabled: true,
                network: true,
                ask: false,
            },
        );
        let h = AgentHost::new(Arc::new(NoServices), s);
        assert!(h.tool_info("calc").is_some() && h.tool_info("fetch").is_some());
        assert!(
            !h.must_confirm("calc"),
            "offline tools don't ask by default"
        );
        assert!(h.must_confirm("fetch"), "online tools ask");
        assert!(!h.must_confirm("missing"));

        // With the tool's internet off, it can't go online, so it doesn't ask.
        let mut s = setup(None);
        s.permissions = Permissions {
            tools: true,
            network: true,
            confirm_online_tools: true,
        };
        s.tool_grants.insert(
            "fetch".into(),
            ToolGrant {
                enabled: true,
                network: false,
                ask: false,
            },
        );
        let h = AgentHost::new(Arc::new(NoServices), s);
        assert!(!h.must_confirm("fetch"));
        assert!(!h.net_policy(&manifest("fetch", true)).allowed);
        // A tool set to always ask does.
        let mut s = setup(None);
        s.permissions.tools = true;
        s.tool_grants.insert(
            "calc".into(),
            ToolGrant {
                enabled: true,
                network: false,
                ask: true,
            },
        );
        let h = AgentHost::new(Arc::new(NoServices), s);
        assert!(h.must_confirm("calc"));
    }

    #[tokio::test]
    async fn runs_without_a_broker_cant_ask() {
        let h = AgentHost::new(Arc::new(NoServices), setup(None));
        let ask = Ask {
            block: "a".into(),
            label: "Ask me".into(),
            kind: AskKind::Question,
            text: "?".into(),
        };
        assert!(h.ask(ask.clone()).await.is_err());

        let broker = AskBroker::new();
        let mut s = setup(None);
        s.asks = Some((
            broker.clone(),
            Origin {
                run_id: "r".into(),
                kind: RunKind::Chat,
                agent_id: None,
                job_id: None,
                title: "t".into(),
            },
        ));
        let h = AgentHost::new(Arc::new(NoServices), s);
        let (answer, ()) = tokio::join!(h.ask(ask), async {
            loop {
                if let Some(p) = broker.pending().first() {
                    broker.answer(&p.id, Answer::Text("ok".into())).unwrap();
                    break;
                }
                tokio::task::yield_now().await;
            }
        });
        assert_eq!(answer, Ok(Answer::Text("ok".into())));
    }

    fn block(id: &str, kind: &str, params: serde_json::Value) -> Block {
        Block {
            id: id.into(),
            kind: kind.into(),
            label: id.into(),
            position: Position::default(),
            params,
        }
    }

    fn link(a: &str, ap: &str, b: &str, bp: &str) -> Link {
        Link {
            from: PortRef {
                block: a.into(),
                port: ap.into(),
            },
            to: PortRef {
                block: b.into(),
                port: bp.into(),
            },
        }
    }

    #[tokio::test]
    async fn jobs_read_their_file_and_save_replies() {
        let tmp = tempfile::tempdir().unwrap();
        let ws = tmp.path().join("ws");
        std::fs::create_dir_all(ws.join("inbox")).unwrap();
        std::fs::write(ws.join("inbox/note.txt"), "buy milk").unwrap();
        std::fs::write(ws.join(".hidden"), "x").unwrap();
        let jail = Jail::open(&ws, &Protected::none()).unwrap();

        let snap = snapshot(&jail, ".").unwrap();
        assert!(snap.is_empty(), "folders and hidden files aren't watched");
        let snap = snapshot(&jail, "inbox").unwrap();
        assert_eq!(snap.keys().collect::<Vec<_>>(), ["note.txt"]);
        assert!(snap["note.txt"].modified_ms > 0);

        // message → reply, no model needed.
        let file = LoopFile {
            blocks: vec![
                block("in", "input", json!({})),
                block("out", "output", json!({})),
            ],
            links: vec![link("in", "out", "out", "in")],
            ..LoopFile::new("echo")
        };
        let mut job = Job::new("j", "J", "a");
        job.message = "File _FILE_ says: _FILE_TEXT_".into();
        job.save_reply_to = Some("out/_DATE_.md".into());
        let host = AgentHost::new(Arc::new(NoServices), setup(Some(jail)));
        let req = RunRequest {
            run_id: "r".into(),
            job: job.clone(),
            cause: Cause::File {
                path: "inbox/note.txt".into(),
            },
            cancel: Arc::new(AtomicBool::new(false)),
        };
        let (r, _) = run_job(&req, &file, &host, &TimeZone::UTC, |_| {}).await;
        assert_eq!(r.status, RecordStatus::Completed);
        assert_eq!(r.reply, "File inbox/note.txt says: buy milk");
        assert!(r.note.starts_with("Reply saved to out/"), "{}", r.note);
        let saved = std::fs::read_dir(ws.join("out")).unwrap().count();
        assert_eq!(saved, 1);

        // A job's own file that can't be read fails the run.
        job.file = Some("missing.txt".into());
        let req = RunRequest {
            cause: Cause::Manual,
            job,
            ..req
        };
        let (r, _) = run_job(&req, &file, &host, &TimeZone::UTC, |_| {}).await;
        assert_eq!(r.status, RecordStatus::Failed);
        assert!(r.note.contains("couldn't be read"));
    }
}
