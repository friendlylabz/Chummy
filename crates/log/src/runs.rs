//! Run logs: one JSON-lines file per run of a loop, in
//! `logs/runs/<day>/<run id>.jsonl`. The first line describes the run, the
//! last says how it ended, and every line in between is something that
//! happened: a block step with its inputs and outputs, a model call, a tool
//! call or result, a notice, a question, a reply.

use std::collections::HashSet;
use std::fs::{File, OpenOptions};
use std::io::{BufRead, BufReader, BufWriter, Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};
use std::sync::Mutex;

use chummy_flow::log::{LoggedValue, ModelLog, StepLog, cut};
use chummy_flow::{Event, RunOutcome, RunStatus};
use serde::{Deserialize, Serialize};

use crate::crypt::{self, LogKey, OpenError};
use crate::{day_of, now_ms, valid_day};

/// Days of run logs kept.
pub const KEEP_DAYS: usize = 30;
/// Most bytes of run logs kept in all; the oldest days go first.
pub const KEEP_BYTES: u64 = 1024 * 1024 * 1024;
/// How much a run log records.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Detail {
    /// Which blocks ran, model and tool calls, questions, notices and
    /// replies, without the text that went in and out.
    Summary,
    /// Also the text, cut to [`STANDARD_VALUE`] characters.
    Standard,
    /// Everything, cut only past 6,000 characters a value.
    #[default]
    Verbose,
}

/// Value length at [`Detail::Standard`].
pub const STANDARD_VALUE: usize = 500;

/// How a run log is written.
#[derive(Debug, Clone, Default)]
pub struct LogOptions {
    pub detail: Detail,
    /// Seal every line with this key.
    pub key: Option<LogKey>,
}

/// After this many bytes, a run's values are cut much shorter.
const FULL_BYTES: u64 = 32 * 1024 * 1024;
/// Value length once a run's log is past [`FULL_BYTES`].
const SHORT_VALUE: usize = 300;
/// Most runs a search returns.
pub const MAX_RESULTS: usize = 500;
/// Most bytes read for a full-text search of one run.
const SEARCH_BYTES: u64 = 64 * 1024 * 1024;

/// What a run is, written first.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct RunHeader {
    pub run_id: String,
    /// What the user sees: the agent's name, a job's name or "Test: …".
    pub title: String,
    /// "chat", "job" or "test".
    pub source: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub agent_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub agent_name: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub job_id: Option<String>,
    pub loop_name: String,
    /// The model connection's name.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    /// The message the run started with (cut short).
    pub message: String,
    pub started_at: u64,
}

/// How a run ended, written last.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct RunEnd {
    pub ended_at: u64,
    /// "completed", "stopped", "failed", "limit" or "interrupted".
    pub status: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    /// The failed block's path.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub block: Option<String>,
    pub steps: u32,
    pub llm_calls: u32,
    pub tool_calls: u32,
    pub replies: u32,
    /// Estimated tokens the turn added to context (see `TurnUsage`).
    #[serde(skip_serializing_if = "is_zero")]
    pub tokens_added: u64,
    /// Estimated tokens in the latest model call's context, and that
    /// model's context length (0: no model was called).
    #[serde(skip_serializing_if = "is_zero")]
    pub context_tokens: u64,
    #[serde(skip_serializing_if = "is_zero32")]
    pub context_window: u32,
    /// The model's tokens used today when the run ended, and how many are
    /// allowed today (`None`: no daily limit).
    #[serde(skip_serializing_if = "is_zero")]
    pub daily_used: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub daily_allowed: Option<u64>,
}

fn is_zero(n: &u64) -> bool {
    *n == 0
}

fn is_zero32(n: &u32) -> bool {
    *n == 0
}

impl RunEnd {
    pub fn from_outcome(o: &RunOutcome) -> Self {
        let (status, error, block) = match &o.status {
            RunStatus::Completed => ("completed", None, None),
            RunStatus::Stopped => ("stopped", None, None),
            RunStatus::LimitReached { message } => ("limit", Some(message.clone()), None),
            RunStatus::Failed { block, error } => ("failed", Some(error.clone()), block.clone()),
            RunStatus::Paused { reason } => ("paused", reason.clone(), None),
        };
        RunEnd {
            ended_at: now_ms(),
            status: status.into(),
            error,
            block,
            steps: o.steps,
            llm_calls: o.llm_calls,
            tool_calls: o.tool_calls,
            replies: o.replies.len() as u32,
            tokens_added: o.usage.tokens_added,
            context_tokens: o.usage.context_tokens,
            context_window: o.usage.context_window,
            daily_used: 0,
            daily_allowed: None,
        }
    }

