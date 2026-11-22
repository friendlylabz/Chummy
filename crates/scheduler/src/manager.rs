//! The job manager: keeps the [`Planner`] fed with the time and file
//! changes, starts runs through a [`JobRunner`], and records what happened.
//!
//! Everything specific to where jobs run (opening agents' folders, models,
//! tools, telling windows) is behind [`JobRunner`]. The Chummy app implements
//! it with its windows; chummyd will implement it with its own config and
//! connections, and reuse this manager unchanged.

use std::collections::BTreeMap;
use std::future::Future;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::{Duration, Instant};

use jiff::Timestamp;
use jiff::tz::TimeZone;
use serde::Serialize;
use tokio::sync::Notify;

use crate::job::{Job, JobError};
use crate::plan::{Cause, Note, Planner, Start};
use crate::store::{BadJobFile, JobStore, RecordStatus, RunRecord};
use crate::watch::{FolderWatch, POLL_SECONDS, Snapshot};

/// One run for the runner to carry out.
#[derive(Debug, Clone)]
pub struct RunRequest {
    pub run_id: String,
    pub job: Job,
    pub cause: Cause,
    /// Set when the run should stop (by the user, or the job's time limit).
    pub cancel: Arc<AtomicBool>,
}

/// How a run went.
#[derive(Debug, Clone, PartialEq)]
pub struct RunResult {
    pub status: RecordStatus,
    pub reply: String,
    /// What went wrong, if anything.
    pub note: String,
    pub llm_calls: u32,
    pub tool_calls: u32,
}

impl RunResult {
    /// A run that couldn't start, e.g. because its agent is gone.
    pub fn failed(note: impl Into<String>) -> Self {
        Self {
            status: RecordStatus::Failed,
            reply: String::new(),
            note: note.into(),
            llm_calls: 0,
            tool_calls: 0,
        }
    }
}

/// What the manager needs from wherever it runs.
pub trait JobRunner: Send + Sync + 'static {
    /// Whether the agent is busy outside the scheduler (e.g. in a chat).
    fn agent_busy(&self, agent: &str) -> bool;
    /// Runs the job's loop to the end.
    fn run(&self, req: RunRequest) -> impl Future<Output = RunResult> + Send;
    /// The files directly inside `folder` of the agent's folder (through
    /// the agent's jail).
    fn snapshot(&self, agent: &str, folder: &str) -> Result<Snapshot, String>;
    /// Jobs, their states or their histories changed. `job` is `None` when
    /// it's the whole list.
    fn changed(&self, job: Option<&str>);
    /// A run finished; e.g. to show a notification.
    fn finished(&self, _job: &Job, _record: &RunRecord) {}
}

/// A job as shown to the user.
#[derive(Debug, Clone, Serialize)]
pub struct JobView {
    pub job: Job,
    /// "running", "queued" or "idle".
    pub state: &'static str,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub run_id: Option<String>,
    /// Next scheduled run (unix seconds).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub next_run: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub last: Option<RunRecord>,
    /// Each trigger in words.
    pub schedule: Vec<String>,
    /// Why a watched folder can't be read, if it can't.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub watch_error: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct Overview {
    pub jobs: Vec<JobView>,
    pub paused: bool,
    pub bad_files: Vec<BadJobFile>,
}

struct Active {
    run_id: String,
    cancel: Arc<AtomicBool>,
}

struct Inner<R> {
    runner: R,
    store: JobStore,
    /// `None`: the computer's zone, looked up on every check.
    tz: Option<TimeZone>,
    planner: Mutex<Planner>,
    watches: Mutex<BTreeMap<(String, String), FolderWatch>>,
    watch_errors: Mutex<BTreeMap<String, String>>,
    last_poll: Mutex<Option<Instant>>,
    active: Mutex<BTreeMap<String, Active>>,
    bad_files: Mutex<Vec<BadJobFile>>,
    wake: Notify,
}

pub struct Manager<R: JobRunner> {
    inner: Arc<Inner<R>>,
}

impl<R: JobRunner> Clone for Manager<R> {
    fn clone(&self) -> Self {
        Self {
            inner: self.inner.clone(),
        }
    }
}

fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|e| e.into_inner())
}

fn now() -> i64 {
    Timestamp::now().as_second()
}

fn new_run_id() -> String {
    let mut b = [0u8; 8];
    let _ = getrandom::fill(&mut b);
    format!(
        "job-{}",
        b.iter().map(|x| format!("{x:02x}")).collect::<String>()
    )
}

