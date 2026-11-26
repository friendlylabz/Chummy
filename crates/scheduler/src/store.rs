//! Jobs on disk, in one folder:
//!
//! ```text
//! jobs/
//!   <id>.json            job definitions (chummy.job)
//!   state.json           when each job was last checked and run
//!   history/<id>.json    the job's recent runs, newest last
//! ```
//!
//! The Chummy app keeps this folder in the Chummy folder. Every write is
//! atomic. chummyd keeps jobs in memory instead ([`JobStore::in_memory`]):
//! the jobs come from its bundle, and their state and history are saved in
//! its sealed state ([`JobStore::snapshot`]).

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use chummy_store::{JsonDoc, write_atomic};
use serde::{Deserialize, Serialize};

use crate::job::{Job, JobError, valid_id};
use crate::plan::{Cause, JobState};

/// Runs kept in each job's history.
pub const HISTORY_LEN: usize = 100;
/// Longest reply kept in a history record, in characters.
pub const HISTORY_REPLY_CHARS: usize = 8000;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RecordStatus {
    Completed,
    Failed,
    Stopped,
    /// A loop limit (steps, model calls, tool calls) was reached.
    Limit,
    /// A scheduled time passed while Chummy wasn't running.
    Missed,
    /// Not run, e.g. because it was still running from before.
    Skipped,
}

/// One run of a job, or one time it didn't run.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RunRecord {
    pub run_id: String,
    pub cause: Cause,
    pub status: RecordStatus,
    /// Unix seconds.
    pub started_at: i64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub finished_at: Option<i64>,
    /// The loop's reply (cut to [`HISTORY_REPLY_CHARS`]).
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub reply: String,
    /// What went wrong, or why it didn't run.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub note: String,
    #[serde(default)]
    pub llm_calls: u32,
    #[serde(default)]
    pub tool_calls: u32,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
struct StateDoc {
    version: u32,
    paused: bool,
    jobs: BTreeMap<String, JobState>,
}

#[derive(Debug, Default, Serialize, Deserialize)]
#[serde(default)]
struct HistoryDoc {
    version: u32,
    runs: Vec<RunRecord>,
}

/// A job file that couldn't be read.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct BadJobFile {
    pub file: String,
    pub error: String,
}

#[derive(Debug, Clone)]
pub struct JobStore {
    dir: PathBuf,
    /// Set for a store kept in memory (then `dir` is empty).
    memory: Option<Arc<Mutex<Memory>>>,
}

/// A store kept in memory.
#[derive(Debug, Default)]
struct Memory {
    jobs: BTreeMap<String, Job>,
    saved: Saved,
}

/// What a store in memory saves elsewhere: job state and history (the jobs
/// themselves come from elsewhere).
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
struct Saved {
    state: StateDoc,
    history: BTreeMap<String, Vec<RunRecord>>,
}

fn truncate(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        s.to_string()
    } else {
        s.chars().take(max).collect::<String>() + "…"
    }
}

impl JobStore {
    /// Uses (and creates) `dir`.
    pub fn open(dir: &Path) -> std::io::Result<Self> {
        std::fs::create_dir_all(dir.join("history"))?;
        Ok(Self {
            dir: dir.to_path_buf(),
            memory: None,
        })
    }

    /// A store in memory holding `jobs`, carrying on the state and history
    /// in `snapshot` (from [`JobStore::snapshot`]) for the jobs still there.
    /// Something that doesn't read as a snapshot starts afresh.
    pub fn in_memory(jobs: Vec<Job>, snapshot: serde_json::Value) -> Self {
        let mut saved: Saved = serde_json::from_value(snapshot).unwrap_or_default();
        let jobs: BTreeMap<String, Job> = jobs.into_iter().map(|j| (j.id.clone(), j)).collect();
        saved.state.jobs.retain(|id, _| jobs.contains_key(id));
        saved.history.retain(|id, _| jobs.contains_key(id));
        Self {
            dir: PathBuf::new(),
            memory: Some(Arc::new(Mutex::new(Memory { jobs, saved }))),
        }
    }