    pub fn interrupted(why: &str) -> Self {
        RunEnd {
            ended_at: now_ms(),
            status: "interrupted".into(),
            error: Some(why.into()),
            ..Default::default()
        }
    }
}

/// One line of a run log.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Entry {
    Start(RunHeader),
    Step(StepLog),
    Model(ModelLog),
    ToolCall {
        at: u64,
        block: String,
        tool: String,
        arguments: String,
    },
    ToolResult {
        at: u64,
        block: String,
        tool: String,
        text: String,
        is_error: bool,
    },
    Notice {
        at: u64,
        block: String,
        text: String,
    },
    Asked {
        at: u64,
        block: String,
        text: String,
    },
    Answered {
        at: u64,
        block: String,
    },
    Reply {
        at: u64,
        block: String,
        text: String,
    },
    End(RunEnd),
    /// Lines that couldn't be read (only made when reading).
    Unreadable {
        lines: u32,
        reason: String,
    },
}

impl Entry {
    /// The entry a run event is logged as, if it's logged at all (streamed
    /// text and block states aren't: steps and model calls cover them).
    pub fn from_event(e: &Event) -> Option<Entry> {
        let at = now_ms();
        Some(match e {
            Event::Step(s) => Entry::Step((**s).clone()),
            Event::ModelCall(m) => Entry::Model((**m).clone()),
            Event::ToolCall {
                block,
                tool,
                arguments,
            } => Entry::ToolCall {
                at,
                block: block.clone(),
                tool: tool.clone(),
                arguments: arguments.clone(),
            },
            Event::ToolResult {
                block,
                tool,
                preview,
                is_error,
            } => Entry::ToolResult {
                at,
                block: block.clone(),
                tool: tool.clone(),
                text: preview.clone(),
                is_error: *is_error,
            },
            Event::Notice { block, text } => Entry::Notice {
                at,
                block: block.clone(),
                text: text.clone(),
            },
            Event::Asked { block, text } => Entry::Asked {
                at,
                block: block.clone(),
                text: text.clone(),
            },
            Event::Answered { block } => Entry::Answered {
                at,
                block: block.clone(),
            },
            Event::Reply { block, text } => Entry::Reply {
                at,
                block: block.clone(),
                text: text.clone(),
            },
            // A Pane output's input is in its block step already.
            Event::BlockState { .. }
            | Event::BlockOutput { .. }
            | Event::LlmText { .. }
            | Event::Pane { .. } => {
                return None;
            }
        })
    }
}

/// Run ids are used as file names, so only these characters are allowed.
pub fn valid_run_id(id: &str) -> bool {
    !id.is_empty()
        && id.len() <= 80
        && id
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
}

fn shorten(v: &mut LoggedValue, max: usize) {
    if max == 0 {
        v.cut = v.chars;
        v.text.clear();
    } else if v.text.chars().count() > max {
        let (t, left) = cut(&v.text, max);
        v.text = t;
        v.cut += left;
    }
}

fn shorten_text(s: &mut String, max: usize) {
    if max == 0 {
        s.clear();
    } else {
        *s = cut(s, max).0;
    }
}

/// Cuts the values in `e` to `max` characters (0: none at all).
fn trim(e: &mut Entry, max: usize) {
    match e {
        Entry::Step(s) => {
            for p in s.inputs.iter_mut().chain(s.outputs.iter_mut()) {
                shorten(&mut p.value, max);
            }
        }
        Entry::Model(m) => {
            shorten(&mut m.sent, max);
            shorten(&mut m.answer, max);
            shorten_text(&mut m.reasoning, max);
            for c in &mut m.calls {
                // Keep which tool, drop the arguments.
                if max == 0 {
                    *c = c.split('(').next().unwrap_or("").to_string();
                } else {
                    shorten_text(c, max);
                }
            }
        }
        Entry::ToolCall { arguments, .. } => shorten_text(arguments, max),
        Entry::ToolResult { text, .. } => shorten_text(text, max),
        _ => {}
    }
}

