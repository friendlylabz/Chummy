//! The loop library: `loops/<id>.json` in the Chummy folder.

use std::path::{Path, PathBuf};

use chummy_flow::{LoopFile, analyze};
use serde::Serialize;

#[derive(Debug, Clone, Serialize)]
pub struct LoopSummary {
    pub id: String,
    pub name: String,
    pub description: String,
    pub blocks: usize,
    /// No validation errors.
    pub ok: bool,
    pub modified: u64,
}

pub struct Library {
    dir: PathBuf,
}

/// Loop ids become file names, so they are restricted to a safe alphabet.
pub fn valid_id(id: &str) -> bool {
    !id.is_empty()
        && id.len() <= 80
        && !id.starts_with('.')
        && id
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
}

/// Fingerprints of built-in loops that have since been replaced. A saved
/// copy that still matches one exactly was never edited, so it's upgraded.
/// Add the old built-in's fingerprint here whenever a built-in changes.
const RETIRED: &[&str] = &[
    // Chummy assistant, format 3, before "one go, or plan fully first" (Nov 2026).
    "824d111054b29bc81b7a6d16b6d33e77050c8cac08f12df830ade4a77d54a8f2",
    // Chummy assistant "one go, or plan fully first" (pass 10), replaced by the
    // one agent loop (pass 11, Nov 2026).
    "5feeeda2a0c5fd2c5e899f8544000e0ff224be2900b59c43007bd012a76edd63",
];

/// A loop's content as a SHA-256 hex string.
pub fn fingerprint(l: &LoopFile) -> String {
    use sha2::{Digest, Sha256};
    Sha256::digest(l.to_json().as_bytes())
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect()
}

impl Library {
    pub fn new(dir: &Path) -> Self {
        Self {
            dir: dir.to_path_buf(),
        }
    }

    /// Saved copies of built-in loops that are out of date are replaced by
    /// the current built-in with the same name, keeping their id (so agents
    /// keep using them): copies saved before loop format 3, and unedited
    /// copies of a retired version (see [`RETIRED`]). Edited copies of
    /// format 3 or later are left alone.
    pub fn upgrade_builtins(&self) -> Vec<String> {
        self.upgrade_builtins_with(RETIRED)
    }

    fn upgrade_builtins_with(&self, retired_prints: &[&str]) -> Vec<String> {
        let builtins = chummy_flow::templates();
        let mut done = Vec::new();
        for e in std::fs::read_dir(&self.dir).into_iter().flatten().flatten() {
            let p = e.path();
            if p.extension().is_none_or(|x| x != "json") {
                continue;
            }
            let Ok(raw) = std::fs::read_to_string(&p) else {
                continue;
            };
            let Ok(json) = serde_json::from_str::<serde_json::Value>(&raw) else {
                continue;
            };
            let name = json.get("name").and_then(|v| v.as_str()).unwrap_or("");
            let Some(t) = builtins.iter().find(|t| t.file.name == name) else {
                continue;
            };
            let old_format = json.get("version").and_then(|v| v.as_u64()).unwrap_or(0) < 3;
            let retired = || {
                LoopFile::from_json(&raw)
                    .is_ok_and(|l| retired_prints.contains(&fingerprint(&l).as_str()))
            };
            if !old_format && !retired() {
                continue;
            }
            if std::fs::write(&p, t.file.to_json()).is_ok() {
                done.push(name.to_string());
            }
        }
        done
    }

    fn path(&self, id: &str) -> Result<PathBuf, String> {
        if !valid_id(id) {
            return Err("That loop id isn't valid.".into());
        }
        Ok(self.dir.join(format!("{id}.json")))
    }

    pub fn list(&self) -> Vec<LoopSummary> {
        let mut out = Vec::new();
        for e in std::fs::read_dir(&self.dir).into_iter().flatten().flatten() {
            let p = e.path();
            if p.extension().is_none_or(|x| x != "json") {
                continue;
            }
            let Some(id) = p
                .file_stem()
                .and_then(|s| s.to_str())
                .filter(|s| valid_id(s))
            else {
                continue;
            };
            let modified = e
                .metadata()
                .and_then(|m| m.modified())
                .ok()
                .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
                .map(|d| d.as_secs())
                .unwrap_or(0);
            match std::fs::read_to_string(&p)
                .map_err(|e| e.to_string())
                .and_then(|s| LoopFile::from_json(&s).map_err(|e| e.to_string()))
            {
                Ok(l) => out.push(LoopSummary {
                    id: id.into(),
                    ok: analyze(&l).ok,
                    blocks: l.blocks.len(),
                    name: l.name,
                    description: l.description,
                    modified,
                }),
                Err(e) => out.push(LoopSummary {
                    id: id.into(),
                    name: id.into(),
                    description: format!("Can't be opened: {e}"),
                    blocks: 0,
                    ok: false,
                    modified,
                }),
            }
        }
        out.sort_by_key(|a| a.name.to_lowercase());
        out
    }