    /// A store in memory's state and history, to save elsewhere; `Null`
    /// for a store in a folder.
    pub fn snapshot(&self) -> serde_json::Value {
        match &self.memory {
            Some(m) => serde_json::to_value(&lock(m).saved).unwrap_or_default(),
            None => serde_json::Value::Null,
        }
    }

    pub fn dir(&self) -> &Path {
        &self.dir
    }

    fn job_path(&self, id: &str) -> Result<PathBuf, JobError> {
        if !valid_id(id) {
            return Err(JobError::Invalid("The job's id isn't valid.".into()));
        }
        Ok(self.dir.join(format!("{id}.json")))
    }

    fn history_doc(&self, id: &str) -> Result<JsonDoc<HistoryDoc>, JobError> {
        if !valid_id(id) {
            return Err(JobError::Invalid("The job's id isn't valid.".into()));
        }
        Ok(JsonDoc::new(
            self.dir.join("history").join(format!("{id}.json")),
        ))
    }

    fn state_doc(&self) -> JsonDoc<StateDoc> {
        JsonDoc::new(self.dir.join("state.json"))
    }

    /// Every job, sorted by name, plus files that couldn't be read. A file
    /// whose name doesn't match its id is reported, not loaded.
    pub fn list(&self) -> (Vec<Job>, Vec<BadJobFile>) {
        if let Some(m) = &self.memory {
            let mut jobs: Vec<Job> = lock(m).jobs.values().cloned().collect();
            sort_jobs(&mut jobs);
            return (jobs, vec![]);
        }
        let mut jobs = Vec::new();
        let mut bad = Vec::new();
        let entries = std::fs::read_dir(&self.dir).into_iter().flatten().flatten();
        for e in entries {
            let p = e.path();
            let name = e.file_name().to_string_lossy().to_string();
            let Some(stem) = name.strip_suffix(".json") else {
                continue;
            };
            if stem == "state" || !p.is_file() || stem.contains(".json") {
                continue;
            }
            let loaded = std::fs::metadata(&p)
                .map_err(|e| e.to_string())
                .and_then(|m| {
                    if m.len() > crate::job::MAX_FILE_BYTES as u64 {
                        Err(JobError::TooLarge.to_string())
                    } else {
                        std::fs::read_to_string(&p).map_err(|e| e.to_string())
                    }
                })
                .and_then(|t| Job::from_json(&t).map_err(|e| e.to_string()))
                .and_then(|j| {
                    if j.id == stem {
                        Ok(j)
                    } else {
                        Err(format!("its id “{}” doesn't match the file name", j.id))
                    }
                });
            match loaded {
                Ok(j) => jobs.push(j),
                Err(error) => bad.push(BadJobFile { file: name, error }),
            }
        }
        sort_jobs(&mut jobs);
        (jobs, bad)
    }

    pub fn load(&self, id: &str) -> Result<Job, JobError> {
        if let Some(m) = &self.memory {
            return lock(m)
                .jobs
                .get(id)
                .cloned()
                .ok_or_else(|| JobError::Parse("there's no such job".into()));
        }
        let text = std::fs::read_to_string(self.job_path(id)?)
            .map_err(|e| JobError::Parse(e.to_string()))?;
        Job::from_json(&text)
    }

    /// Checks and saves a job.
    pub fn save(&self, job: &Job) -> Result<(), JobError> {
        job.check()?;
        if let Some(m) = &self.memory {
            lock(m).jobs.insert(job.id.clone(), job.clone());
            return Ok(());
        }
        let mut text = job.to_json();
        text.push('\n');
        write_atomic(&self.job_path(&job.id)?, text.as_bytes(), false)
            .map_err(|e| JobError::Invalid(format!("The job couldn't be saved: {e}")))
    }