/// Writes one run's log as it goes.
pub struct RunLog {
    path: PathBuf,
    out: Mutex<Option<BufWriter<File>>>,
    bytes: Mutex<u64>,
    opts: LogOptions,
}

impl RunLog {
    /// Starts a run's log under `root` (`logs/runs`). If it can't be created,
    /// the run goes ahead without one.
    pub fn create(root: &Path, header: RunHeader, opts: LogOptions) -> Option<RunLog> {
        if !valid_run_id(&header.run_id) {
            return None;
        }
        let dir = root.join(day_of(header.started_at));
        std::fs::create_dir_all(&dir).ok()?;
        let path = dir.join(format!("{}.jsonl", header.run_id));
        let file = OpenOptions::new()
            .create_new(true)
            .write(true)
            .open(&path)
            .ok()?;
        let log = RunLog {
            path,
            out: Mutex::new(Some(BufWriter::new(file))),
            bytes: Mutex::new(0),
            opts,
        };
        let mut header = header;
        header.message = cut(&header.message, 4000).0;
        log.write(&Entry::Start(header));
        Some(log)
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    pub fn write(&self, e: &Entry) {
        let mut bytes = self.bytes.lock().unwrap_or_else(|e| e.into_inner());
        let mut e = e.clone();
        let max = match self.opts.detail {
            Detail::Summary => Some(0),
            Detail::Standard => Some(STANDARD_VALUE),
            Detail::Verbose => None,
        };
        let max = if *bytes > FULL_BYTES {
            Some(max.unwrap_or(SHORT_VALUE).min(SHORT_VALUE))
        } else {
            max
        };
        if let Some(max) = max {
            trim(&mut e, max);
        }
        let Ok(line) = serde_json::to_string(&e) else {
            return;
        };
        let line = match &self.opts.key {
            Some(k) => match k.seal(&line) {
                Ok(l) => l,
                // Never fall back to plain text.
                Err(_) => return,
            },
            None => line,
        };
        let mut out = self.out.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(w) = out.as_mut() {
            if writeln!(w, "{line}").is_ok() {
                *bytes += line.len() as u64 + 1;
            }
            // Steps are flushed so a crash keeps what happened before it.
            if matches!(e, Entry::Start(_) | Entry::End(_) | Entry::Step(_)) {
                let _ = w.flush();
            }
        }
    }

    pub fn event(&self, e: &Event) {
        if let Some(entry) = Entry::from_event(e) {
            self.write(&entry);
        }
    }

    /// Writes how the run ended and closes the log. Later writes are ignored.
    pub fn end(&self, end: RunEnd) {
        self.write(&Entry::End(end));
        if let Some(mut w) = self.out.lock().unwrap_or_else(|e| e.into_inner()).take() {
            let _ = w.flush();
        }
    }

    pub fn ended(&self) -> bool {
        self.out.lock().unwrap_or_else(|e| e.into_inner()).is_none()
    }
}

impl Drop for RunLog {
    fn drop(&mut self) {
        if !self.ended() {
            self.end(RunEnd::interrupted("The run ended without a result."));
        }
    }
}

/// A run as listed: its header and, if it has ended, how.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct RunSummary {
    pub header: RunHeader,
    pub end: Option<RunEnd>,
    pub bytes: u64,
    /// Written encrypted.
    pub encrypted: bool,
    /// Why it can't be read here, if it can't.
    pub locked: Option<String>,
    /// The day folder it's in.
    pub day: String,
}

/// What to look for when listing runs. Empty fields match everything.
#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize)]
#[serde(default)]
pub struct RunQuery {
    /// Words that must all appear in the title, agent, loop, model or
    /// message (or anywhere in the log, with `full_text`).
    pub text: String,
    pub full_text: bool,
    /// "chat", "job" or "test".
    pub source: String,
    /// "completed", "stopped", "failed", "limit", "interrupted" or
    /// "running" (no end yet).
    pub status: String,
    pub agent_id: String,
    /// Inclusive, `YYYY-MM-DD`.
    pub from_day: String,
    pub to_day: String,
    /// 0 means [`MAX_RESULTS`].
    pub limit: usize,
}

