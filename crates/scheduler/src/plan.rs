//! Deciding what runs when: a plain state machine, given the time.
//!
//! [`Planner`] knows the jobs, when each was last checked, the queue of runs
//! waiting to start and what's running. It never sleeps or reads the clock
//! itself, so it's tested with exact times and behaves the same wherever it's
//! driven from (the app's background task, or chummyd).
//!
//! Rules:
//! - A job runs at most once at a time. A scheduled time that comes while
//!   it's still running (or waiting to) is skipped and noted.
//! - An agent does one thing at a time: a run waits while another run, or a
//!   chat, uses the same agent.
//! - At most `max_running` runs go at once.
//! - A scheduled time more than [`GRACE_SECONDS`] in the past when it's
//!   noticed (Chummy was closed, or the computer asleep) is *missed*: skipped,
//!   or run once, as the job says.
//! - Folder triggers queue one run per changed file, at most
//!   [`MAX_FILE_RUNS_PER_HOUR`] an hour per job.

use std::collections::{BTreeMap, VecDeque};

use jiff::Timestamp;
use jiff::tz::TimeZone;
use serde::{Deserialize, Serialize};

use crate::job::{Job, Missed};

/// How late a scheduled run may start and still count as on time.
pub const GRACE_SECONDS: i64 = 600;
/// Most runs a job's folder triggers can start in an hour.
pub const MAX_FILE_RUNS_PER_HOUR: usize = 30;
/// Most runs waiting in the queue for one job.
pub const MAX_QUEUED_PER_JOB: usize = 50;

/// What the planner remembers about a job between launches.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct JobState {
    /// Scheduled times up to here have been dealt with (unix seconds).
    pub checked_until: i64,
    /// Repeating schedules count from here (when the job was switched on).
    pub anchor: i64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_run: Option<i64>,
}

impl JobState {
    fn fresh(now: i64) -> Self {
        Self {
            checked_until: now,
            anchor: now,
            last_run: None,
        }
    }
}

/// Why a run started.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Cause {
    /// Its scheduled time came (unix seconds).
    Schedule { due: i64 },
    /// A scheduled time was missed and the job runs once to catch up.
    CatchUp { due: i64 },
    /// Started by hand.
    Manual,
    /// A file in a watched folder was added or changed.
    File { path: String },
}

impl Cause {
    pub fn describe(&self) -> String {
        match self {
            Cause::Schedule { .. } => "On schedule".into(),
            Cause::CatchUp { .. } => "Catching up on a missed time".into(),
            Cause::Manual => "Started by hand".into(),
            Cause::File { path } => format!("{path} changed"),
        }
    }
}

/// A run waiting to start.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Start {
    pub job: String,
    pub agent: String,
    pub cause: Cause,
    pub queued_at: i64,
}

/// Something that didn't run, for the job's history.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Note {
    Missed {
        job: String,
        due: i64,
    },
    Skipped {
        job: String,
        cause: Cause,
        reason: String,
    },
}

#[derive(Debug)]
pub struct Planner {
    jobs: BTreeMap<String, Job>,
    state: BTreeMap<String, JobState>,
    queue: VecDeque<Start>,
    /// Job id → agent, for jobs running now.
    running: BTreeMap<String, String>,
    file_runs: BTreeMap<String, VecDeque<i64>>,
    pub max_running: usize,
    paused: bool,
}

impl Planner {
    pub fn new(state: BTreeMap<String, JobState>, max_running: usize) -> Self {
        Self {
            jobs: BTreeMap::new(),
            state,
            queue: VecDeque::new(),
            running: BTreeMap::new(),
            file_runs: BTreeMap::new(),
            max_running: max_running.max(1),
            paused: false,
        }
    }

    pub fn job(&self, id: &str) -> Option<&Job> {
        self.jobs.get(id)
    }

    pub fn jobs(&self) -> impl Iterator<Item = &Job> {
        self.jobs.values()
    }

    pub fn state(&self) -> &BTreeMap<String, JobState> {
        &self.state
    }

    pub fn paused(&self) -> bool {
        self.paused
    }

