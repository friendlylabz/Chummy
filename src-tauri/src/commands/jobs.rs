//! Scheduled jobs: the app's side of `chummy-scheduler`.
//!
//! [`AppJobs`] is the app's [`JobRunner`]: runs go through
//! `chummy-session` (a job runs as its agent, with the same folder, model
//! and permissions as chatting with it, and is shown like any other run);
//! the app adds the tray, notifications and the Automation menu. The
//! manager itself is shared with chummyd.

use chummy_scheduler::{
    Job, JobRunner, Overview, RecordStatus, RunRecord, RunRequest, RunResult, Snapshot,
};
use tauri::{AppHandle, Emitter, Manager, State, WebviewWindow};

use super::{CmdResult, err};
use crate::dialogs;
use crate::events;
use crate::notify;
use crate::platform::AppPlatform;
use crate::state::AppState;

/// Largest job file imported.
const MAX_IMPORT_BYTES: u64 = 256 * 1024;

pub struct AppJobs {
    pub app: AppHandle,
}

impl JobRunner for AppJobs {
    /// Jobs run alongside the agent's chats and other jobs, like a chat in
    /// the background; only a test run in the loop editor holds them back.
    fn agent_busy(&self, agent: &str) -> bool {
        !self.app.state::<AppState>().session.job_may_start(agent)
    }

    async fn run(&self, req: RunRequest) -> RunResult {
        chummy_session::run_job(&AppPlatform::new(&self.app), &req).await
    }

    fn snapshot(&self, agent: &str, folder: &str) -> Result<Snapshot, String> {
        chummy_session::snapshot(&AppPlatform::new(&self.app), agent, folder)
    }

    fn changed(&self, _job: Option<&str>) {
        let state = self.app.state::<AppState>();
        if let Some(m) = state.jobs.get() {
            let _ = self.app.emit(events::JOBS_CHANGED, m.overview());
        }
        crate::tray::refresh(&self.app);
    }

    fn finished(&self, job: &Job, record: &RunRecord) {
        let problem = !matches!(record.status, RecordStatus::Completed);
        chummy_log::system::log(
            if problem {
                chummy_log::system::Level::Warn
            } else {
                chummy_log::system::Level::Info
            },
            "jobs",
            format!(
                "Job “{}” finished: {}",
                job.name,
                format!("{:?}", record.status).to_lowercase()
            ),
            &[
                ("job", &job.id),
                ("run", &record.run_id),
                ("note", &record.note),
            ],
        );
        let show = match job.notify {
            chummy_scheduler::Notify::Always => true,
            chummy_scheduler::Notify::Problems => problem && record.status != RecordStatus::Stopped,
            chummy_scheduler::Notify::Never => false,
        };
        if show {
            // No reply text: it may be private, and notifications show on
            // lock screens.
            let body = if problem {
                "Open the Automation menu to see what happened."
            } else {
                "Open the Automation menu to see the reply."
            };
            let title = if problem {
                format!("“{}” didn't finish", job.name)
            } else {
                format!("“{}” finished", job.name)
            };
            notify::show(&self.app, &title, body);
        }
    }
}

fn manager(state: &AppState) -> CmdResult<&chummy_scheduler::Manager<AppJobs>> {
    state
        .jobs
        .get()
        .ok_or_else(|| "Automation is still starting.".to_string())
}

#[tauri::command]
pub fn list_jobs(state: State<'_, AppState>) -> CmdResult<Overview> {
    Ok(manager(&state)?.overview())
}

/// A file-name-safe id from a name, unique among `taken`.
fn new_id(name: &str, taken: &[String]) -> String {
    let mut base: String = name
        .to_lowercase()
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() { c } else { '-' })
        .collect::<String>()
        .split('-')
        .filter(|p| !p.is_empty())
        .collect::<Vec<_>>()
        .join("-");
    base.truncate(40);
    let base = base.trim_matches('-').to_string();
    let base = if base.is_empty() || !chummy_scheduler::job::valid_id(&base) {
        "job".to_string()
    } else {
        base
    };
    if !taken.contains(&base) {
        return base;
    }
    (2..)
        .map(|n| format!("{base}-{n}"))
        .find(|id| !taken.contains(id))
        .unwrap_or(base)
}

/// Checks the parts of a job that depend on this app's agents and loops.
fn check_refs(state: &AppState, job: &Job) -> CmdResult<()> {
    let s = state.settings();
    if s.agent(&job.agent).is_none() {
        return Err("Choose the agent that runs this job.".into());
    }
    if let Some(l) = &job.loop_id
        && state.library.load(l).is_err()
    {
        return Err("The job's loop is missing. Choose another, or use the agent's own.".into());
    }
    Ok(())
}

/// Saves a job. An empty id makes a new job. Returns the id.
#[tauri::command]
pub fn save_job(state: State<'_, AppState>, mut job: Job) -> CmdResult<String> {
    let m = manager(&state)?;
    job.format = chummy_scheduler::job::FORMAT.into();
    job.version = chummy_scheduler::job::VERSION;
    if job.id.is_empty() {
        let taken: Vec<String> = m.overview().jobs.into_iter().map(|j| j.job.id).collect();
        job.id = new_id(&job.name, &taken);
    }
    check_refs(&state, &job)?;
    m.save(job.clone()).map_err(err)?;
    Ok(job.id)
}

#[tauri::command]
pub fn delete_job(state: State<'_, AppState>, id: String) -> CmdResult<()> {
    manager(&state)?.delete(&id).map_err(err)
}