/// Days with run logs, newest first.
pub fn days(root: &Path) -> Vec<String> {
    let mut days: Vec<String> = std::fs::read_dir(root)
        .into_iter()
        .flatten()
        .flatten()
        .filter_map(|e| {
            let name = e.file_name().into_string().ok()?;
            valid_day(&name).then_some(name)
        })
        .collect();
    days.sort_unstable_by(|a, b| b.cmp(a));
    days
}

fn decode(line: &str, key: Option<&LogKey>) -> Result<Entry, OpenError> {
    let text = crypt::open(line, key)?;
    serde_json::from_str(&text).map_err(|_| OpenError::Damaged)
}

fn first_line(path: &Path) -> Option<String> {
    let f = File::open(path).ok()?;
    let mut line = String::new();
    BufReader::new(f).read_line(&mut line).ok()?;
    Some(line)
}

fn last_line(path: &Path) -> Option<String> {
    let mut f = File::open(path).ok()?;
    let len = f.metadata().ok()?.len();
    let start = len.saturating_sub(256 * 1024);
    f.seek(SeekFrom::Start(start)).ok()?;
    let mut buf = String::new();
    f.read_to_string(&mut buf).ok()?;
    buf.lines()
        .rev()
        .find(|l| !l.trim().is_empty())
        .map(String::from)
}

fn summary(path: &Path, day: &str, key: Option<&LogKey>) -> Option<RunSummary> {
    let first = first_line(path)?;
    let (header, locked) = match decode(first.trim_end(), key) {
        Ok(Entry::Start(h)) => (h, None),
        Ok(_) => return None,
        Err(e) => {
            let run_id = path.file_stem()?.to_str()?.to_string();
            let started_at = std::fs::metadata(path)
                .and_then(|m| m.modified())
                .ok()
                .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
                .map(|d| d.as_millis() as u64)
                .unwrap_or(0);
            (
                RunHeader {
                    run_id,
                    title: "Encrypted run".into(),
                    started_at,
                    ..Default::default()
                },
                Some(reason(&e)),
            )
        }
    };
    let end = last_line(path).and_then(|l| match decode(&l, key) {
        Ok(Entry::End(e)) => Some(e),
        _ => None,
    });
    Some(RunSummary {
        header,
        end,
        bytes: std::fs::metadata(path).map(|m| m.len()).unwrap_or(0),
        day: day.to_string(),
        encrypted: first.starts_with(crypt::PREFIX),
        locked,
    })
}

/// Why lines can't be read, for people.
pub fn reason(e: &OpenError) -> String {
    match e {
        OpenError::NoKey => "This log is encrypted, and there's no log key on this computer. Import the key it was written with (Settings → Logs).".into(),
        OpenError::OtherKey(id) => format!(
            "This log was encrypted with another log key ({}). Import that key to read it.",
            id.as_bytes()
                .chunks(4)
                .map(|c| std::str::from_utf8(c).unwrap_or(""))
                .collect::<Vec<_>>()
                .join("-")
        ),
        OpenError::Damaged => "Some lines are damaged or were changed, so they can't be read.".into(),
    }
}

fn contains_all(hay: &str, words: &[String]) -> bool {
    let hay = hay.to_lowercase();
    words.iter().all(|w| hay.contains(w.as_str()))
}

fn file_contains_all(path: &Path, words: &[String], key: Option<&LogKey>) -> bool {
    let Ok(f) = File::open(path) else {
        return false;
    };
    let mut buf = String::new();
    if f.take(SEARCH_BYTES).read_to_string(&mut buf).is_err() {
        return false;
    }
    if !buf.starts_with(crypt::PREFIX) {
        return contains_all(&buf, words);
    }
    let text: String = buf
        .lines()
        .filter_map(|l| crypt::open(l, key).ok())
        .collect::<Vec<_>>()
        .join("\n");
    contains_all(&text, words)
}

