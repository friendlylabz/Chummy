//! The Configure chummyd window: chummyd's whole configuration, made from
//! the app's agents, loops, models and jobs, and exported as a bundle
//! sealed with a strong password (`chummy-bundle`, the same code chummyd
//! opens it with).
//!
//! API keys never reach the window: they go from the keychain straight into
//! the sealed bundle. A bundle opened to change it is kept here (its
//! instance, state key and keys), and the window only holds a token for it.

use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::sync::Mutex;

use chummy_bundle::{BundleAgent, Config, MatrixChannel, Report};
use chummy_log::system::{self as syslog, Level};
use serde::{Deserialize, Serialize};
use tauri::{AppHandle, State, WebviewWindow};
use zeroize::Zeroizing;

use super::CmdResult;
use crate::dialogs;
use crate::state::AppState;

/// What the window shows to choose from.
#[derive(Debug, Clone, Serialize)]
pub struct Sources {
    pub agents: Vec<AgentSource>,
    pub jobs: Vec<JobSource>,
    /// This computer's time zone, the default.
    pub time_zone: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct AgentSource {
    pub id: String,
    pub name: String,
    pub loop_name: String,
    /// The model it uses, if any.
    pub model: Option<String>,
    /// Its model is Chummy's built-in engine, which chummyd doesn't have.
    pub builtin_engine: bool,
    pub tools: Vec<String>,
    /// Some tool may go online for it.
    pub network: bool,
}

#[derive(Debug, Clone, Serialize)]
pub struct JobSource {
    pub id: String,
    pub name: String,
    pub agent: String,
}

/// The choices made in the window.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Draft {
    pub name: String,
    pub time_zone: String,
    pub max_runs: u32,
    /// Messages each chat keeps, and chats each agent keeps.
    #[serde(default = "default_chat_messages")]
    pub chat_messages: u32,
    #[serde(default = "default_chats")]
    pub chats: u32,
    /// Agent ids included.
    pub agents: Vec<String>,
    /// Agent ids whose tool approvals may be answered over channels.
    pub approvals: Vec<String>,
    /// Job ids included.
    pub jobs: Vec<String>,
    pub terminal: bool,
    /// The Matrix channel, if it's on.
    #[serde(default)]
    pub matrix: Option<MatrixDraft>,
    pub message_text: bool,
    /// An opened bundle this one replaces (its token).
    #[serde(default)]
    pub base: Option<String>,
}

fn default_chat_messages() -> u32 {
    chummy_bundle::Limits::default().chat_messages
}
fn default_chats() -> u32 {
    chummy_bundle::Limits::default().chats
}

/// The Matrix channel as the window edits it.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct MatrixDraft {
    pub homeserver: String,
    pub user_id: String,
    /// Typed in the window; empty keeps the opened bundle's. Never sent
    /// back to the window.
    #[serde(default)]
    pub password: String,
    /// The opened bundle has a password (so it can be left empty).
    #[serde(default)]
    pub has_password: bool,
    /// Who may talk to each agent, by agent id.
    #[serde(default)]
    pub users: BTreeMap<String, Vec<String>>,
    /// Who each job reports to, by job id.
    #[serde(default)]
    pub report_to: BTreeMap<String, String>,
    pub upload_mb: u32,
}

/// A bundle opened to change it.
#[derive(Debug, Clone, Serialize)]
pub struct Opened {
    pub draft: Draft,
    /// What in it the app doesn't have (agents or jobs no longer here).
    pub missing: Vec<String>,
}

/// What an opened bundle keeps for the next export: its instance and keys.
struct Base {
    config: Config,
}

static OPENED: Mutex<Option<HashMap<String, Base>>> = Mutex::new(None);

fn opened() -> std::sync::MutexGuard<'static, Option<HashMap<String, Base>>> {
    OPENED.lock().unwrap_or_else(|e| e.into_inner())
}

fn system_tz() -> String {
    iana_time_zone::get_timezone().unwrap_or_else(|_| "UTC".into())
}

fn jobs_here(state: &AppState) -> Vec<chummy_scheduler::Job> {
    state
        .jobs
        .get()
        .map(|m| m.overview().jobs.into_iter().map(|j| j.job).collect())
        .unwrap_or_default()
}