    /// Pausing stops scheduled and folder runs from starting; times that
    /// pass while paused are skipped, not caught up. Runs started by hand
    /// still go.
    pub fn set_paused(&mut self, paused: bool, now: i64) {
        if paused && !self.paused {
            self.queue.retain(|s| s.cause == Cause::Manual);
        }
        if !paused && self.paused {
            for st in self.state.values_mut() {
                st.checked_until = st.checked_until.max(now);
            }
        }
        self.paused = paused;
    }

    /// Replaces the job list (after a load or an edit). New jobs and jobs
    /// just switched on start counting from `now`, so they don't catch up
    /// on the past. Waiting runs of removed jobs are dropped, and scheduled
    /// ones of switched-off jobs too.
    pub fn set_jobs(&mut self, jobs: Vec<Job>, now: i64) {
        let mut next = BTreeMap::new();
        for job in jobs {
            let old = self.jobs.get(&job.id);
            let switched_on = job.enabled && old.is_none_or(|o| !o.enabled);
            let rescheduled = old.is_some_and(|o| o.triggers != job.triggers);
            let st = self
                .state
                .entry(job.id.clone())
                .or_insert_with(|| JobState::fresh(now));
            if old.is_some() && (switched_on || rescheduled) {
                st.checked_until = now;
                st.anchor = now;
            }
            next.insert(job.id.clone(), job);
        }
        self.state.retain(|id, _| next.contains_key(id));
        self.queue.retain(|s| match next.get(&s.job) {
            None => false,
            Some(j) => j.enabled || s.cause == Cause::Manual,
        });
        for s in self.queue.iter_mut() {
            if let Some(j) = next.get(&s.job) {
                s.agent = j.agent.clone();
            }
        }
        self.file_runs.retain(|id, _| next.contains_key(id));
        self.jobs = next;
    }

    fn busy(&self, job: &str) -> bool {
        self.running.contains_key(job) || self.queue.iter().any(|s| s.job == job)
    }

    pub fn is_running(&self, job: &str) -> bool {
        self.running.contains_key(job)
    }

    pub fn is_queued(&self, job: &str) -> bool {
        self.queue.iter().any(|s| s.job == job)
    }

    /// Queues the runs whose time has come. Returns what was skipped or
    /// missed.
    pub fn tick(&mut self, now: i64, tz: &TimeZone) -> Vec<Note> {
        let mut notes = Vec::new();
        let ids: Vec<String> = self.jobs.keys().cloned().collect();
        for id in ids {
            let job = &self.jobs[&id];
            let st = self
                .state
                .entry(id.clone())
                .or_insert_with(|| JobState::fresh(now));
            if st.checked_until > now {
                // The clock went back; start again from now.
                st.checked_until = now;
                continue;
            }
            if !job.enabled || self.paused {
                st.checked_until = now;
                continue;
            }
            let (Ok(from), Ok(anchor)) = (
                Timestamp::from_second(st.checked_until),
                Timestamp::from_second(st.anchor),
            ) else {
                st.checked_until = now;
                continue;
            };
            let due = job
                .triggers
                .iter()
                .filter_map(|t| t.next_after(from, anchor, tz))
                .map(|t| t.as_second())
                .filter(|&t| t <= now)
                .min();
            st.checked_until = now;
            let Some(due) = due else { continue };
            let late = now - due > GRACE_SECONDS;
            if late && job.missed == Missed::Skip {
                notes.push(Note::Missed { job: id, due });
                continue;
            }
            let cause = if late {
                Cause::CatchUp { due }
            } else {
                Cause::Schedule { due }
            };
            if self.busy(&id) {
                notes.push(Note::Skipped {
                    job: id,
                    cause,
                    reason: "It was still running from an earlier time.".into(),
                });
                continue;
            }
            let agent = job.agent.clone();
            self.queue.push_back(Start {
                job: id,
                agent,
                cause,
                queued_at: now,
            });
        }
        notes
    }

