//! # chummy-scheduler
//!
//! Jobs: a user-configured agent loop, fed an input from some source, run on a
//! trigger.
//!
//! Status: pass 1 defines job types. Triggers, dispatch and input readers arrive
//! in pass 4. Input sources still go through the sandbox: a file source must be
//! inside the workspace, and a webpage source needs a network grant.

use serde::{Deserialize, Serialize};

/// A scheduled job definition.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Job {
    pub id: String,
    pub name: String,
    /// Id of the loop in the library to run.
    pub loop_id: String,
    pub input: InputSource,
    pub trigger: Trigger,
    #[serde(default)]
    pub enabled: bool,
}

/// Where a job's input comes from.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum InputSource {
    /// Fixed text typed into the job.
    Text { text: String },
    /// A file inside the workspace (path relative to the workspace root).
    File { path: String },
    /// Every new or changed file in a workspace folder.
    FolderWatch { path: String },
    /// A web page, fetched as text. Requires a network grant.
    Webpage { url: String },
}

/// When a job runs.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Trigger {
    /// Only when started from the dispatch screen.
    Manual,
    /// Every `seconds` seconds.
    Interval { seconds: u64 },
    /// Standard 5-field cron expression, local time.
    Cron { expr: String },
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn job_defaults_to_disabled() {
        let json = r#"{ "id": "j1", "name": "Digest", "loop_id": "summarize",
                        "input": { "kind": "webpage", "url": "https://example.com" },
                        "trigger": { "kind": "interval", "seconds": 3600 } }"#;
        let job: Job = serde_json::from_str(json).unwrap();
        assert!(!job.enabled);
        assert_eq!(job.trigger, Trigger::Interval { seconds: 3600 });
    }
}
