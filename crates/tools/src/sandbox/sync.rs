//! Copying a workspace into a sandbox that can't mount folders (OpenShell),
//! and bringing the tool's changes back.
//!
//! Both directions go through the agent's [`Jail`], so nothing outside the
//! workspace is ever read or written, and links are never followed:
//!
//! - [`stage`] copies the workspace's folders and regular files into a
//!   staging folder (links, devices and pipes are left out) and remembers a
//!   hash of every file.
//! - [`apply`] compares what came back with that snapshot. New and changed
//!   files are written, and files the tool deleted are deleted, but only
//!   where the workspace still matches the snapshot: if you (or another
//!   chat) changed a file while the tool ran, your version is kept and the
//!   conflict is reported. Links and special files that came back are
//!   ignored. If what came back is over the limits, nothing is applied.

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

use chummy_sandbox::Jail;
use sha2::{Digest, Sha256};

/// How much a workspace may hold to be copied for each call.
#[derive(Debug, Clone, Copy)]
pub struct Limits {
    pub max_files: usize,
    pub max_bytes: u64,
}

pub const LIMITS: Limits = Limits {
    max_files: 20_000,
    max_bytes: 512 * 1024 * 1024,
};

/// The workspace as it was copied in.
#[derive(Debug, Default, Clone)]
pub struct Snapshot {
    files: BTreeMap<String, [u8; 32]>,
    dirs: BTreeSet<String>,
    /// Links and special files that weren't copied.
    pub skipped: Vec<String>,
    pub bytes: u64,
}

impl Snapshot {
    pub fn file_count(&self) -> usize {
        self.files.len()
    }
}

/// What [`apply`] did.
#[derive(Debug, Default, Clone, PartialEq)]
pub struct Report {
    pub written: Vec<String>,
    pub deleted: Vec<String>,
    /// Changed in the workspace while the tool ran: kept as they were.
    pub conflicts: Vec<String>,
    /// Came back as links or special files: ignored.
    pub ignored: Vec<String>,
}

impl Report {
    /// A short note for the tool's log, or `None` when there's nothing
    /// worth saying.
    pub fn note(&self) -> Option<String> {
        let mut parts = Vec::new();
        if !self.conflicts.is_empty() {
            parts.push(format!(
                "kept your version of {} (changed while the tool ran): {}",
                plural(self.conflicts.len(), "file"),
                list(&self.conflicts)
            ));
        }
        if !self.ignored.is_empty() {
            parts.push(format!(
                "ignored {} that came back as links or special files: {}",
                plural(self.ignored.len(), "item"),
                list(&self.ignored)
            ));
        }
        (!parts.is_empty()).then(|| format!("[Chummy: {}]", parts.join("; ")))
    }
}

fn plural(n: usize, what: &str) -> String {
    format!("{n} {what}{}", if n == 1 { "" } else { "s" })
}

fn list(items: &[String]) -> String {
    let mut s: Vec<&str> = items.iter().take(5).map(String::as_str).collect();
    if items.len() > 5 {
        s.push("…");
    }
    s.join(", ")
}

fn join(rel: &str, name: &str) -> String {
    if rel.is_empty() {
        name.to_string()
    } else {
        format!("{rel}/{name}")
    }
}

fn too_many(limits: Limits) -> String {
    format!(
        "The workspace has more than {} files or {} MB, too much to copy into the sandbox for each tool call.",
        limits.max_files,
        limits.max_bytes / (1024 * 1024)
    )
}

/// Copies the workspace into `dest` (which must be empty or missing).
pub fn stage(jail: &Jail, dest: &Path, limits: Limits) -> Result<Snapshot, String> {
    std::fs::create_dir_all(dest).map_err(|e| format!("couldn't prepare the copy: {e}"))?;
    let mut snap = Snapshot::default();
    let mut todo = vec![String::new()];
    while let Some(rel) = todo.pop() {
        let entries = jail.list(&rel).map_err(|e| e.to_string())?;
        for e in entries {
            let child = join(&rel, &e.name);
            let target = dest.join(&child);
            if e.is_dir {
                std::fs::create_dir_all(&target).map_err(|e| format!("couldn't copy: {e}"))?;
                snap.dirs.insert(child.clone());
                todo.push(child);
            } else if e.is_file {
                if snap.files.len() >= limits.max_files {
                    return Err(too_many(limits));
                }
                let room = limits.max_bytes.saturating_sub(snap.bytes);
                let bytes = jail.read_bytes(&child, room).map_err(|err| match err {
                    chummy_sandbox::JailError::TooLarge { .. } => too_many(limits),
                    other => other.to_string(),
                })?;
                snap.bytes += bytes.len() as u64;
                std::fs::write(&target, &bytes).map_err(|e| format!("couldn't copy: {e}"))?;
                snap.files.insert(child, Sha256::digest(&bytes).into());
            } else {
                snap.skipped.push(child);
            }
        }
    }
    Ok(snap)
}

