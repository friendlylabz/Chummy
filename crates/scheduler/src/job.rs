//! The job file format, `chummy.job` version 1.
//!
//! A job runs one agent's loop with a message, on one or more triggers. The
//! same file works in the Chummy app and in chummyd: it names the agent by
//! id, and the agent (its folder, model and permissions) comes from whoever
//! runs the job. A job can never give itself more access than its agent has.
//! Spec: `docs/dev/file-formats.md` (Scheduled jobs).

use jiff::Zoned;
use serde::{Deserialize, Serialize};

use crate::trigger::Trigger;

pub const FORMAT: &str = "chummy.job";
pub const VERSION: u32 = 1;

/// Largest job file accepted.
pub const MAX_FILE_BYTES: usize = 256 * 1024;
/// Largest message, in characters.
pub const MAX_MESSAGE_CHARS: usize = 50_000;
/// Most triggers per job.
pub const MAX_TRIGGERS: usize = 10;
/// Longest time limit: a week.
pub const MAX_TIME_LIMIT_MINUTES: u32 = 10_080;

/// What happens when a scheduled time passed while Chummy wasn't running
/// (or the computer was asleep).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Missed {
    /// Skip it and wait for the next time.
    #[default]
    Skip,
    /// Run once as soon as possible (several missed times still run once).
    RunOnce,
}

/// When to show a notification for a finished run.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Notify {
    Always,
    /// Only when a run fails, stops or hits a limit.
    #[default]
    Problems,
    Never,
}

/// A job definition, as stored in `jobs/<id>.json`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Job {
    pub format: String,
    pub version: u32,
    pub id: String,
    pub name: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub description: String,
    /// Off: the job only runs when started by hand.
    #[serde(default)]
    pub enabled: bool,
    /// The agent that runs it.
    pub agent: String,
    /// A loop to use instead of the agent's own.
    #[serde(rename = "loop", default, skip_serializing_if = "Option::is_none")]
    pub loop_id: Option<String>,
    /// The message the loop starts with. Placeholders: `_NOW_`, `_DATE_`,
    /// `_TIME_`, `_FILE_` and `_FILE_TEXT_` (see [`Job::message_for`]).
    #[serde(default)]
    pub message: String,
    /// A file in the agent's folder to include, for runs not started by a
    /// folder trigger.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub file: Option<String>,
    #[serde(default)]
    pub triggers: Vec<Trigger>,
    #[serde(default)]
    pub missed: Missed,
    /// Stop a run after this many minutes; 0 means no limit.
    #[serde(default)]
    pub time_limit_minutes: u32,
    /// Also save the reply to this file in the agent's folder. `_DATE_` and
    /// `_TIME_` may be used in the name.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub save_reply_to: Option<String>,
    #[serde(default)]
    pub notify: Notify,
}

#[derive(Debug, thiserror::Error, PartialEq)]
pub enum JobError {
    #[error("this isn't a valid job file: {0}")]
    Parse(String),
    #[error("this isn't a Chummy job file (format is “{0}”)")]
    WrongFormat(String),
    #[error("this job was made with a newer version of Chummy (format version {0})")]
    UnsupportedVersion(u64),
    #[error("the job file is too large")]
    TooLarge,
    #[error("{0}")]
    Invalid(String),
}

/// Ids become file names, so they use a small, safe alphabet.
pub fn valid_id(id: &str) -> bool {
    // "state" and "history" are taken by the job folder's own files.
    !id.is_empty()
        && id != "state"
        && id != "history"
        && id.len() <= 64
        && !id.starts_with(['.', '-'])
        && id
            .chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-' || c == '_')
}

/// Why `path` can't name something inside an agent's folder, if it can't.
/// The folder jail checks again when the path is used; this is for clear
/// messages when a job is saved.
pub fn path_problem(path: &str) -> Option<&'static str> {
    let p = path.trim();
    if p.is_empty() {
        return Some("is empty");
    }
    if p.contains('\0') {
        return Some("has an invalid character");
    }
    let b = p.as_bytes();
    if p.starts_with(['/', '\\']) || (b.len() > 1 && b[1] == b':') || p.starts_with('~') {
        return Some("must be inside the agent's folder (a relative path like notes/today.md)");
    }
    if p.split(['/', '\\']).any(|c| c == "..") {
        return Some("can't use .. to leave the agent's folder");
    }
    None
}

impl Job {
    /// A new, disabled job with nothing scheduled.
    pub fn new(id: &str, name: &str, agent: &str) -> Self {
        Self {
            format: FORMAT.into(),
            version: VERSION,
            id: id.into(),
            name: name.into(),
            description: String::new(),
            enabled: false,
            agent: agent.into(),
            loop_id: None,
            message: String::new(),
            file: None,
            triggers: vec![],
            missed: Missed::Skip,
            time_limit_minutes: 0,
            save_reply_to: None,
            notify: Notify::Problems,
        }
    }