/// Joins a watched folder and a file name into a path in the agent's folder.
fn join(folder: &str, name: &str) -> String {
    let f = folder.trim().trim_end_matches(['/', '\\']);
    if f.is_empty() || f == "." {
        name.to_string()
    } else {
        format!("{f}/{name}")
    }
}

fn note_record(note: Note) -> (String, RunRecord) {
    match &note {
        Note::Missed { job, .. } => chummy_log::system::log(
            chummy_log::system::Level::Warn,
            "jobs",
            "A scheduled time was missed (Chummy wasn't running)",
            &[("job", job)],
        ),
        Note::Skipped { job, reason, .. } => chummy_log::system::log(
            chummy_log::system::Level::Info,
            "jobs",
            format!("A job run was skipped: {reason}"),
            &[("job", job)],
        ),
    }
    match note {
        Note::Missed { job, due } => (
            job,
            RunRecord {
                run_id: new_run_id(),
                cause: Cause::Schedule { due },
                status: RecordStatus::Missed,
                started_at: due,
                finished_at: None,
                reply: String::new(),
                note: "Chummy wasn't running (or the computer was asleep) at this time.".into(),
                llm_calls: 0,
                tool_calls: 0,
            },
        ),
        Note::Skipped { job, cause, reason } => (
            job,
            RunRecord {
                run_id: new_run_id(),
                cause,
                status: RecordStatus::Skipped,
                started_at: now(),
                finished_at: None,
                reply: String::new(),
                note: reason,
                llm_calls: 0,
                tool_calls: 0,
            },
        ),
    }
}

/// Makes sure a run leaves the planner however its task ends (even by a
/// panic), so a job never stays "running".
struct RunGuard<R: JobRunner> {
    inner: Arc<Inner<R>>,
    job: String,
}

impl<R: JobRunner> Drop for RunGuard<R> {
    fn drop(&mut self) {
        lock(&self.inner.planner).finished(&self.job, now());
        lock(&self.inner.active).remove(&self.job);
        self.inner.wake.notify_one();
    }
}

impl<R: JobRunner> Manager<R> {
    /// Loads the jobs in `store`. `tz` fixes the time zone (chummyd); `None`
    /// follows the computer's.
    pub fn new(runner: R, store: JobStore, max_running: usize, tz: Option<TimeZone>) -> Self {
        let (state, paused) = store.state();
        let mut planner = Planner::new(state, max_running);
        let (jobs, bad) = store.list();
        planner.set_jobs(jobs, now());
        planner.set_paused(paused, now());
        Self {
            inner: Arc::new(Inner {
                runner,
                store,
                tz,
                planner: Mutex::new(planner),
                watches: Mutex::default(),
                watch_errors: Mutex::default(),
                last_poll: Mutex::new(None),
                active: Mutex::default(),
                bad_files: Mutex::new(bad),
                wake: Notify::new(),
            }),
        }
    }

    pub fn runner(&self) -> &R {
        &self.inner.runner
    }

    fn tz(&self) -> TimeZone {
        self.inner.tz.clone().unwrap_or_else(TimeZone::system)
    }

    /// Checks schedules and folders every few seconds and starts runs, until
    /// the returned future is dropped.
    pub async fn run_forever(&self) {
        loop {
            self.step();
            tokio::select! {
                _ = tokio::time::sleep(Duration::from_secs(5)) => {}
                _ = self.inner.wake.notified() => {}
            }
        }
    }

    /// One check: queue what's due, poll folders, start what can start.
    pub fn step(&self) {
        let t = now();
        let tz = self.tz();
        let (notes, before) = {
            let mut p = lock(&self.inner.planner);
            let before = p.state().clone();
            (p.tick(t, &tz), before)
        };
        let mut changed = !notes.is_empty();
        self.record_notes(notes);
        changed |= self.poll_folders(t);
        {
            let p = lock(&self.inner.planner);
            if *p.state() != before
                && let Err(e) = self.inner.store.save_state(p.state(), p.paused())
            {
                chummy_log::system::error(
                    "jobs",
                    format!("Couldn't save the scheduler's state: {e}"),
                );
            }
        }
        changed |= self.dispatch();
        if changed {
            self.inner.runner.changed(None);
        }
    }

    fn record_notes(&self, notes: Vec<Note>) {
        for n in notes {
            let (job, r) = note_record(n);
            let _ = self.inner.store.record(&job, r);
        }
    }