    /// Queues a run started by hand or by a changed file.
    pub fn request(&mut self, job_id: &str, cause: Cause, now: i64) -> Result<(), Note> {
        let skipped = |reason: &str| Note::Skipped {
            job: job_id.to_string(),
            cause: cause.clone(),
            reason: reason.to_string(),
        };
        let Some(job) = self.jobs.get(job_id) else {
            return Err(skipped("The job no longer exists."));
        };
        match &cause {
            Cause::File { path } => {
                if !job.enabled || self.paused {
                    return Err(skipped("The job is switched off or paused."));
                }
                if self
                    .queue
                    .iter()
                    .any(|s| s.job == job_id && s.cause == cause)
                {
                    return Ok(());
                }
                if self.queue.iter().filter(|s| s.job == job_id).count() >= MAX_QUEUED_PER_JOB {
                    return Err(skipped("Too many files are already waiting for this job."));
                }
                let recent = self.file_runs.entry(job_id.to_string()).or_default();
                while recent.front().is_some_and(|&t| now - t >= 3600) {
                    recent.pop_front();
                }
                if recent.len() >= MAX_FILE_RUNS_PER_HOUR {
                    return Err(skipped(&format!(
                        "Files changed too often: folder triggers start at most {MAX_FILE_RUNS_PER_HOUR} runs an hour. {path} was not run."
                    )));
                }
                recent.push_back(now);
            }
            _ => {
                if self.busy(job_id) {
                    return Err(skipped("It's already running or waiting to run."));
                }
            }
        }
        let agent = job.agent.clone();
        self.queue.push_back(Start {
            job: job_id.to_string(),
            agent,
            cause,
            queued_at: now,
        });
        Ok(())
    }

    /// The next run that can start now, if any, marked as running.
    /// `agent_busy` says whether an agent is busy outside the scheduler
    /// (e.g. answering a chat).
    pub fn next_start(&mut self, agent_busy: &dyn Fn(&str) -> bool) -> Option<Start> {
        if self.running.len() >= self.max_running {
            return None;
        }
        let pos = self.queue.iter().position(|s| {
            !self.running.contains_key(&s.job)
                && !self.running.values().any(|a| *a == s.agent)
                && !agent_busy(&s.agent)
        })?;
        let start = self.queue.remove(pos)?;
        self.running.insert(start.job.clone(), start.agent.clone());
        Some(start)
    }

    /// A run ended.
    pub fn finished(&mut self, job: &str, now: i64) {
        self.running.remove(job);
        if let Some(st) = self.state.get_mut(job) {
            st.last_run = Some(now);
        }
    }

    /// Drops a job's waiting runs. Returns how many.
    pub fn cancel_queued(&mut self, job: &str) -> usize {
        let before = self.queue.len();
        self.queue.retain(|s| s.job != job);
        before - self.queue.len()
    }

    /// Runs waiting to start, oldest first.
    pub fn queue(&self) -> impl Iterator<Item = &Start> {
        self.queue.iter()
    }