    /// Reads a job file, checking its format and version.
    pub fn from_json(text: &str) -> Result<Self, JobError> {
        if text.len() > MAX_FILE_BYTES {
            return Err(JobError::TooLarge);
        }
        let raw: serde_json::Value =
            serde_json::from_str(text).map_err(|e| JobError::Parse(e.to_string()))?;
        match raw.get("format").and_then(|f| f.as_str()) {
            Some(FORMAT) => {}
            Some(other) => return Err(JobError::WrongFormat(other.into())),
            None => return Err(JobError::Parse("it has no “format” field".into())),
        }
        let version = raw
            .get("version")
            .and_then(|v| v.as_u64())
            .ok_or_else(|| JobError::Parse("it has no “version” number".into()))?;
        if version > u64::from(VERSION) {
            return Err(JobError::UnsupportedVersion(version));
        }
        if version == 0 {
            return Err(JobError::Parse("version 0 isn't a real version".into()));
        }
        serde_json::from_value(raw).map_err(|e| JobError::Parse(e.to_string()))
    }

    pub fn to_json(&self) -> String {
        serde_json::to_string_pretty(self).unwrap_or_default()
    }

    /// Everything wrong with the job, in words for the user. Empty: it can
    /// be saved.
    pub fn problems(&self) -> Vec<String> {
        let mut v = Vec::new();
        if !valid_id(&self.id) {
            v.push("The job's id isn't valid.".into());
        }
        if self.name.trim().is_empty() {
            v.push("Give the job a name.".into());
        } else if self.name.chars().count() > 100 {
            v.push("The name is too long (100 characters at most).".into());
        }
        if self.description.chars().count() > 2000 {
            v.push("The description is too long.".into());
        }
        if self.agent.trim().is_empty() {
            v.push("Choose the agent that runs the job.".into());
        }
        if self.loop_id.as_deref().is_some_and(str::is_empty) {
            v.push("Choose a loop, or use the agent's own.".into());
        }
        if self.message.trim().is_empty() && self.file.is_none() {
            v.push("Write the message the job starts with, or include a file.".into());
        }
        if self.message.chars().count() > MAX_MESSAGE_CHARS {
            v.push(format!(
                "The message is too long ({MAX_MESSAGE_CHARS} characters at most)."
            ));
        }
        if let Some(why) = self.file.as_deref().and_then(path_problem) {
            v.push(format!("The included file's path {why}."));
        }
        if let Some(why) = self.save_reply_to.as_deref().and_then(path_problem) {
            v.push(format!("The path to save replies to {why}."));
        }
        if self.triggers.len() > MAX_TRIGGERS {
            v.push(format!("A job can have at most {MAX_TRIGGERS} schedules."));
        }
        for t in &self.triggers {
            if let Some(why) = t.problem() {
                v.push(why);
            }
        }
        if self.time_limit_minutes > MAX_TIME_LIMIT_MINUTES {
            v.push("The time limit can be a week at most.".into());
        }
        v
    }

    /// Checks the job can be saved.
    pub fn check(&self) -> Result<(), JobError> {
        match self.problems().first() {
            Some(p) => Err(JobError::Invalid(p.clone())),
            None => Ok(()),
        }
    }

    /// Folders in the agent's folder that trigger this job when files change.
    pub fn watched_folders(&self) -> Vec<&str> {
        self.triggers
            .iter()
            .filter_map(|t| match t {
                Trigger::Folder { path } => Some(path.as_str()),
                _ => None,
            })
            .collect()
    }

    /// The message for one run. `file` is the run's file (the one that
    /// triggered it, or the job's included file) and its text, if read.
    ///
    /// - `_NOW_`: date and time, e.g. `Sunday 2026-11-22 13:28`
    /// - `_DATE_`: `2026-11-22`; `_TIME_`: `13:28`
    /// - `_FILE_`: the file's path; `_FILE_TEXT_`: its text
    ///
    /// When there's file text and the message uses neither file placeholder,
    /// the text is added after the message.
    pub fn message_for(&self, now: &Zoned, file: Option<(&str, Option<&str>)>) -> String {
        let (path, text) = match file {
            Some((p, t)) => (p, t.unwrap_or("")),
            None => ("", ""),
        };
        let uses_file = self.message.contains("_FILE_");
        let mut out = fill(&self.message, now, path, text);
        if !uses_file && !text.is_empty() {
            if !out.trim().is_empty() {
                out.push_str("\n\n");
            }
            out.push_str(text);
        }
        out
    }