    /// Polls watched folders (at most every [`POLL_SECONDS`]). Returns
    /// whether anything changed.
    fn poll_folders(&self, t: i64) -> bool {
        {
            let mut last = lock(&self.inner.last_poll);
            if last.is_some_and(|l| l.elapsed() < Duration::from_secs(POLL_SECONDS)) {
                return false;
            }
            *last = Some(Instant::now());
        }
        let targets: Vec<(Job, String)> = {
            let p = lock(&self.inner.planner);
            if p.paused() {
                return false;
            }
            p.jobs()
                .filter(|j| j.enabled && !p.is_running(&j.id))
                .flat_map(|j| {
                    j.watched_folders()
                        .into_iter()
                        .map(|f| (j.clone(), f.to_string()))
                        .collect::<Vec<_>>()
                })
                .collect()
        };
        let mut errors = BTreeMap::new();
        let mut changed = false;
        let mut keep = Vec::new();
        for (job, folder) in targets {
            keep.push((job.id.clone(), folder.clone()));
            let snap = match self.inner.runner.snapshot(&job.agent, &folder) {
                Ok(s) => s,
                Err(e) => {
                    errors.insert(job.id.clone(), format!("{folder}: {e}"));
                    continue;
                }
            };
            let ready = lock(&self.inner.watches)
                .entry((job.id.clone(), folder.clone()))
                .or_default()
                .poll(snap);
            for name in ready {
                let cause = Cause::File {
                    path: join(&folder, &name),
                };
                let r = lock(&self.inner.planner).request(&job.id, cause, t);
                if let Err(note) = r {
                    self.record_notes(vec![note]);
                }
                changed = true;
            }
        }
        // Forget folders no longer watched (or whose job is running: they're
        // looked at afresh when it ends).
        lock(&self.inner.watches)
            .retain(|k, _| keep.contains(k) || lock(&self.inner.planner).is_running(&k.0));
        let mut old = lock(&self.inner.watch_errors);
        if *old != errors {
            *old = errors;
            changed = true;
        }
        changed
    }

    /// Starts every run that can start. Returns whether any did.
    fn dispatch(&self) -> bool {
        let mut any = false;
        loop {
            let start = {
                let inner = &self.inner;
                lock(&inner.planner).next_start(&|a| inner.runner.agent_busy(a))
            };
            let Some(start) = start else { return any };
            any = true;
            self.spawn(start);
        }
    }

    fn spawn(&self, start: Start) {
        let Some(job) = lock(&self.inner.planner).job(&start.job).cloned() else {
            lock(&self.inner.planner).finished(&start.job, now());
            return;
        };
        let run_id = new_run_id();
        let cancel = Arc::new(AtomicBool::new(false));
        lock(&self.inner.active).insert(
            job.id.clone(),
            Active {
                run_id: run_id.clone(),
                cancel: cancel.clone(),
            },
        );
        let inner = self.inner.clone();
        let req = RunRequest {
            run_id: run_id.clone(),
            job: job.clone(),
            cause: start.cause.clone(),
            cancel: cancel.clone(),
        };
        tokio::spawn(async move {
            let _guard = RunGuard {
                inner: inner.clone(),
                job: job.id.clone(),
            };
            let started_at = now();
            inner.runner.changed(Some(&job.id));
            let timed_out = Arc::new(AtomicBool::new(false));
            let limit = job.time_limit_minutes;
            let timer = (limit > 0).then(|| {
                let cancel = cancel.clone();
                let timed_out = timed_out.clone();
                tokio::spawn(async move {
                    tokio::time::sleep(Duration::from_secs(u64::from(limit) * 60)).await;
                    timed_out.store(true, Ordering::Relaxed);
                    cancel.store(true, Ordering::Relaxed);
                })
            });
            let mut result = inner.runner.run(req).await;
            if let Some(t) = timer {
                t.abort();
            }
            if timed_out.load(Ordering::Relaxed) && result.status == RecordStatus::Stopped {
                result.note = format!(
                    "Stopped after {limit} minute{}, the job's time limit.",
                    if limit == 1 { "" } else { "s" }
                );
            }
            let record = RunRecord {
                run_id,
                cause: start.cause,
                status: result.status,
                started_at,
                finished_at: Some(now()),
                reply: result.reply,
                note: result.note,
                llm_calls: result.llm_calls,
                tool_calls: result.tool_calls,
            };
            if let Err(e) = inner.store.record(&job.id, record.clone()) {
                chummy_log::system::error(
                    "jobs",
                    format!("Couldn't save the run of job {}: {e}", job.id),
                );
            }
            // Files the run wrote in watched folders don't start it again.
            for folder in job.watched_folders() {
                if let Ok(s) = inner.runner.snapshot(&job.agent, folder) {
                    lock(&inner.watches)
                        .entry((job.id.clone(), folder.to_string()))
                        .or_default()
                        .accept(s);
                }
            }
            {
                let mut p = inner.planner.lock().unwrap_or_else(|e| e.into_inner());
                p.finished(&job.id, now());
                let _ = inner.store.save_state(p.state(), p.paused());
            }
            lock(&inner.active).remove(&job.id);
            inner.runner.finished(&job, &record);
            inner.runner.changed(Some(&job.id));
        });
    }