    /// When the job next runs on its schedule, if it does.
    pub fn next_run(&self, job_id: &str, now: i64, tz: &TimeZone) -> Option<i64> {
        let job = self.jobs.get(job_id)?;
        if !job.enabled || self.paused {
            return None;
        }
        let st = self.state.get(job_id)?;
        let from = Timestamp::from_second(st.checked_until.max(now)).ok()?;
        let anchor = Timestamp::from_second(st.anchor).ok()?;
        job.triggers
            .iter()
            .filter_map(|t| t.next_after(from, anchor, tz))
            .map(|t| t.as_second())
            .min()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::trigger::Trigger;
    use jiff::civil::DateTime;

    fn ny() -> TimeZone {
        TimeZone::get("America/New_York").unwrap()
    }

    fn at(s: &str) -> i64 {
        s.parse::<DateTime>()
            .unwrap()
            .to_zoned(ny())
            .unwrap()
            .timestamp()
            .as_second()
    }

    fn job(id: &str, agent: &str, triggers: Vec<Trigger>) -> Job {
        let mut j = Job::new(id, id, agent);
        j.enabled = true;
        j.message = "go".into();
        j.triggers = triggers;
        j
    }

    fn daily(time: &str) -> Trigger {
        Trigger::Daily {
            time: time.into(),
            days: vec![],
        }
    }

    fn not_busy(_: &str) -> bool {
        false
    }

    #[test]
    fn runs_on_time_once() {
        let mut p = Planner::new(BTreeMap::new(), 1);
        p.set_jobs(
            vec![job("a", "x", vec![daily("09:00")])],
            at("2026-11-22T08:00"),
        );
        assert!(p.tick(at("2026-11-22T08:59"), &ny()).is_empty());
        assert!(p.next_start(&not_busy).is_none());
        assert_eq!(
            p.next_run("a", at("2026-11-22T08:59"), &ny()),
            Some(at("2026-11-22T09:00"))
        );
        p.tick(at("2026-11-22T09:00"), &ny());
        let s = p.next_start(&not_busy).unwrap();
        assert_eq!(
            s.cause,
            Cause::Schedule {
                due: at("2026-11-22T09:00")
            }
        );
        // The same time isn't queued again.
        p.tick(at("2026-11-22T09:01"), &ny());
        p.finished("a", at("2026-11-22T09:05"));
        assert!(p.next_start(&not_busy).is_none());
        assert_eq!(p.state()["a"].last_run, Some(at("2026-11-22T09:05")));
    }

    #[test]
    fn missed_times_are_skipped_or_caught_up_once() {
        let mut p = Planner::new(BTreeMap::new(), 1);
        let mut catch = job("catch", "y", vec![daily("09:00")]);
        catch.missed = Missed::RunOnce;
        p.set_jobs(
            vec![job("skip", "x", vec![daily("09:00")]), catch],
            at("2026-11-20T08:00"),
        );
        // Two days later (Chummy was closed): two 09:00s passed.
        let notes = p.tick(at("2026-11-22T12:00"), &ny());
        assert_eq!(
            notes,
            [Note::Missed {
                job: "skip".into(),
                due: at("2026-11-20T09:00")
            }]
        );
        let s = p.next_start(&not_busy).unwrap();
        assert_eq!(
            (s.job.as_str(), s.cause.clone()),
            (
                "catch",
                Cause::CatchUp {
                    due: at("2026-11-20T09:00")
                }
            )
        );
        p.finished("catch", at("2026-11-22T12:01"));
        p.tick(at("2026-11-22T12:02"), &ny());
        assert!(p.next_start(&not_busy).is_none(), "caught up only once");
        // A little late (within the grace period) is still on time.
        let notes = p.tick(at("2026-11-23T09:05"), &ny());
        assert!(notes.is_empty());
        assert_eq!(p.queue().count(), 2);
    }

    #[test]
    fn one_run_per_job_and_per_agent() {
        let mut p = Planner::new(BTreeMap::new(), 3);
        p.set_jobs(
            vec![
                job("a", "x", vec![Trigger::Every { minutes: 5 }]),
                job("b", "x", vec![Trigger::Every { minutes: 5 }]),
                job("c", "z", vec![Trigger::Every { minutes: 5 }]),
            ],
            at("2026-11-22T08:00"),
        );
        p.tick(at("2026-11-22T08:05"), &ny());
        let first = p.next_start(&not_busy).unwrap();
        assert_eq!(first.job, "a");
        // b shares a's agent, so c goes next.
        assert_eq!(p.next_start(&not_busy).unwrap().job, "c");
        assert!(p.next_start(&not_busy).is_none());
        // a's next time comes while it's still running: skipped.
        let notes = p.tick(at("2026-11-22T08:10"), &ny());
        assert!(
            notes
                .iter()
                .any(|n| matches!(n, Note::Skipped { job, .. } if job == "a"))
        );
        assert!(
            notes
                .iter()
                .any(|n| matches!(n, Note::Skipped { job, .. } if job == "b"))
        );
        p.finished("a", at("2026-11-22T08:11"));
        assert_eq!(p.next_start(&not_busy).unwrap().job, "b");
        // An agent busy chatting holds its runs back.
        p.finished("b", at("2026-11-22T08:12"));
        p.request("a", Cause::Manual, at("2026-11-22T08:12"))
            .unwrap();
        assert!(p.next_start(&|a| a == "x").is_none());
        assert_eq!(p.next_start(&not_busy).unwrap().job, "a");
    }

    #[test]
    fn concurrency_limit() {
        let mut p = Planner::new(BTreeMap::new(), 1);
        p.set_jobs(
            vec![job("a", "x", vec![]), job("b", "y", vec![])],
            at("2026-11-22T08:00"),
        );
        p.request("a", Cause::Manual, 0).unwrap();
        p.request("b", Cause::Manual, 0).unwrap();
        assert!(p.request("a", Cause::Manual, 0).is_err(), "already waiting");
        assert!(p.next_start(&not_busy).is_some());
        assert!(p.next_start(&not_busy).is_none(), "only one at a time");
    }

    #[test]
    fn switching_on_and_editing_start_from_now() {
        let mut p = Planner::new(BTreeMap::new(), 1);
        let mut j = job("a", "x", vec![daily("09:00")]);
        j.enabled = false;
        p.set_jobs(vec![j.clone()], at("2026-11-20T08:00"));
        p.tick(at("2026-11-20T12:00"), &ny());
        // Switched on two days later: no catching up on the time it was off.
        j.enabled = true;
        j.missed = Missed::RunOnce;
        p.set_jobs(vec![j.clone()], at("2026-11-22T12:00"));
        assert!(p.tick(at("2026-11-22T12:01"), &ny()).is_empty());
        assert!(p.next_start(&not_busy).is_none());
        // Editing the schedule resets the "every" anchor.
        j.triggers = vec![Trigger::Every { minutes: 30 }];
        p.set_jobs(vec![j.clone()], at("2026-11-22T12:10"));
        assert_eq!(
            p.next_run("a", at("2026-11-22T12:10"), &ny()),
            Some(at("2026-11-22T12:40"))
        );
        // Removing a job drops its queue and state.
        p.request("a", Cause::Manual, 0).unwrap();
        p.set_jobs(vec![], 0);
        assert_eq!(p.queue().count(), 0);
        assert!(p.state().is_empty());
    }

    #[test]
    fn pausing_and_clock_changes() {
        let mut p = Planner::new(BTreeMap::new(), 1);
        p.set_jobs(
            vec![job("a", "x", vec![daily("09:00")])],
            at("2026-11-22T08:00"),
        );
        p.set_paused(true, at("2026-11-22T08:30"));
        assert!(p.tick(at("2026-11-22T09:00"), &ny()).is_empty());
        assert!(p.next_start(&not_busy).is_none());
        assert!(p.next_run("a", at("2026-11-22T09:00"), &ny()).is_none());
        // Hand-started runs still go while paused.
        p.request("a", Cause::Manual, at("2026-11-22T09:01"))
            .unwrap();
        assert!(p.next_start(&not_busy).is_some());
        p.finished("a", at("2026-11-22T09:02"));
        p.set_paused(false, at("2026-11-22T10:00"));
        assert!(
            p.tick(at("2026-11-22T10:00"), &ny()).is_empty(),
            "no catch-up after a pause"
        );
        // The clock jumping back doesn't run anything twice.
        p.tick(at("2026-11-22T07:00"), &ny());
        assert!(p.tick(at("2026-11-22T08:00"), &ny()).is_empty());
        assert_eq!(p.state()["a"].checked_until, at("2026-11-22T08:00"));
    }

    #[test]
    fn folder_runs_queue_per_file_and_are_rate_limited() {
        let mut p = Planner::new(BTreeMap::new(), 1);
        p.set_jobs(
            vec![job("w", "x", vec![Trigger::Folder { path: "in".into() }])],
            0,
        );
        let file = |n: usize| Cause::File {
            path: format!("in/{n}.txt"),
        };
        p.request("w", file(0), 100).unwrap();
        p.request("w", file(0), 100).unwrap();
        p.request("w", file(1), 100).unwrap();
        assert_eq!(p.queue().count(), 2, "same file isn't queued twice");
        for n in 2..MAX_FILE_RUNS_PER_HOUR {
            p.request("w", file(n), 200).unwrap();
        }
        assert!(p.request("w", file(999), 300).is_err());
        // An hour later there's room again.
        assert!(p.request("w", file(1000), 100 + 3600).is_ok());
        // Folder runs need the job switched on.
        let mut off = job("w", "x", vec![]);
        off.enabled = false;
        p.set_jobs(vec![off], 0);
        assert!(p.request("w", file(5), 9999).is_err());
    }
}