    pub fn load(&self, id: &str) -> Result<LoopFile, String> {
        let text = std::fs::read_to_string(self.path(id)?)
            .map_err(|_| "That loop no longer exists.".to_string())?;
        LoopFile::from_json(&text).map_err(|e| e.to_string())
    }

    /// Saves atomically. Loops with validation errors may be saved (work in
    /// progress); agents just can't run them until they're fixed.
    /// Saves a loop, always in the current format version.
    pub fn save(&self, id: &str, file: &LoopFile) -> Result<(), String> {
        let path = self.path(id)?;
        let mut file = file.clone();
        file.version = chummy_flow::VERSION;
        let tmp = self.dir.join(format!("{id}.json.tmp"));
        {
            use std::io::Write;
            let mut f = std::fs::File::create(&tmp).map_err(|e| e.to_string())?;
            f.write_all(file.to_json().as_bytes())
                .and_then(|_| f.sync_all())
                .map_err(|e| e.to_string())?;
        }
        std::fs::rename(&tmp, &path).map_err(|e| e.to_string())
    }

    pub fn delete(&self, id: &str) -> Result<(), String> {
        let path = self.path(id)?;
        if path.exists() {
            std::fs::remove_file(path).map_err(|e| e.to_string())?;
        }
        Ok(())
    }

    /// A new unused id based on a name.
    pub fn new_id(&self, name: &str) -> String {
        let base: String = name
            .to_lowercase()
            .chars()
            .map(|c| if c.is_ascii_alphanumeric() { c } else { '-' })
            .collect::<String>()
            .split('-')
            .filter(|s| !s.is_empty())
            .collect::<Vec<_>>()
            .join("-");
        let base = if base.is_empty() {
            "loop".to_string()
        } else {
            base.chars().take(48).collect()
        };
        let mut id = base.clone();
        let mut n = 2;
        while self.dir.join(format!("{id}.json")).exists() {
            id = format!("{base}-{n}");
            n += 1;
        }
        id
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn save_list_load_delete() {
        let tmp = tempfile::tempdir().unwrap();
        let lib = Library::new(tmp.path());
        let file = chummy_flow::templates()
            .into_iter()
            .find(|t| t.id == "simple_chat")
            .unwrap()
            .file;
        let id = lib.new_id(&file.name);
        assert_eq!(id, "simple-chat");
        lib.save(&id, &file).unwrap();
        assert_eq!(lib.new_id(&file.name), "simple-chat-2");
        let list = lib.list();
        assert_eq!(list.len(), 1);
        assert!(list[0].ok);
        assert_eq!(lib.load(&id).unwrap(), file);
        lib.delete(&id).unwrap();
        assert!(lib.list().is_empty());
        assert!(lib.load("../../etc/passwd").is_err());
    }

    #[test]
    fn old_copies_of_built_in_loops_are_replaced() {
        let tmp = tempfile::tempdir().unwrap();
        let lib = Library::new(tmp.path());
        let current = chummy_flow::templates()
            .into_iter()
            .find(|t| t.id == chummy_flow::ASSISTANT)
            .unwrap()
            .file;
        let old = format!(
            r#"{{"format":"chummy.loop","version":2,"name":"{}","blocks":[]}}"#,
            current.name
        );
        std::fs::write(tmp.path().join("mine.json"), &old).unwrap();
        let other = r#"{"format":"chummy.loop","version":2,"name":"My own","blocks":[]}"#;
        std::fs::write(tmp.path().join("other.json"), other).unwrap();
        assert_eq!(lib.upgrade_builtins(), std::slice::from_ref(&current.name));
        assert_eq!(lib.load("mine").unwrap(), current);
        assert_eq!(
            std::fs::read_to_string(tmp.path().join("other.json")).unwrap(),
            other
        );
        assert!(lib.upgrade_builtins().is_empty(), "only once");
    }

    #[test]
    fn unedited_copies_of_retired_built_ins_are_replaced() {
        let tmp = tempfile::tempdir().unwrap();
        let lib = Library::new(tmp.path());
        let current = chummy_flow::templates()
            .into_iter()
            .find(|t| t.id == chummy_flow::ASSISTANT)
            .unwrap()
            .file;
        // Stand-ins for an old version, and for an edited copy of it.
        let mut old = current.clone();
        old.description = "An older version.".into();
        let mut edited = old.clone();
        edited.limits.max_steps = 123;
        std::fs::write(tmp.path().join("old.json"), old.to_json()).unwrap();
        std::fs::write(tmp.path().join("edited.json"), edited.to_json()).unwrap();
        let print = fingerprint(&old);
        assert_eq!(
            lib.upgrade_builtins_with(&[print.as_str()]),
            std::slice::from_ref(&current.name)
        );
        assert_eq!(lib.load("old").unwrap(), current);
        assert_eq!(lib.load("edited").unwrap(), edited, "edits are kept");
        // The real list holds proper fingerprints.
        assert!(
            RETIRED
                .iter()
                .all(|p| p.len() == 64 && p.chars().all(|c| c.is_ascii_hexdigit()))
        );
        assert!(
            !RETIRED.contains(&fingerprint(&current).as_str()),
            "the current one isn't retired"
        );
    }
}