    /// Re-reads the job files (after they changed on disk).
    pub fn reload(&self) {
        let (jobs, bad) = self.inner.store.list();
        lock(&self.inner.planner).set_jobs(jobs, now());
        *lock(&self.inner.bad_files) = bad;
        self.inner.wake.notify_one();
        self.inner.runner.changed(None);
    }

    /// Everything the scheduler window shows.
    pub fn overview(&self) -> Overview {
        let t = now();
        let tz = self.tz();
        let p = lock(&self.inner.planner);
        let active = lock(&self.inner.active);
        let errors = lock(&self.inner.watch_errors);
        let jobs = p
            .jobs()
            .map(|j| JobView {
                state: if p.is_running(&j.id) {
                    "running"
                } else if p.is_queued(&j.id) {
                    "queued"
                } else {
                    "idle"
                },
                run_id: active.get(&j.id).map(|a| a.run_id.clone()),
                next_run: p.next_run(&j.id, t, &tz),
                last: self.inner.store.history(&j.id).pop(),
                schedule: j.triggers.iter().map(|t| t.describe()).collect(),
                watch_error: errors.get(&j.id).cloned(),
                job: j.clone(),
            })
            .collect();
        Overview {
            jobs,
            paused: p.paused(),
            bad_files: lock(&self.inner.bad_files).clone(),
        }
    }

    pub fn job(&self, id: &str) -> Option<Job> {
        lock(&self.inner.planner).job(id).cloned()
    }

    /// Saves a new or edited job.
    pub fn save(&self, job: Job) -> Result<(), JobError> {
        self.inner.store.save(&job)?;
        let mut jobs: Vec<Job> = lock(&self.inner.planner)
            .jobs()
            .filter(|j| j.id != job.id)
            .cloned()
            .collect();
        jobs.push(job.clone());
        lock(&self.inner.planner).set_jobs(jobs, now());
        self.inner.wake.notify_one();
        self.inner.runner.changed(Some(&job.id));
        Ok(())
    }

    /// Deletes a job, stopping it if it's running.
    pub fn delete(&self, id: &str) -> Result<(), JobError> {
        self.stop(id);
        self.inner.store.delete(id)?;
        let jobs: Vec<Job> = lock(&self.inner.planner)
            .jobs()
            .filter(|j| j.id != id)
            .cloned()
            .collect();
        lock(&self.inner.planner).set_jobs(jobs, now());
        lock(&self.inner.watches).retain(|k, _| k.0 != id);
        self.inner.runner.changed(None);
        Ok(())
    }

    pub fn set_enabled(&self, id: &str, enabled: bool) -> Result<(), JobError> {
        let mut job = self
            .job(id)
            .ok_or_else(|| JobError::Invalid("That job no longer exists.".into()))?;
        job.enabled = enabled;
        self.save(job)
    }

    /// Starts a job now (it waits if its agent is busy).
    pub fn run_now(&self, id: &str) -> Result<(), String> {
        lock(&self.inner.planner)
            .request(id, Cause::Manual, now())
            .map_err(|n| match n {
                Note::Skipped { reason, .. } => reason,
                Note::Missed { .. } => "It couldn't start.".into(),
            })?;
        self.inner.wake.notify_one();
        self.inner.runner.changed(Some(id));
        Ok(())
    }

    /// Stops a job's run and drops its waiting runs.
    pub fn stop(&self, id: &str) {
        lock(&self.inner.planner).cancel_queued(id);
        if let Some(a) = lock(&self.inner.active).get(id) {
            a.cancel.store(true, Ordering::Relaxed);
        }
        self.inner.runner.changed(Some(id));
    }