#[tauri::command]
pub fn set_job_enabled(state: State<'_, AppState>, id: String, enabled: bool) -> CmdResult<()> {
    let m = manager(&state)?;
    if enabled && let Some(job) = m.job(&id) {
        check_refs(&state, &job)?;
    }
    m.set_enabled(&id, enabled).map_err(err)
}

#[tauri::command]
pub fn run_job_now(state: State<'_, AppState>, id: String) -> CmdResult<()> {
    let m = manager(&state)?;
    if let Some(job) = m.job(&id) {
        check_refs(&state, &job)?;
    }
    m.run_now(&id)
}

#[tauri::command]
pub fn stop_job(state: State<'_, AppState>, id: String) -> CmdResult<()> {
    manager(&state)?.stop(&id);
    Ok(())
}

#[tauri::command]
pub fn job_history(state: State<'_, AppState>, id: String) -> CmdResult<Vec<RunRecord>> {
    Ok(manager(&state)?.history(&id))
}

#[tauri::command]
pub fn set_jobs_paused(app: AppHandle, state: State<'_, AppState>, paused: bool) -> CmdResult<()> {
    manager(&state)?.set_paused(paused);
    crate::tray::refresh(&app);
    Ok(())
}

/// Imports a job file. Imported jobs start switched off, so nothing runs
/// until the user has looked at it.
#[tauri::command]
pub async fn import_job(
    app: AppHandle,
    window: WebviewWindow,
    state: State<'_, AppState>,
) -> CmdResult<Option<String>> {
    let Some(path) =
        dialogs::pick_json_file(&app, window.label(), "Import a job", "Chummy job").await
    else {
        return Ok(None);
    };
    let read = (|| {
        let len = std::fs::metadata(&path).map_err(err)?.len();
        if len > MAX_IMPORT_BYTES {
            return Err("That file is too big to be a job.".to_string());
        }
        let text = std::fs::read_to_string(&path).map_err(err)?;
        Job::from_json(&text).map_err(err)
    })();
    let mut job = match read {
        Ok(j) => j,
        Err(e) => {
            dialogs::error(
                &app,
                window.label(),
                "Couldn't import that job",
                &format!("{}\n\n{e}", path.display()),
            )
            .await;
            return Ok(None);
        }
    };
    let m = manager(&state)?;
    let taken: Vec<String> = m.overview().jobs.into_iter().map(|j| j.job.id).collect();
    job.id = new_id(&job.name, &taken);
    job.enabled = false;
    // The agent may not exist here; the user picks one before switching it on.
    if state.settings().agent(&job.agent).is_none() {
        job.agent = state
            .settings()
            .agents
            .first()
            .map(|a| a.id.clone())
            .unwrap_or_default();
    }
    if let Some(l) = &job.loop_id
        && state.library.load(l).is_err()
    {
        job.loop_id = None;
    }
    m.save(job.clone()).map_err(err)?;
    Ok(Some(job.id))
}

#[tauri::command]
pub async fn export_job(
    app: AppHandle,
    window: WebviewWindow,
    state: State<'_, AppState>,
    id: String,
) -> CmdResult<bool> {
    let job = manager(&state)?
        .job(&id)
        .ok_or("That job no longer exists.")?;
    let Some(path) = dialogs::save_json_file(
        &app,
        window.label(),
        "Export job",
        "Chummy job",
        &format!("{id}.job.json"),
    )
    .await
    else {
        return Ok(false);
    };
    std::fs::write(&path, job.to_json() + "\n")
        .map_err(|e| format!("Couldn't save {}: {e}", path.display()))?;
    Ok(true)
}

/// Starts the scheduler. Called once, at startup.
pub fn start(app: &AppHandle) -> Result<(), String> {
    let state = app.state::<AppState>();
    let store = chummy_scheduler::JobStore::open(&state.dirs.jobs)
        .map_err(|e| format!("couldn't open the jobs folder: {e}"))?;
    let max = state.settings().general.max_job_runs.clamp(1, 4) as usize;
    let m = chummy_scheduler::Manager::new(AppJobs { app: app.clone() }, store, max, None);
    if state.jobs.set(m.clone()).is_err() {
        return Ok(());
    }
    tauri::async_runtime::spawn(async move { m.run_forever().await });
    Ok(())
}

/// A short line about the jobs, for the tray and the quit question.
pub fn summary(state: &AppState) -> (usize, usize, bool) {
    match state.jobs.get() {
        Some(m) => {
            let o = m.overview();
            let scheduled = o
                .jobs
                .iter()
                .filter(|j| j.job.enabled && !j.job.triggers.is_empty())
                .count();
            (m.active().len(), scheduled, o.paused)
        }
        None => (0, 0, false),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ids_are_safe_and_unique() {
        assert_eq!(new_id("Daily digest!", &[]), "daily-digest");
        assert_eq!(
            new_id("Daily digest", &["daily-digest".into()]),
            "daily-digest-2"
        );
        assert_eq!(new_id("日報", &[]), "job");
        assert_eq!(new_id("state", &[]), "job", "reserved names aren't used");
        let long = new_id(&"a".repeat(100), &[]);
        assert!(long.len() <= 40 && chummy_scheduler::job::valid_id(&long));

        // (was causes_read_well)
        {
            assert_eq!(
                chummy_scheduler::Cause::Manual.describe(),
                "Started by hand"
            );
        }
    }
}