/// What came back, read without following links.
struct Back {
    files: BTreeMap<String, PathBuf>,
    dirs: BTreeSet<String>,
    ignored: Vec<String>,
}

fn scan(root: &Path, limits: Limits) -> Result<Back, String> {
    let mut back = Back {
        files: BTreeMap::new(),
        dirs: BTreeSet::new(),
        ignored: vec![],
    };
    let mut bytes = 0u64;
    let mut todo = vec![String::new()];
    while let Some(rel) = todo.pop() {
        let dir = if rel.is_empty() {
            root.to_path_buf()
        } else {
            root.join(&rel)
        };
        let rd = std::fs::read_dir(&dir).map_err(|e| format!("couldn't read the results: {e}"))?;
        for entry in rd {
            let entry = entry.map_err(|e| format!("couldn't read the results: {e}"))?;
            let Some(name) = entry.file_name().to_str().map(str::to_string) else {
                back.ignored
                    .push(join(&rel, &entry.file_name().to_string_lossy()));
                continue;
            };
            let child = join(&rel, &name);
            // symlink_metadata: a link is reported as a link, never followed.
            let meta = std::fs::symlink_metadata(entry.path())
                .map_err(|e| format!("couldn't read the results: {e}"))?;
            if meta.is_dir() {
                back.dirs.insert(child.clone());
                todo.push(child);
            } else if meta.is_file() {
                bytes += meta.len();
                if back.files.len() >= limits.max_files || bytes > limits.max_bytes {
                    return Err(format!(
                        "The tool left more than {} files or {} MB in the workspace, so Chummy didn't copy its changes back.",
                        limits.max_files,
                        limits.max_bytes / (1024 * 1024)
                    ));
                }
                back.files.insert(child, entry.path());
            } else {
                back.ignored.push(child);
            }
        }
    }
    Ok(back)
}

/// The hash of a workspace file now, or `None` if it isn't a file there.
fn current(jail: &Jail, rel: &str, limits: Limits) -> Option<[u8; 32]> {
    jail.read_bytes(rel, limits.max_bytes)
        .ok()
        .map(|b| Sha256::digest(&b).into())
}

/// Brings the changes in `back` (the workspace copy after the call) into
/// the workspace.
pub fn apply(
    jail: &Jail,
    before: &Snapshot,
    back: &Path,
    limits: Limits,
) -> Result<Report, String> {
    let after = scan(back, limits)?;
    let mut report = Report {
        ignored: after.ignored.clone(),
        ..Default::default()
    };
    // Folders first, so files have somewhere to go.
    for d in after.dirs.difference(&before.dirs) {
        if jail.create_dir(d).is_err() {
            report.ignored.push(d.clone());
        }
    }
    for (rel, path) in &after.files {
        let bytes = std::fs::read(path).map_err(|e| format!("couldn't read the results: {e}"))?;
        let hash: [u8; 32] = Sha256::digest(&bytes).into();
        let was = before.files.get(rel);
        if was == Some(&hash) {
            continue;
        }
        if current(jail, rel, limits).as_ref() != was {
            report.conflicts.push(rel.clone());
            continue;
        }
        match jail.write(rel, &bytes) {
            Ok(()) => report.written.push(rel.clone()),
            Err(_) => report.ignored.push(rel.clone()),
        }
    }
    for (rel, hash) in &before.files {
        if after.files.contains_key(rel) {
            continue;
        }
        if current(jail, rel, limits).as_ref() != Some(hash) {
            // Gone from the copy, but you changed it meanwhile (or it's
            // already gone): leave it.
            if jail.exists(rel).unwrap_or(false) {
                report.conflicts.push(rel.clone());
            }
            continue;
        }
        if jail.remove_file(rel).is_ok() {
            report.deleted.push(rel.clone());
        }
    }
    // Folders the tool removed: deepest first; ones that aren't empty stay.
    let mut gone: Vec<&String> = before.dirs.difference(&after.dirs).collect();
    gone.sort_by_key(|d| std::cmp::Reverse(d.matches('/').count()));
    for d in gone {
        if jail.remove_dir(d).is_ok() {
            report.deleted.push(format!("{d}/"));
        }
    }
    Ok(report)
}

#[cfg(test)]
mod tests {
    use super::*;
    use chummy_sandbox::Protected;

    fn jail(dir: &Path) -> Jail {
        Jail::open(dir, &Protected::none()).unwrap()
    }