    /// Deletes a job and its history.
    pub fn delete(&self, id: &str) -> Result<(), JobError> {
        if let Some(m) = &self.memory {
            let mut m = lock(m);
            m.jobs.remove(id);
            m.saved.history.remove(id);
            return Ok(());
        }
        let path = self.job_path(id)?;
        match std::fs::remove_file(&path) {
            Ok(()) => {}
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            Err(e) => {
                return Err(JobError::Invalid(format!(
                    "The job couldn't be deleted: {e}"
                )));
            }
        }
        let h = self.history_doc(id)?;
        let _ = std::fs::remove_file(h.path());
        let mut bak = h.path().as_os_str().to_os_string();
        bak.push(".bak");
        let _ = std::fs::remove_file(PathBuf::from(bak));
        Ok(())
    }

    /// Saved job states and whether jobs are paused.
    pub fn state(&self) -> (BTreeMap<String, JobState>, bool) {
        if let Some(m) = &self.memory {
            let m = lock(m);
            return (m.saved.state.jobs.clone(), m.saved.state.paused);
        }
        let doc = self
            .state_doc()
            .load(|v| v)
            .map(|l| l.value)
            .unwrap_or_default();
        (doc.jobs, doc.paused)
    }

    pub fn save_state(
        &self,
        jobs: &BTreeMap<String, JobState>,
        paused: bool,
    ) -> Result<(), String> {
        if let Some(m) = &self.memory {
            lock(m).saved.state = StateDoc {
                version: 1,
                paused,
                jobs: jobs.clone(),
            };
            return Ok(());
        }
        self.state_doc()
            .save(&StateDoc {
                version: 1,
                paused,
                jobs: jobs.clone(),
            })
            .map_err(|e| e.to_string())
    }

    /// The job's recent runs, newest last.
    pub fn history(&self, id: &str) -> Vec<RunRecord> {
        if let Some(m) = &self.memory {
            return lock(m).saved.history.get(id).cloned().unwrap_or_default();
        }
        self.history_doc(id)
            .ok()
            .and_then(|d| d.load(|v| v).ok())
            .map(|l| l.value.runs)
            .unwrap_or_default()
    }

    /// Adds or updates (by run id) a record, keeping the newest
    /// [`HISTORY_LEN`].
    pub fn record(&self, id: &str, mut r: RunRecord) -> Result<(), String> {
        r.reply = truncate(&r.reply, HISTORY_REPLY_CHARS);
        r.note = truncate(&r.note, 2000);
        if let Some(m) = &self.memory {
            let mut m = lock(m);
            if !m.jobs.contains_key(id) {
                return Err("there's no such job".into());
            }
            add_record(m.saved.history.entry(id.to_string()).or_default(), r);
            return Ok(());
        }
        let doc = self.history_doc(id).map_err(|e| e.to_string())?;
        let mut h = doc.load(|v| v).map(|l| l.value).unwrap_or_default();
        h.version = 1;
        add_record(&mut h.runs, r);
        doc.save(&h).map_err(|e| e.to_string())
    }
}

/// Adds or updates (by run id) a record, keeping the newest [`HISTORY_LEN`].
fn add_record(runs: &mut Vec<RunRecord>, r: RunRecord) {
    match runs.iter_mut().find(|x| x.run_id == r.run_id) {
        Some(x) => *x = r,
        None => runs.push(r),
    }
    if runs.len() > HISTORY_LEN {
        let extra = runs.len() - HISTORY_LEN;
        runs.drain(..extra);
    }
}

fn sort_jobs(jobs: &mut [Job]) {
    jobs.sort_by(|a, b| {
        a.name
            .to_lowercase()
            .cmp(&b.name.to_lowercase())
            .then(a.id.cmp(&b.id))
    });
}

