//! Jobs on disk, in one folder:
//!
//! ```text
//! jobs/
//!   <id>.json            job definitions (chummy.job)
//!   state.json           when each job was last checked and run
//!   history/<id>.json    the job's recent runs, newest last
//! ```
//!
//! The Chummy app keeps this folder in the Chummy folder; chummyd keeps it in
//! its own base folder. Every write is atomic.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

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

#[derive(Debug, Default, Serialize, Deserialize)]
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
        })
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
        jobs.sort_by(|a, b| {
            a.name
                .to_lowercase()
                .cmp(&b.name.to_lowercase())
                .then(a.id.cmp(&b.id))
        });
        (jobs, bad)
    }

    pub fn load(&self, id: &str) -> Result<Job, JobError> {
        let text = std::fs::read_to_string(self.job_path(id)?)
            .map_err(|e| JobError::Parse(e.to_string()))?;
        Job::from_json(&text)
    }

    /// Checks and saves a job.
    pub fn save(&self, job: &Job) -> Result<(), JobError> {
        job.check()?;
        let mut text = job.to_json();
        text.push('\n');
        write_atomic(&self.job_path(&job.id)?, text.as_bytes(), false)
            .map_err(|e| JobError::Invalid(format!("The job couldn't be saved: {e}")))
    }

    /// Deletes a job and its history.
    pub fn delete(&self, id: &str) -> Result<(), JobError> {
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
        self.history_doc(id)
            .ok()
            .and_then(|d| d.load(|v| v).ok())
            .map(|l| l.value.runs)
            .unwrap_or_default()
    }

    /// Adds or updates (by run id) a record, keeping the newest
    /// [`HISTORY_LEN`].
    pub fn record(&self, id: &str, mut r: RunRecord) -> Result<(), String> {
        let doc = self.history_doc(id).map_err(|e| e.to_string())?;
        let mut h = doc.load(|v| v).map(|l| l.value).unwrap_or_default();
        r.reply = truncate(&r.reply, HISTORY_REPLY_CHARS);
        r.note = truncate(&r.note, 2000);
        h.version = 1;
        match h.runs.iter_mut().find(|x| x.run_id == r.run_id) {
            Some(x) => *x = r,
            None => h.runs.push(r),
        }
        if h.runs.len() > HISTORY_LEN {
            let extra = h.runs.len() - HISTORY_LEN;
            h.runs.drain(..extra);
        }
        doc.save(&h).map_err(|e| e.to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

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
    }

    #[test]
    fn state_round_trips() {
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