#[tauri::command]
pub fn chummyd_sources(state: State<'_, AppState>) -> Sources {
    let s = state.settings();
    let loops = state.loops();
    Sources {
        agents: s
            .agents
            .iter()
            .map(|a| {
                let model = s.agent_model(a);
                AgentSource {
                    id: a.id.clone(),
                    name: a.name.clone(),
                    loop_name: loops
                        .iter()
                        .find(|l| l.id == a.loop_id)
                        .map(|l| l.name.clone())
                        .unwrap_or_else(|| a.loop_id.clone()),
                    model: model.map(|m| m.name.clone()),
                    builtin_engine: model
                        .is_some_and(|m| m.kind == chummy_llm::ProviderKind::Local),
                    tools: a.tools.keys().cloned().collect(),
                    network: a.tools.values().any(|t| t.network),
                }
            })
            .collect(),
        jobs: jobs_here(&state)
            .into_iter()
            .map(|j| JobSource {
                id: j.id,
                name: j.name,
                agent: j.agent,
            })
            .collect(),
        time_zone: system_tz(),
    }
}

/// The bundle's configuration for `draft`, from the app's settings, loops,
/// jobs and keychain.
fn build(state: &AppState, d: &Draft) -> Result<Config, String> {
    let s = state.settings();
    let name = d.name.trim();
    let mut c = Config::new_instance(name, d.time_zone.trim())?;
    let mut base_matrix_password = None;
    let base_secrets = match d.base.as_deref() {
        Some(token) => {
            let guard = opened();
            let base = guard
                .as_ref()
                .and_then(|m| m.get(token))
                .ok_or("The bundle you opened is no longer open. Open it again.")?;
            // The same instance, so its state folder carries on.
            c.instance.id = base.config.instance.id.clone();
            c.state_key = base.config.state_key.clone();
            base_matrix_password = base
                .config
                .channels
                .matrix
                .as_ref()
                .map(|m| m.password.clone());
            base.config.secrets.clone()
        }
        None => Default::default(),
    };
    c.limits.max_runs = d.max_runs;
    c.limits.chat_messages = d.chat_messages;
    c.limits.chats = d.chats;
    c.permissions = s.permissions.clone();
    c.channels.terminal.enabled = d.terminal;
    c.logging.message_text = d.message_text;
    if let Some(m) = &d.matrix {
        let password = if m.password.is_empty() {
            base_matrix_password.unwrap_or_default()
        } else {
            m.password.clone()
        };
        let users = |u: &Vec<String>| -> Vec<String> {
            u.iter()
                .map(|x| x.trim().to_string())
                .filter(|x| !x.is_empty())
                .collect()
        };
        c.channels.matrix = Some(MatrixChannel {
            homeserver: m.homeserver.trim().trim_end_matches('/').to_string(),
            user_id: m.user_id.trim().to_string(),
            password,
            // Only for what's included: unticking an agent or job drops it.
            users: m
                .users
                .iter()
                .filter(|(a, u)| d.agents.contains(a) && !users(u).is_empty())
                .map(|(a, u)| (a.clone(), users(u)))
                .collect(),
            report_to: m
                .report_to
                .iter()
                .filter(|(j, u)| d.jobs.contains(j) && !u.trim().is_empty())
                .map(|(j, u)| (j.clone(), u.trim().to_string()))
                .collect(),
            upload_mb: m.upload_mb,
        });
    }

    let mut providers: BTreeSet<String> = BTreeSet::new();
    let add_loop = |c: &mut Config, id: &str| -> Result<(), String> {
        if !c.loops.contains_key(id) {
            let file = state.trusted_loop(id)?;
            c.loops.insert(id.to_string(), file);
        }
        Ok(())
    };
    for id in &d.agents {
        let a = s
            .agent(id)
            .ok_or("An agent you chose no longer exists. Choose again.")?;
        add_loop(&mut c, &a.loop_id)?;
        match &a.model {
            Some(m) => {
                providers.insert(m.clone());
            }
            None => {
                if let Some(m) = s.default_model() {
                    providers.insert(m.id.clone());
                    c.default_provider = Some(m.id.clone());
                }
            }
        }
        c.agents.push(BundleAgent {
            id: a.id.clone(),
            name: a.name.clone(),
            loop_id: a.loop_id.clone(),
            model: a.model.clone(),
            tools: a.tools.clone(),
            loop_config: a.loop_config.clone(),
            model_options: a.model_options.clone(),
            channel_approvals: d.approvals.contains(id),
        });
    }
    for job in jobs_here(state)
        .into_iter()
        .filter(|j| d.jobs.contains(&j.id))
    {
        if !d.agents.contains(&job.agent) {
            return Err(format!(
                "The job “{}” runs as an agent that isn't included. Include the agent, or leave the job out.",
                job.name
            ));
        }
        if let Some(l) = &job.loop_id {
            add_loop(&mut c, l)?;
        }
        c.jobs.push(job);
    }
    // Models a loop's own blocks name (by connection id) come along too.
    let loops_text: String = c
        .loops
        .values()
        .map(|l| l.to_json())
        .chain(
            c.agents
                .iter()
                .map(|a| serde_json::to_string(&a.loop_config).unwrap_or_default()),
        )
        .collect();
    for p in &s.providers {
        if loops_text.contains(&format!("\"{}\"", p.id)) {
            providers.insert(p.id.clone());
        }
    }
    for id in &providers {
        let Some(p) = s.provider(id) else { continue };
        c.providers.push(p.clone());
        let key = p.secret_key();
        match state.secrets.get(&key) {
            Ok(Some(v)) => {
                c.secrets.insert(key, v);
            }
            _ => {
                if let Some(v) = base_secrets.get(&key) {
                    c.secrets.insert(key, v.clone());
                }
            }
        }
    }
    Ok(c)
}