fn lock<T>(m: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|e| e.into_inner())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_store_in_memory_keeps_state_and_history_for_its_snapshot() {
        let mut j = Job::new("digest", "Digest", "a1");
        j.message = "hi".into();
        let store = JobStore::in_memory(vec![j.clone()], serde_json::Value::Null);
        assert_eq!(store.list().0, [j.clone()]);
        assert_eq!(store.load("digest").unwrap(), j);
        store.record("digest", rec(1)).unwrap();
        assert!(store.record("gone", rec(2)).is_err());
        let mut st = BTreeMap::new();
        let js = JobState {
            checked_until: 1,
            anchor: 1,
            last_run: None,
        };
        st.insert("digest".to_string(), js);
        st.insert("gone".to_string(), js);
        store.save_state(&st, true).unwrap();
        let snap = store.snapshot();

        // Carried on with the same jobs; a job no longer there is dropped.
        let again = JobStore::in_memory(vec![j.clone()], snap.clone());
        assert_eq!(again.history("digest").len(), 1);
        assert_eq!(
            again.history("digest")[0].reply.chars().count(),
            HISTORY_REPLY_CHARS + 1
        );
        let (state, paused) = again.state();
        assert!(paused && state.contains_key("digest") && !state.contains_key("gone"));
        let other = JobStore::in_memory(vec![], snap);
        assert!(other.history("digest").is_empty() && other.state().0.is_empty());
        // A store in a folder has no snapshot.
        let tmp = tempfile::tempdir().unwrap();
        assert!(JobStore::open(tmp.path()).unwrap().snapshot().is_null());
    }

    fn rec(n: usize) -> RunRecord {
        RunRecord {
            run_id: format!("r{n}"),
            cause: Cause::Manual,
            status: RecordStatus::Completed,
            started_at: n as i64,
            finished_at: None,
            reply: "x".repeat(HISTORY_REPLY_CHARS + 10),
            note: String::new(),
            llm_calls: 1,
            tool_calls: 0,
        }
    }

    #[test]
    fn saves_lists_and_deletes() {
        let tmp = tempfile::tempdir().unwrap();
        let store = JobStore::open(&tmp.path().join("jobs")).unwrap();
        let mut j = Job::new("digest", "Digest", "a1");
        j.message = "hi".into();
        store.save(&j).unwrap();
        let mut bad = Job::new("bad", "", "a1");
        bad.message = "hi".into();
        assert!(store.save(&bad).is_err(), "checked before saving");
        // A broken file and a renamed file are reported, not loaded.
        std::fs::write(store.dir().join("broken.json"), "{").unwrap();
        std::fs::write(store.dir().join("other.json"), j.to_json()).unwrap();
        let (jobs, problems) = store.list();
        assert_eq!(jobs, [j.clone()]);
        assert_eq!(problems.len(), 2);
        assert_eq!(store.load("digest").unwrap(), j);
        assert!(store.load("../settings").is_err());

        for n in 0..HISTORY_LEN + 5 {
            store.record("digest", rec(n)).unwrap();
        }
        let h = store.history("digest");
        assert_eq!(h.len(), HISTORY_LEN);
        assert_eq!(h[0].run_id, "r5");
        assert!(h[0].reply.chars().count() <= HISTORY_REPLY_CHARS + 1);
        // Updating a record keeps its place.
        let mut r = rec(50);
        r.status = RecordStatus::Failed;
        store.record("digest", r).unwrap();
        let h = store.history("digest");
        assert_eq!(h.len(), HISTORY_LEN);
        assert_eq!(
            h.iter().find(|x| x.run_id == "r50").unwrap().status,
            RecordStatus::Failed
        );

        store.delete("digest").unwrap();
        assert!(store.list().0.is_empty());
        assert!(store.history("digest").is_empty());
        store.delete("digest").unwrap();

        // (was state_round_trips)
        {
            let tmp = tempfile::tempdir().unwrap();
            let store = JobStore::open(tmp.path()).unwrap();
            assert_eq!(store.state(), (BTreeMap::new(), false));
            let mut m = BTreeMap::new();
            m.insert(
                "a".to_string(),
                JobState {
                    checked_until: 5,
                    anchor: 1,
                    last_run: Some(3),
                },
            );
            store.save_state(&m, true).unwrap();
            assert_eq!(store.state(), (m, true));
        }
    }
}