    /// The file a reply is saved to, for a run at `now`.
    pub fn reply_path(&self, now: &Zoned) -> Option<String> {
        self.save_reply_to.as_ref().map(|t| {
            t.replace("_DATE_", &now.strftime("%Y-%m-%d").to_string())
                .replace("_TIME_", &now.strftime("%H%M").to_string())
        })
    }
}

/// Fills the placeholders in one pass, so text that came from a file is
/// never itself searched for placeholders.
fn fill(template: &str, now: &Zoned, path: &str, text: &str) -> String {
    const KEYS: [&str; 5] = ["_FILE_TEXT_", "_FILE_", "_NOW_", "_DATE_", "_TIME_"];
    let value = |k: &str| match k {
        "_FILE_TEXT_" => text.to_string(),
        "_FILE_" => path.to_string(),
        "_NOW_" => now.strftime("%A %Y-%m-%d %H:%M").to_string(),
        "_DATE_" => now.strftime("%Y-%m-%d").to_string(),
        _ => now.strftime("%H:%M").to_string(),
    };
    let mut out = String::with_capacity(template.len());
    let mut rest = template;
    while let Some(i) = rest.find('_') {
        out.push_str(&rest[..i]);
        let tail = &rest[i..];
        match KEYS.iter().find(|k| tail.starts_with(**k)) {
            Some(k) => {
                out.push_str(&value(k));
                rest = &tail[k.len()..];
            }
            None => {
                out.push('_');
                rest = &tail[1..];
            }
        }
    }
    out.push_str(rest);
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::trigger::Day;

    fn now() -> Zoned {
        "2026-11-22T13:28:00-05:00[America/New_York]"
            .parse()
            .unwrap()
    }

    #[test]
    fn round_trips_and_is_strict() {
        let mut j = Job::new("digest", "Daily digest", "a1");
        j.message = "Summarize _DATE_".into();
        j.triggers = vec![Trigger::Daily {
            time: "08:30".into(),
            days: vec![Day::Mon, Day::Fri],
        }];
        let text = j.to_json();
        assert!(text.contains("\"format\": \"chummy.job\""));
        assert_eq!(Job::from_json(&text).unwrap(), j);

        let typo = text.replace("\"message\"", "\"mesage\"");
        assert!(matches!(Job::from_json(&typo), Err(JobError::Parse(_))));
        let newer = text.replace("\"version\": 1", "\"version\": 2");
        assert_eq!(Job::from_json(&newer), Err(JobError::UnsupportedVersion(2)));
        let huge = text.replace("\"version\": 1", "\"version\": 18446744073709551615");
        assert!(matches!(
            Job::from_json(&huge),
            Err(JobError::UnsupportedVersion(_))
        ));
        let other = text.replace("chummy.job", "chummy.loop");
        assert!(matches!(
            Job::from_json(&other),
            Err(JobError::WrongFormat(_))
        ));
        assert_eq!(
            Job::from_json(&" ".repeat(MAX_FILE_BYTES + 1)),
            Err(JobError::TooLarge)
        );

        // (was messages_fill_placeholders_once)
        {
            let mut j = Job::new("j", "J", "a");
            j.message = "Now _NOW_; file _FILE_:\n_FILE_TEXT_".into();
            let m = j.message_for(&now(), Some(("in/a.txt", Some("says _DATE_"))));
            assert_eq!(
                m,
                "Now Sunday 2026-11-22 13:28; file in/a.txt:\nsays _DATE_"
            );
            // File text is added when the message doesn't place it.
            j.message = "Summarize this:".into();
            assert_eq!(
                j.message_for(&now(), Some(("a", Some("text")))),
                "Summarize this:\n\ntext"
            );
            assert_eq!(j.message_for(&now(), None), "Summarize this:");
            j.save_reply_to = Some("reports/_DATE_-_TIME_.md".into());
            assert_eq!(j.reply_path(&now()).unwrap(), "reports/2026-11-22-1328.md");
        }
    }

    #[test]
    fn problems_are_explained() {
        let mut j = Job::new("Bad Id", "", "");
        j.file = Some("../secrets.txt".into());
        j.save_reply_to = Some("/etc/x".into());
        j.triggers = vec![Trigger::Every { minutes: 1 }];
        let p = j.problems().join(" | ");
        for needle in [
            "id isn't valid",
            "name",
            "agent",
            "..",
            "inside the agent's folder",
            "5 minutes",
        ] {
            assert!(p.contains(needle), "{needle} missing from {p}");
        }
        let mut ok = Job::new("ok", "Fine", "a1");
        ok.message = "hi".into();
        assert!(ok.problems().is_empty());
        for bad in ["C:\\x", "~/x", "a/../../b", "\\\\server\\share"] {
            assert!(path_problem(bad).is_some(), "{bad}");
        }
        assert!(path_problem("notes/today.md").is_none() && path_problem(".").is_none());
    }
}