    /// Stops every run and empties the queue.
    pub fn stop_all(&self) {
        let ids: Vec<String> = lock(&self.inner.planner)
            .jobs()
            .map(|j| j.id.clone())
            .collect();
        for id in ids {
            lock(&self.inner.planner).cancel_queued(&id);
        }
        for a in lock(&self.inner.active).values() {
            a.cancel.store(true, Ordering::Relaxed);
        }
        self.inner.runner.changed(None);
    }

    pub fn set_paused(&self, paused: bool) {
        {
            let mut p = lock(&self.inner.planner);
            p.set_paused(paused, now());
            let _ = self.inner.store.save_state(p.state(), p.paused());
        }
        self.inner.wake.notify_one();
        self.inner.runner.changed(None);
    }

    /// Most runs going at once (at least 1).
    pub fn set_max_running(&self, n: usize) {
        lock(&self.inner.planner).max_running = n.max(1);
        self.inner.wake.notify_one();
    }

    pub fn paused(&self) -> bool {
        lock(&self.inner.planner).paused()
    }

    pub fn history(&self, id: &str) -> Vec<RunRecord> {
        self.inner.store.history(id)
    }

    /// Runs going now, by job id → run id.
    pub fn active(&self) -> BTreeMap<String, String> {
        lock(&self.inner.active)
            .iter()
            .map(|(j, a)| (j.clone(), a.run_id.clone()))
            .collect()
    }

    /// Runs waiting to start.
    pub fn queued(&self) -> usize {
        lock(&self.inner.planner).queue().count()
    }

