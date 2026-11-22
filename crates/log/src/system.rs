//! The system log: what Chummy itself did. One JSON line per entry, one file
//! per local day (`logs/system/YYYY-MM-DD.jsonl`), kept for
//! [`KEEP_DAYS`] days. Logging never fails the caller: a log that can't be
//! written is skipped.
//!
//! Never log secrets, message text or file contents here. Runs have their
//! own logs (see [`crate::runs`]).

use std::collections::{BTreeMap, VecDeque};
use std::fs::{File, OpenOptions};
use std::io::{BufRead, BufReader, Write};
use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock};

use serde::{Deserialize, Serialize};

use crate::{day_of, now_ms, valid_day};

/// Days of system log kept.
pub const KEEP_DAYS: usize = 30;
/// Entries kept in memory for a quick look.
const RECENT: usize = 500;
/// Most entries read from one day's file.
const MAX_READ: usize = 50_000;

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Level {
    Debug,
    Info,
    Warn,
    Error,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SysEntry {
    /// Unix time in milliseconds.
    pub at: u64,
    pub level: Level,
    /// What part of Chummy: "app", "settings", "models", "tools", "runs",
    /// "jobs", "questions", "engine", "security".
    pub source: String,
    pub message: String,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub fields: BTreeMap<String, String>,
}

type Listener = Box<dyn Fn(&SysEntry) + Send + Sync>;

pub struct SystemLog {
    dir: PathBuf,
    file: Mutex<Option<(String, File)>>,
    recent: Mutex<VecDeque<SysEntry>>,
    listener: Mutex<Option<Listener>>,
    /// Lowest level written; `None` writes nothing.
    min: Mutex<Option<Level>>,
    /// Also print to stderr (for development).
    echo: bool,
}

static GLOBAL: OnceLock<SystemLog> = OnceLock::new();

impl SystemLog {
    pub fn new(dir: impl Into<PathBuf>) -> Self {
        let dir = dir.into();
        let _ = std::fs::create_dir_all(&dir);
        SystemLog {
            dir,
            file: Mutex::new(None),
            recent: Mutex::new(VecDeque::new()),
            listener: Mutex::new(None),
            min: Mutex::new(Some(Level::Info)),
            echo: cfg!(debug_assertions),
        }
    }

    pub fn dir(&self) -> &Path {
        &self.dir
    }

    /// Called with every new entry (e.g. to show it live).
    pub fn set_listener(&self, f: impl Fn(&SysEntry) + Send + Sync + 'static) {
        *self.listener.lock().unwrap_or_else(|e| e.into_inner()) = Some(Box::new(f));
    }

    /// Lowest level to write (`None`: no system log at all).
    pub fn set_min_level(&self, min: Option<Level>) {
        *self.min.lock().unwrap_or_else(|e| e.into_inner()) = min;
    }

    pub fn write(&self, e: SysEntry) {
        let min = *self.min.lock().unwrap_or_else(|e| e.into_inner());
        if min.is_none_or(|m| e.level < m) {
            return;
        }
        if self.echo {
            eprintln!("[{:?}] {}: {}", e.level, e.source, e.message);
        }
        let day = day_of(e.at);
        if let Ok(line) = serde_json::to_string(&e) {
            let mut f = self.file.lock().unwrap_or_else(|e| e.into_inner());
            if f.as_ref().is_none_or(|(d, _)| *d != day) {
                *f = OpenOptions::new()
                    .create(true)
                    .append(true)
                    .open(self.dir.join(format!("{day}.jsonl")))
                    .ok()
                    .map(|file| (day.clone(), file));
                self.cleanup();
            }
            if let Some((_, file)) = f.as_mut() {
                let _ = writeln!(file, "{line}");
            }
        }
        {
            let mut r = self.recent.lock().unwrap_or_else(|e| e.into_inner());
            if r.len() == RECENT {
                r.pop_front();
            }
            r.push_back(e.clone());
        }
        if let Some(l) = self
            .listener
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .as_ref()
        {
            l(&e);
        }
    }

    /// Days with a log, newest first.
    pub fn days(&self) -> Vec<String> {
        let mut days: Vec<String> = std::fs::read_dir(&self.dir)
            .into_iter()
            .flatten()
            .flatten()
            .filter_map(|e| {
                let name = e.file_name().into_string().ok()?;
                let day = name.strip_suffix(".jsonl")?;
                valid_day(day).then(|| day.to_string())
            })
            .collect();
        days.sort_unstable_by(|a, b| b.cmp(a));
        days
    }

    /// One day's entries, oldest first.
    pub fn read_day(&self, day: &str) -> Vec<SysEntry> {
        if !valid_day(day) {
            return vec![];
        }
        let Ok(f) = File::open(self.dir.join(format!("{day}.jsonl"))) else {
            return vec![];
        };
        BufReader::new(f)
            .lines()
            .map_while(Result::ok)
            .filter_map(|l| serde_json::from_str(&l).ok())
            .take(MAX_READ)
            .collect()
    }

    /// The latest entries, oldest first.
    pub fn recent(&self) -> Vec<SysEntry> {
        self.recent
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .iter()
            .cloned()
            .collect()
    }

    /// Deletes days beyond [`KEEP_DAYS`].
    fn cleanup(&self) {
        for day in self.days().into_iter().skip(KEEP_DAYS) {
            let _ = std::fs::remove_file(self.dir.join(format!("{day}.jsonl")));
        }
    }
}

/// Sets up the app-wide system log. Later calls are ignored.
pub fn init(dir: impl Into<PathBuf>) -> &'static SystemLog {
    GLOBAL.get_or_init(|| SystemLog::new(dir))
}