/// What the checks chummyd runs at start-up say about `draft`.
#[tauri::command]
pub fn chummyd_check(state: State<'_, AppState>, draft: Draft) -> Report {
    match build(&state, &draft) {
        Ok(c) => c.check(),
        Err(e) => Report {
            errors: vec![e],
            warnings: vec![],
        },
    }
}

/// Whether a Matrix homeserver answers (its supported versions).
#[tauri::command]
pub async fn chummyd_check_homeserver(url: String) -> CmdResult<String> {
    let url = url.trim().trim_end_matches('/').to_string();
    if let Some(p) = chummy_llm::endpoint_problem(&url) {
        return Err(p);
    }
    if chummy_llm::unencrypted_remote(&url) {
        return Err(
            "The homeserver must use https (plain http only on this computer or your local network)."
                .into(),
        );
    }
    let client = chummy_http::builder()
        .timeout(std::time::Duration::from_secs(15))
        .build()
        .map_err(|e| e.to_string())?;
    let r = client
        .get(format!("{url}/_matrix/client/versions"))
        .send()
        .await
        .map_err(|e| {
            format!(
                "The homeserver didn't answer: {}",
                chummy_http::describe(&e)
            )
        })?;
    if !r.status().is_success() {
        return Err(format!(
            "That doesn't look like a Matrix homeserver (it answered {}).",
            r.status()
        ));
    }
    #[derive(Deserialize)]
    struct Versions {
        versions: Vec<String>,
    }
    let v: Versions = r
        .json()
        .await
        .map_err(|_| "That doesn't look like a Matrix homeserver.".to_string())?;
    let latest = v
        .versions
        .iter()
        .filter(|v| v.starts_with('v'))
        .max_by(|a, b| {
            let n = |s: &str| -> Vec<u32> {
                s[1..].split('.').filter_map(|p| p.parse().ok()).collect()
            };
            n(a).cmp(&n(b))
        });
    Ok(match latest {
        Some(l) => format!("The homeserver answers (Matrix {l})."),
        None => "The homeserver answers.".into(),
    })
}

/// Why a password isn't strong enough, if it isn't.
#[tauri::command]
pub fn chummyd_password_problem(password: String) -> Option<String> {
    let password = Zeroizing::new(password);
    chummy_bundle::password_problem(&password)
}

#[tauri::command]
pub fn chummyd_generate_password() -> CmdResult<String> {
    chummy_bundle::generate_password()
}