/// Runs matching `q`, newest first. Runs in `running` with no end yet are
/// "running"; others with no end were interrupted.
pub fn search(
    root: &Path,
    q: &RunQuery,
    running: &HashSet<String>,
    key: Option<&LogKey>,
) -> Vec<RunSummary> {
    let limit = if q.limit == 0 {
        MAX_RESULTS
    } else {
        q.limit.min(MAX_RESULTS)
    };
    let words: Vec<String> = q
        .text
        .split_whitespace()
        .map(|w| w.to_lowercase())
        .collect();
    let mut out = Vec::new();
    for day in days(root) {
        if !q.to_day.is_empty() && day > q.to_day {
            continue;
        }
        if !q.from_day.is_empty() && day < q.from_day {
            break;
        }
        let dir = root.join(&day);
        let mut found: Vec<RunSummary> = std::fs::read_dir(&dir)
            .into_iter()
            .flatten()
            .flatten()
            .filter_map(|e| {
                let p = e.path();
                (p.extension().is_some_and(|x| x == "jsonl")).then_some(p)
            })
            .filter_map(|p| {
                let s = summary(&p, &day, key)?;
                let h = &s.header;
                if !q.source.is_empty() && h.source != q.source {
                    return None;
                }
                if !q.agent_id.is_empty() && h.agent_id.as_deref() != Some(q.agent_id.as_str()) {
                    return None;
                }
                let status = status_of(&s, running);
                if !q.status.is_empty() && status != q.status {
                    return None;
                }
                if !words.is_empty() {
                    let meta = format!(
                        "{} {} {} {} {}",
                        h.title,
                        h.agent_name.as_deref().unwrap_or(""),
                        h.loop_name,
                        h.model.as_deref().unwrap_or(""),
                        h.message
                    );
                    if !contains_all(&meta, &words)
                        && !(q.full_text && file_contains_all(&p, &words, key))
                    {
                        return None;
                    }
                }
                Some(s)
            })
            .collect();
        found.sort_unstable_by_key(|s| std::cmp::Reverse(s.header.started_at));
        out.extend(found);
        if out.len() >= limit {
            out.truncate(limit);
            break;
        }
    }
    out
}

/// A run's status as listed.
pub fn status_of(s: &RunSummary, running: &HashSet<String>) -> String {
    match &s.end {
        Some(e) => e.status.clone(),
        None if running.contains(&s.header.run_id) => "running".into(),
        None => "interrupted".into(),
    }
}

/// Finds a run's log file.
pub fn find(root: &Path, run_id: &str) -> Option<PathBuf> {
    if !valid_run_id(run_id) {
        return None;
    }
    days(root)
        .into_iter()
        .map(|d| root.join(d).join(format!("{run_id}.jsonl")))
        .find(|p| p.is_file())
}

/// Every entry of a run's log, in the order written. Lines that can't be
/// read are counted in one [`Entry::Unreadable`] after the first entry.
pub fn read(root: &Path, run_id: &str, key: Option<&LogKey>) -> Option<Vec<Entry>> {
    let f = File::open(find(root, run_id)?).ok()?;
    let mut out = Vec::new();
    let mut bad = 0u32;
    let mut why = None;
    for line in BufReader::new(f).lines().map_while(Result::ok) {
        if line.trim().is_empty() {
            continue;
        }
        match decode(&line, key) {
            Ok(e) => out.push(e),
            Err(e) => {
                bad += 1;
                why.get_or_insert(e);
            }
        }
    }
    if let Some(e) = why {
        out.insert(
            out.len().min(1),
            Entry::Unreadable {
                lines: bad,
                reason: reason(&e),
            },
        );
    }
    Some(out)
}

/// Deletes one run's log.
pub fn delete(root: &Path, run_id: &str) -> bool {
    find(root, run_id).is_some_and(|p| std::fs::remove_file(p).is_ok())
}