    fn setup() -> (tempfile::TempDir, tempfile::TempDir, Jail) {
        let ws = tempfile::tempdir().unwrap();
        let stage_dir = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(ws.path().join("notes/old")).unwrap();
        std::fs::write(ws.path().join("a.txt"), "a").unwrap();
        std::fs::write(ws.path().join("notes/b.txt"), "b").unwrap();
        std::fs::write(ws.path().join("notes/old/c.txt"), "c").unwrap();
        let j = jail(ws.path());
        (ws, stage_dir, j)
    }

    #[test]
    fn a_round_trip_brings_back_new_changed_and_deleted_files() {
        let (ws, st, j) = setup();
        let copy = st.path().join("workspace");
        let snap = stage(&j, &copy, LIMITS).unwrap();
        assert_eq!(snap.file_count(), 3);
        assert_eq!(
            std::fs::read_to_string(copy.join("notes/b.txt")).unwrap(),
            "b"
        );

        // The "tool": change a, add d, delete the old folder.
        std::fs::write(copy.join("a.txt"), "A!").unwrap();
        std::fs::create_dir_all(copy.join("new")).unwrap();
        std::fs::write(copy.join("new/d.txt"), "d").unwrap();
        std::fs::remove_dir_all(copy.join("notes/old")).unwrap();

        let r = apply(&j, &snap, &copy, LIMITS).unwrap();
        assert_eq!(
            std::fs::read_to_string(ws.path().join("a.txt")).unwrap(),
            "A!"
        );
        assert_eq!(
            std::fs::read_to_string(ws.path().join("new/d.txt")).unwrap(),
            "d"
        );
        assert!(!ws.path().join("notes/old").exists());
        assert!(ws.path().join("notes/b.txt").exists());
        assert!(r.conflicts.is_empty() && r.note().is_none(), "{r:?}");
        assert_eq!(r.written.len(), 2);
    }

    #[test]
    fn your_changes_during_a_call_win() {
        let (ws, st, j) = setup();
        let copy = st.path().join("workspace");
        let snap = stage(&j, &copy, LIMITS).unwrap();
        std::fs::write(copy.join("a.txt"), "tool").unwrap();
        std::fs::remove_file(copy.join("notes/b.txt")).unwrap();
        // Meanwhile, you edit both.
        std::fs::write(ws.path().join("a.txt"), "you").unwrap();
        std::fs::write(ws.path().join("notes/b.txt"), "you too").unwrap();

        let r = apply(&j, &snap, &copy, LIMITS).unwrap();
        assert_eq!(
            std::fs::read_to_string(ws.path().join("a.txt")).unwrap(),
            "you"
        );
        assert_eq!(
            std::fs::read_to_string(ws.path().join("notes/b.txt")).unwrap(),
            "you too"
        );
        assert_eq!(r.conflicts.len(), 2);
        assert!(r.note().unwrap().contains("kept your version"));
    }

    #[cfg(unix)]
    #[test]
    fn links_never_travel_either_way() {
        let (ws, st, j) = setup();
        let outside = tempfile::tempdir().unwrap();
        std::fs::write(outside.path().join("secret.txt"), "secret").unwrap();
        std::os::unix::fs::symlink(outside.path().join("secret.txt"), ws.path().join("link"))
            .unwrap();
        let copy = st.path().join("workspace");
        let snap = stage(&j, &copy, LIMITS).unwrap();
        assert!(!copy.join("link").exists(), "links aren't copied in");
        assert_eq!(snap.skipped, vec!["link".to_string()]);

        // The tool plants a link pointing outside; it must not come back.
        std::os::unix::fs::symlink("/etc/passwd", copy.join("planted")).unwrap();
        let r = apply(&j, &snap, &copy, LIMITS).unwrap();
        assert!(std::fs::symlink_metadata(ws.path().join("planted")).is_err());
        assert_eq!(r.ignored, vec!["planted".to_string()]);
        // And the original link (left out of the copy) isn't deleted.
        assert!(std::fs::symlink_metadata(ws.path().join("link")).is_ok());
        assert_eq!(
            std::fs::read_to_string(outside.path().join("secret.txt")).unwrap(),
            "secret"
        );
    }

    #[test]
    fn too_much_is_refused_both_ways() {
        let (_ws, st, j) = setup();
        let tiny = Limits {
            max_files: 2,
            max_bytes: 1024,
        };
        let err = stage(&j, &st.path().join("w"), tiny).unwrap_err();
        assert!(err.contains("too much to copy"), "{err}");

        let copy = st.path().join("ok");
        let snap = stage(&j, &copy, LIMITS).unwrap();
        for i in 0..5 {
            std::fs::write(copy.join(format!("x{i}")), "x").unwrap();
        }
        let err = apply(&j, &snap, &copy, tiny).unwrap_err();
        assert!(err.contains("didn't copy its changes back"), "{err}");
    }
}