    /// Asks for a check soon, e.g. after a chat ends so a waiting run can
    /// start.
    pub fn wake(&self) {
        self.inner.wake.notify_one();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::trigger::Trigger;
    use crate::watch::FileStamp;
    use std::sync::atomic::AtomicUsize;

    #[derive(Default)]
    struct Fake {
        runs: Mutex<Vec<RunRequest>>,
        files: Mutex<Snapshot>,
        busy: AtomicBool,
        changes: AtomicUsize,
        finished: Mutex<Vec<RecordStatus>>,
        /// Runs wait for the cancel flag instead of finishing.
        hang: AtomicBool,
    }

    impl JobRunner for Arc<Fake> {
        fn agent_busy(&self, _: &str) -> bool {
            self.busy.load(Ordering::Relaxed)
        }
        async fn run(&self, req: RunRequest) -> RunResult {
            lock(&self.runs).push(req.clone());
            if self.hang.load(Ordering::Relaxed) {
                while !req.cancel.load(Ordering::Relaxed) {
                    tokio::time::sleep(Duration::from_millis(20)).await;
                }
                return RunResult {
                    status: RecordStatus::Stopped,
                    ..RunResult::failed("")
                };
            }
            RunResult {
                status: RecordStatus::Completed,
                reply: format!("did {}", req.job.id),
                note: String::new(),
                llm_calls: 1,
                tool_calls: 0,
            }
        }
        fn snapshot(&self, _: &str, folder: &str) -> Result<Snapshot, String> {
            if folder == "missing" {
                return Err("not found".into());
            }
            Ok(lock(&self.files).clone())
        }
        fn changed(&self, _: Option<&str>) {
            self.changes.fetch_add(1, Ordering::Relaxed);
        }
        fn finished(&self, _: &Job, r: &RunRecord) {
            lock(&self.finished).push(r.status);
        }
    }

    fn manager() -> (Manager<Arc<Fake>>, Arc<Fake>, tempfile::TempDir) {
        let tmp = tempfile::tempdir().unwrap();
        let fake = Arc::new(Fake::default());
        let store = JobStore::open(tmp.path()).unwrap();
        let m = Manager::new(fake.clone(), store, 2, Some(TimeZone::UTC));
        (m, fake, tmp)
    }

    fn job(id: &str) -> Job {
        let mut j = Job::new(id, id, "agent");
        j.message = "go".into();
        j
    }

    async fn settle(m: &Manager<Arc<Fake>>) {
        for _ in 0..200 {
            if m.active().is_empty() && m.queued() == 0 {
                return;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        panic!("runs didn't finish");
    }

    #[tokio::test]
    async fn run_now_runs_and_records() {
        let (m, fake, _tmp) = manager();
        m.save(job("a")).unwrap();
        m.run_now("a").unwrap();
        m.step();
        settle(&m).await;
        let h = m.history("a");
        assert_eq!(h.len(), 1);
        assert_eq!(
            (h[0].status, h[0].reply.as_str()),
            (RecordStatus::Completed, "did a")
        );
        assert_eq!(h[0].cause, Cause::Manual);
        assert_eq!(*lock(&fake.finished), [RecordStatus::Completed]);
        let o = m.overview();
        assert_eq!(o.jobs[0].state, "idle");
        assert_eq!(o.jobs[0].last.as_ref().unwrap().run_id, h[0].run_id);
        assert!(fake.changes.load(Ordering::Relaxed) > 0);
        // A job that doesn't exist can't run.
        assert!(m.run_now("nope").is_err());
    }

    #[tokio::test]
    async fn busy_agents_wait_and_stopping_works() {
        let (m, fake, _tmp) = manager();
        m.save(job("a")).unwrap();
        fake.busy.store(true, Ordering::Relaxed);
        m.run_now("a").unwrap();
        m.step();
        assert_eq!(m.overview().jobs[0].state, "queued");
        assert!(m.run_now("a").is_err(), "already waiting");
        fake.busy.store(false, Ordering::Relaxed);
        fake.hang.store(true, Ordering::Relaxed);
        m.step();
        assert_eq!(m.overview().jobs[0].state, "running");
        assert!(m.overview().jobs[0].run_id.is_some());
        m.stop("a");
        settle(&m).await;
        assert_eq!(m.history("a")[0].status, RecordStatus::Stopped);
    }

    #[tokio::test(start_paused = true)]
    async fn time_limits_stop_runs() {
        let (m, fake, _tmp) = manager();
        let mut j = job("slow");
        j.time_limit_minutes = 1;
        m.save(j).unwrap();
        fake.hang.store(true, Ordering::Relaxed);
        m.run_now("slow").unwrap();
        m.step();
        tokio::time::sleep(Duration::from_secs(61)).await;
        settle(&m).await;
        let h = m.history("slow");
        assert_eq!(h[0].status, RecordStatus::Stopped);
        assert!(h[0].note.contains("time limit"), "{h:?}");
    }

    #[tokio::test]
    async fn folders_start_runs_for_settled_files() {
        let (m, fake, _tmp) = manager();
        let mut j = job("w");
        j.enabled = true;
        j.triggers = vec![Trigger::Folder {
            path: "inbox".into(),
        }];
        m.save(j).unwrap();
        let poll = |m: &Manager<Arc<Fake>>| {
            *lock(&m.inner.last_poll) = None;
            m.step();
        };
        poll(&m); // baseline
        lock(&fake.files).insert(
            "a.txt".into(),
            FileStamp {
                size: 1,
                modified_ms: 1,
            },
        );
        poll(&m); // seen, settling
        assert!(lock(&fake.runs).is_empty());
        poll(&m); // settled: runs
        settle(&m).await;
        let runs = lock(&fake.runs).clone();
        assert_eq!(runs.len(), 1);
        assert_eq!(
            runs[0].cause,
            Cause::File {
                path: "inbox/a.txt".into()
            }
        );
        // A watched folder that can't be read is reported.
        let mut j = job("w2");
        j.enabled = true;
        j.triggers = vec![Trigger::Folder {
            path: "missing".into(),
        }];
        m.save(j).unwrap();
        poll(&m);
        let o = m.overview();
        let w2 = o.jobs.iter().find(|v| v.job.id == "w2").unwrap();
        assert!(w2.watch_error.as_deref().unwrap().contains("not found"));
    }

    #[tokio::test]
    async fn pause_and_delete_are_remembered() {
        let tmp = tempfile::tempdir().unwrap();
        let fake = Arc::new(Fake::default());
        let m = Manager::new(fake.clone(), JobStore::open(tmp.path()).unwrap(), 1, None);
        let mut j = job("a");
        j.enabled = true;
        j.triggers = vec![Trigger::Every { minutes: 60 }];
        m.save(j).unwrap();
        assert!(m.overview().jobs[0].next_run.is_some());
        m.set_paused(true);
        assert!(m.overview().jobs[0].next_run.is_none());
        // Reopened: still paused, job still there.
        let m2 = Manager::new(fake.clone(), JobStore::open(tmp.path()).unwrap(), 1, None);
        assert!(m2.paused());
        assert_eq!(m2.overview().jobs.len(), 1);
        m2.delete("a").unwrap();
        assert!(m2.overview().jobs.is_empty());
        assert!(m2.history("a").is_empty());
    }
}
