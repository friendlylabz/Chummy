//! Folder triggers, by polling.
//!
//! Polling through the agent's folder jail (instead of OS file events) keeps
//! this the same on every OS, needs no extra permissions, and can't see
//! outside the agent's folder. A file starts a run once it has stopped
//! changing between two polls, so half-written files aren't picked up.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

/// How often folders are polled, in seconds.
pub const POLL_SECONDS: u64 = 15;
/// Most files looked at in one folder.
pub const MAX_FILES: usize = 2000;

/// A file's size and modification time.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct FileStamp {
    pub size: u64,
    /// Modification time in milliseconds (0 if unknown).
    pub modified_ms: i64,
}

/// The files in one folder, by name.
pub type Snapshot = BTreeMap<String, FileStamp>;

/// Whether a file name is ignored: hidden files, and common temporary and
/// partial-download names.
pub fn ignored(name: &str) -> bool {
    name.starts_with('.')
        || name.starts_with("~$")
        || name.ends_with('~')
        || [".tmp", ".part", ".crdownload", ".download", ".swp"]
            .iter()
            .any(|s| name.to_ascii_lowercase().ends_with(s))
}

/// Tracks one watched folder.
#[derive(Debug, Default)]
pub struct FolderWatch {
    known: Option<Snapshot>,
    /// Changed files waiting to be seen unchanged once more.
    settling: BTreeMap<String, FileStamp>,
}

impl FolderWatch {
    /// Compares with the last poll. The first poll only records what's
    /// there. Returns the files that are new or changed and have settled.
    pub fn poll(&mut self, now: Snapshot) -> Vec<String> {
        let Some(known) = self.known.as_mut() else {
            self.known = Some(now);
            return vec![];
        };
        let mut ready = Vec::new();
        known.retain(|name, _| now.contains_key(name));
        self.settling.retain(|name, _| now.contains_key(name));
        for (name, stamp) in &now {
            if known.get(name) == Some(stamp) {
                self.settling.remove(name);
                continue;
            }
            if self.settling.get(name) == Some(stamp) {
                self.settling.remove(name);
                known.insert(name.clone(), *stamp);
                ready.push(name.clone());
            } else {
                self.settling.insert(name.clone(), *stamp);
            }
        }
        ready
    }

    /// Accepts the folder as it is now, without starting runs: used after a
    /// run, so files the job wrote itself don't start it again.
    pub fn accept(&mut self, now: Snapshot) {
        self.settling.clear();
        self.known = Some(now);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn snap(items: &[(&str, u64, i64)]) -> Snapshot {
        items
            .iter()
            .map(|(n, s, m)| {
                (
                    n.to_string(),
                    FileStamp {
                        size: *s,
                        modified_ms: *m,
                    },
                )
            })
            .collect()
    }

    #[test]
    fn new_files_start_runs_once_they_settle() {
        let mut w = FolderWatch::default();
        assert!(
            w.poll(snap(&[("old.txt", 1, 1)])).is_empty(),
            "first poll is a baseline"
        );
        // A file appears, still being written.
        assert!(
            w.poll(snap(&[("old.txt", 1, 1), ("new.txt", 10, 5)]))
                .is_empty()
        );
        assert!(
            w.poll(snap(&[("old.txt", 1, 1), ("new.txt", 20, 6)]))
                .is_empty()
        );
        // Unchanged since the last poll: ready, once.
        assert_eq!(
            w.poll(snap(&[("old.txt", 1, 1), ("new.txt", 20, 6)])),
            ["new.txt"]
        );
        assert!(
            w.poll(snap(&[("old.txt", 1, 1), ("new.txt", 20, 6)]))
                .is_empty()
        );
        // A changed file runs again; deleted files are forgotten.
        assert!(w.poll(snap(&[("old.txt", 2, 9)])).is_empty());
        assert_eq!(w.poll(snap(&[("old.txt", 2, 9)])), ["old.txt"]);
        // Files written during a run are accepted without a run.
        w.accept(snap(&[("old.txt", 2, 9), ("report.md", 3, 10)]));
        assert!(
            w.poll(snap(&[("old.txt", 2, 9), ("report.md", 3, 10)]))
                .is_empty()
        );
    }

    #[test]
    fn temporary_files_are_ignored() {
        for n in [
            ".DS_Store",
            "~$doc.docx",
            "a.txt~",
            "big.iso.part",
            "x.CRDOWNLOAD",
            "f.tmp",
        ] {
            assert!(ignored(n), "{n}");
        }
        assert!(!ignored("notes.md"));
    }
}