/// The app-wide system log, if [`init`] was called.
pub fn global() -> Option<&'static SystemLog> {
    GLOBAL.get()
}

/// Logs to the app-wide system log (or does nothing before [`init`]).
pub fn log(level: Level, source: &str, message: impl Into<String>, fields: &[(&str, &str)]) {
    let e = SysEntry {
        at: now_ms(),
        level,
        source: source.to_string(),
        message: message.into(),
        fields: fields
            .iter()
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect(),
    };
    match GLOBAL.get() {
        Some(l) => l.write(e),
        None if cfg!(debug_assertions) => eprintln!("[{:?}] {}: {}", e.level, e.source, e.message),
        None => {}
    }
}

pub fn info(source: &str, message: impl Into<String>) {
    log(Level::Info, source, message, &[]);
}
pub fn warn(source: &str, message: impl Into<String>) {
    log(Level::Warn, source, message, &[]);
}
pub fn error(source: &str, message: impl Into<String>) {
    log(Level::Error, source, message, &[]);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn writes_reads_and_tells_listeners() {
        let tmp = tempfile::tempdir().unwrap();
        let log = SystemLog::new(tmp.path());
        log.set_min_level(Some(Level::Debug));
        let seen = std::sync::Arc::new(Mutex::new(0));
        let s2 = seen.clone();
        log.set_listener(move |_| *s2.lock().unwrap() += 1);
        let at = now_ms();
        for i in 0..3 {
            log.write(SysEntry {
                at,
                level: Level::Info,
                source: "app".into(),
                message: format!("hello {i}"),
                fields: [("k".to_string(), "v".to_string())].into(),
            });
        }
        let day = day_of(at);
        assert_eq!(log.days(), std::slice::from_ref(&day));
        let read = log.read_day(&day);
        assert_eq!(read.len(), 3);
        assert_eq!(read[2].message, "hello 2");
        assert_eq!(read[0].fields["k"], "v");
        assert_eq!(*seen.lock().unwrap(), 3);
        assert_eq!(log.recent().len(), 3);
        assert!(log.read_day("../../etc").is_empty());
    }

    #[test]
    fn levels_filter_and_off_writes_nothing() {
        let tmp = tempfile::tempdir().unwrap();
        let log = SystemLog::new(tmp.path());
        let e = |level| SysEntry {
            at: now_ms(),
            level,
            source: "app".into(),
            message: "m".into(),
            fields: Default::default(),
        };
        log.write(e(Level::Debug));
        log.write(e(Level::Info));
        log.set_min_level(Some(Level::Error));
        log.write(e(Level::Warn));
        log.write(e(Level::Error));
        log.set_min_level(None);
        log.write(e(Level::Error));
        let levels: Vec<Level> = log.recent().into_iter().map(|e| e.level).collect();
        assert_eq!(levels, [Level::Info, Level::Error]);
    }

    #[test]
    fn keeps_only_recent_days() {
        let tmp = tempfile::tempdir().unwrap();
        for d in 1..=31 {
            std::fs::write(tmp.path().join(format!("2020-01-{d:02}.jsonl")), "").unwrap();
        }
        let log = SystemLog::new(tmp.path());
        log.write(SysEntry {
            at: now_ms(),
            level: Level::Warn,
            source: "app".into(),
            message: "x".into(),
            fields: Default::default(),
        });
        let days = log.days();
        assert_eq!(days.len(), KEEP_DAYS);
        assert!(!days.contains(&"2020-01-01".to_string()));
    }
}