/// Removes logs older than [`KEEP_DAYS`] days, then the oldest days until
/// the rest fit in [`KEEP_BYTES`].
pub fn cleanup(root: &Path) {
    let days = days(root);
    for d in days.iter().skip(KEEP_DAYS) {
        let _ = std::fs::remove_dir_all(root.join(d));
    }
    let mut kept: Vec<(String, u64)> = days
        .into_iter()
        .take(KEEP_DAYS)
        .map(|d| {
            let size = std::fs::read_dir(root.join(&d))
                .into_iter()
                .flatten()
                .flatten()
                .filter_map(|e| e.metadata().ok())
                .map(|m| m.len())
                .sum();
            (d, size)
        })
        .collect();
    let mut total: u64 = kept.iter().map(|(_, s)| s).sum();
    // Never remove today's.
    while total > KEEP_BYTES && kept.len() > 1 {
        let (d, s) = kept.pop().unwrap();
        let _ = std::fs::remove_dir_all(root.join(d));
        total -= s;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chummy_flow::Value;
    use chummy_flow::log::{PortLog, StepStatus, logged};

    fn header(id: &str, title: &str, at: u64) -> RunHeader {
        RunHeader {
            run_id: id.into(),
            title: title.into(),
            source: "chat".into(),
            agent_id: Some("a1".into()),
            loop_name: "Simple chat".into(),
            message: "What's the weather?".into(),
            started_at: at,
            ..Default::default()
        }
    }

    fn step(n: u32, text: &str) -> Event {
        Event::Step(Box::new(StepLog {
            n,
            block: "m".into(),
            label: "Answer".into(),
            kind: "llm".into(),
            function: None,
            inputs: vec![PortLog {
                port: "in".into(),
                value: logged(&Value::Text(text.into()), 6000),
            }],
            outputs: vec![],
            status: StepStatus::Done,
            error: None,
            started_at: now_ms(),
            ms: 3,
        }))
    }

    #[test]
    fn writes_and_reads_a_run() {
        let tmp = tempfile::tempdir().unwrap();
        let log = RunLog::create(
            tmp.path(),
            header("run-1", "Helper", now_ms()),
            LogOptions::default(),
        )
        .unwrap();
        log.event(&step(1, "Sunny"));
        log.event(&Event::LlmText {
            block: "m".into(),
            text: "x".into(),
        });
        log.event(&Event::Reply {
            block: "o".into(),
            text: "It's sunny.".into(),
        });
        log.end(RunEnd {
            status: "completed".into(),
            steps: 2,
            ..Default::default()
        });
        log.event(&step(2, "ignored after the end"));
        drop(log);
        let entries = read(tmp.path(), "run-1", None).unwrap();
        assert_eq!(entries.len(), 4, "{entries:?}");
        assert!(matches!(&entries[0], Entry::Start(h) if h.title == "Helper"));
        assert!(matches!(&entries[1], Entry::Step(s) if s.inputs[0].value.text == "Sunny"));
        assert!(matches!(&entries[3], Entry::End(e) if e.status == "completed"));
        assert!(read(tmp.path(), "../x", None).is_none());

        // (was a_dropped_log_is_marked_interrupted)
        {
            let tmp = tempfile::tempdir().unwrap();
            drop(
                RunLog::create(
                    tmp.path(),
                    header("run-2", "Helper", now_ms()),
                    LogOptions::default(),
                )
                .unwrap(),
            );
            let s = search(tmp.path(), &RunQuery::default(), &HashSet::new(), None);
            assert_eq!(s[0].end.as_ref().unwrap().status, "interrupted");
        }
    }

    #[test]
    fn searches_by_fields_and_full_text() {
        let tmp = tempfile::tempdir().unwrap();
        let now = now_ms();
        let a = RunLog::create(
            tmp.path(),
            header("run-a", "Researcher", now - 1000),
            LogOptions::default(),
        )
        .unwrap();
        a.event(&step(1, "the secret word is pineapple"));
        a.end(RunEnd {
            status: "completed".into(),
            ..Default::default()
        });
        let mut h = header("run-b", "Writer", now);
        h.source = "job".into();
        let b = RunLog::create(tmp.path(), h, LogOptions::default()).unwrap();
        b.event(&step(1, "plain"));
        let running: HashSet<String> = ["run-b".to_string()].into();

        let all = search(tmp.path(), &RunQuery::default(), &running, None);
        assert_eq!(
            all.iter()
                .map(|s| s.header.run_id.as_str())
                .collect::<Vec<_>>(),
            ["run-b", "run-a"]
        );
        let q = |f: fn(&mut RunQuery)| {
            let mut q = RunQuery::default();
            f(&mut q);
            search(tmp.path(), &q, &running, None)
                .into_iter()
                .map(|s| s.header.run_id)
                .collect::<Vec<_>>()
        };
        assert_eq!(q(|q| q.source = "job".into()), ["run-b"]);
        assert_eq!(q(|q| q.status = "running".into()), ["run-b"]);
        assert_eq!(q(|q| q.status = "completed".into()), ["run-a"]);
        assert_eq!(q(|q| q.text = "research".into()), ["run-a"]);
        assert!(q(|q| q.text = "pineapple".into()).is_empty());
        assert_eq!(
            q(|q| {
                q.text = "PINEAPPLE".into();
                q.full_text = true;
            }),
            ["run-a"]
        );
        assert!(q(|q| q.to_day = "2000-01-01".into()).is_empty());
        drop(b);
    }

    #[test]
    fn encrypted_logs_read_only_with_their_key() {
        let tmp = tempfile::tempdir().unwrap();
        let key = LogKey::generate().unwrap();
        let opts = LogOptions {
            detail: Detail::Verbose,
            key: Some(key.clone()),
        };
        let log = RunLog::create(tmp.path(), header("run-e", "Helper", now_ms()), opts).unwrap();
        log.event(&step(1, "my api key is sk-secret"));
        log.end(RunEnd {
            status: "completed".into(),
            ..Default::default()
        });
        drop(log);
        let raw = std::fs::read_to_string(find(tmp.path(), "run-e").unwrap()).unwrap();
        assert!(!raw.contains("sk-secret") && !raw.contains("Helper"));
        assert!(raw.lines().all(|l| l.starts_with(crypt::PREFIX)));

        let entries = read(tmp.path(), "run-e", Some(&key)).unwrap();
        assert!(
            matches!(&entries[1], Entry::Step(s) if s.inputs[0].value.text.contains("sk-secret"))
        );
        let found = |k: Option<&LogKey>| {
            let q = RunQuery {
                text: "sk-secret".into(),
                full_text: true,
                ..Default::default()
            };
            search(tmp.path(), &q, &HashSet::new(), k).len()
        };
        assert_eq!(found(Some(&key)), 1);
        assert_eq!(found(None), 0);

        // Without the key: listed as locked, and reading says why.
        let s = search(tmp.path(), &RunQuery::default(), &HashSet::new(), None);
        assert!(s[0].encrypted && s[0].locked.is_some());
        let entries = read(tmp.path(), "run-e", None).unwrap();
        assert!(matches!(&entries[0], Entry::Unreadable { lines: 3, .. }));
        let other = LogKey::generate().unwrap();
        let entries = read(tmp.path(), "run-e", Some(&other)).unwrap();
        assert!(
            matches!(&entries[0], Entry::Unreadable { reason, .. } if reason.contains("another log key"))
        );
    }

    #[test]
    fn detail_levels_cut_values() {
        let tmp = tempfile::tempdir().unwrap();
        for (detail, id) in [(Detail::Summary, "run-s"), (Detail::Standard, "run-t")] {
            let opts = LogOptions { detail, key: None };
            let log = RunLog::create(tmp.path(), header(id, "Helper", now_ms()), opts).unwrap();
            log.event(&step(1, &"x".repeat(2000)));
            log.end(RunEnd::default());
        }
        let text = |id: &str| match &read(tmp.path(), id, None).unwrap()[1] {
            Entry::Step(s) => s.inputs[0].value.clone(),
            e => panic!("{e:?}"),
        };
        let s = text("run-s");
        assert!(s.text.is_empty() && s.cut == 2000 && s.chars == 2000);
        let t = text("run-t");
        assert!(t.text.chars().count() < 600 && t.cut > 1400);
    }

    #[test]
    fn cleanup_drops_old_days() {
        let tmp = tempfile::tempdir().unwrap();
        for d in 1..=31 {
            std::fs::create_dir_all(tmp.path().join(format!("2020-01-{d:02}"))).unwrap();
        }
        cleanup(tmp.path());
        let d = days(tmp.path());
        assert_eq!(d.len(), KEEP_DAYS);
        assert_eq!(d.last().unwrap(), "2020-01-02");
    }
}