/// Opens a bundle made before, to change it: its instance and keys carry
/// over to the next export, so chummyd's state folder keeps working.
#[tauri::command]
pub async fn chummyd_open_bundle(
    app: AppHandle,
    window: WebviewWindow,
    state: State<'_, AppState>,
    password: String,
) -> CmdResult<Option<Opened>> {
    let password = Zeroizing::new(password);
    let Some(path) = dialogs::pick_file(
        &app,
        window.label(),
        "Open a chummyd bundle",
        "chummyd bundle",
        chummy_bundle::EXTENSION,
    )
    .await
    else {
        return Ok(None);
    };
    let bytes =
        std::fs::read(&path).map_err(|e| format!("Couldn't read {}: {e}", path.display()))?;
    let config = tauri::async_runtime::spawn_blocking(move || {
        // The same key chummyd's password file holds, from the password.
        chummy_bundle::BundleKey::for_bundle(&bytes, &password)
            .and_then(|key| chummy_bundle::open_unchecked(&bytes, &key))
    })
    .await
    .map_err(|e| e.to_string())?
    .map_err(|e| e.to_string())?;
    let s = state.settings();
    let here: BTreeSet<String> = jobs_here(&state).into_iter().map(|j| j.id).collect();
    let mut missing = vec![];
    let agents: Vec<String> = config
        .agents
        .iter()
        .filter(|a| {
            let ok = s.agent(&a.id).is_some();
            if !ok {
                missing.push(format!("The agent “{}” isn't in this app.", a.name));
            }
            ok
        })
        .map(|a| a.id.clone())
        .collect();
    let jobs: Vec<String> = config
        .jobs
        .iter()
        .filter(|j| {
            let ok = here.contains(&j.id);
            if !ok {
                missing.push(format!("The job “{}” isn't in this app.", j.name));
            }
            ok
        })
        .map(|j| j.id.clone())
        .collect();
    let draft = Draft {
        name: config.instance.name.clone(),
        time_zone: config.instance.time_zone.clone(),
        max_runs: config.limits.max_runs,
        chat_messages: config.limits.chat_messages,
        chats: config.limits.chats,
        approvals: config
            .agents
            .iter()
            .filter(|a| a.channel_approvals && agents.contains(&a.id))
            .map(|a| a.id.clone())
            .collect(),
        agents,
        jobs,
        terminal: config.channels.terminal.enabled,
        matrix: config.channels.matrix.as_ref().map(|m| MatrixDraft {
            homeserver: m.homeserver.clone(),
            user_id: m.user_id.clone(),
            password: String::new(),
            has_password: !m.password.is_empty(),
            users: m.users.clone(),
            report_to: m.report_to.clone(),
            upload_mb: m.upload_mb,
        }),
        message_text: config.logging.message_text,
        base: Some(chummy_store::random_id(8)),
    };
    let token = draft.base.clone().unwrap_or_default();
    opened()
        .get_or_insert_with(HashMap::new)
        .insert(token, Base { config });
    syslog::log(
        Level::Info,
        "security",
        "Opened a chummyd bundle to change it",
        &[("file", &path.display().to_string())],
    );
    Ok(Some(Opened { draft, missing }))
}

/// Makes the bundle: checks it, warns what it holds, asks where to save it,
/// and seals it with `password`. Returns where it went, or `None` if the
/// user stopped.
#[tauri::command]
pub async fn chummyd_export(
    app: AppHandle,
    window: WebviewWindow,
    state: State<'_, AppState>,
    draft: Draft,
    password: String,
) -> CmdResult<Option<String>> {
    let password = Zeroizing::new(password);
    if let Some(p) = chummy_bundle::password_problem(&password) {
        return Err(p);
    }
    let config = build(&state, &draft)?;
    let report = config.check();
    if !report.ok() {
        return Err(report.errors.join(" "));
    }
    if !dialogs::confirm(
        &app,
        window.label(),
        "Export the chummyd bundle?",
        "The bundle holds everything chummyd needs, including the API keys of the model connections it uses. Anyone who has both the bundle and its password can read them. Keep the bundle and the password safe, and apart.",
        "Export",
        false,
    )
    .await
    {
        return Ok(None);
    }
    // Always the name chummyd's Docker setup expects, so nothing needs
    // renaming on the server.
    let suggested = format!("bundle.{}", chummy_bundle::EXTENSION);
    let Some(path) = dialogs::save_file(
        &app,
        window.label(),
        "Export the chummyd bundle",
        "chummyd bundle",
        &suggested,
        chummy_bundle::EXTENSION,
    )
    .await
    else {
        return Ok(None);
    };
    let (agents, jobs, instance) = (
        config.agents.len(),
        config.jobs.len(),
        config.instance.id.clone(),
    );
    let sealed =
        tauri::async_runtime::spawn_blocking(move || chummy_bundle::seal(&config, &password))
            .await
            .map_err(|e| e.to_string())?
            .map_err(|e| e.to_string())?;
    chummy_store::write_atomic(&path, &sealed, false)
        .map_err(|e| format!("Couldn't save {}: {e}", path.display()))?;
    syslog::log(
        Level::Warn,
        "security",
        "Exported a chummyd bundle",
        &[
            ("instance", &instance),
            ("agents", &agents.to_string()),
            ("jobs", &jobs.to_string()),
        ],
    );
    Ok(Some(path.display().to_string()))
}
