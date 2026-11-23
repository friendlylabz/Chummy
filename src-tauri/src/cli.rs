//! Command-line options.
//!
//! `--data-dir <folder>` (or `--data-dir=<folder>`) uses another Chummy
//! folder instead of the default per-user one: for a portable copy, a
//! second profile, or a folder on another disk. The folder must be empty,
//! missing (it's created), or already a Chummy folder, so Chummy never takes
//! over (and locks down) a folder full of other things.

use std::ffi::OsString;
use std::path::{Path, PathBuf};

pub const DATA_DIR_FLAG: &str = "--data-dir";

/// The folder named with `--data-dir`, if any, made absolute.
pub fn data_dir_arg(
    args: impl IntoIterator<Item = OsString>,
    cwd: &Path,
) -> Result<Option<PathBuf>, String> {
    let mut args = args.into_iter().skip(1);
    let mut found: Option<OsString> = None;
    while let Some(a) = args.next() {
        let s = a.to_string_lossy();
        if s == DATA_DIR_FLAG {
            found = Some(
                args.next()
                    .ok_or_else(|| format!("{DATA_DIR_FLAG} needs a folder after it"))?,
            );
        } else if let Some(v) = s.strip_prefix(&format!("{DATA_DIR_FLAG}=")) {
            found = Some(v.into());
        }
    }
    let Some(raw) = found else {
        return Ok(None);
    };
    if raw.is_empty() {
        return Err(format!("{DATA_DIR_FLAG} needs a folder"));
    }
    let p = PathBuf::from(raw);
    Ok(Some(if p.is_absolute() { p } else { cwd.join(p) }))
}

/// Checks a folder given with `--data-dir` and creates it if missing.
pub fn prepare_data_dir(dir: &Path) -> Result<PathBuf, String> {
    if dir.exists() && !dir.is_dir() {
        return Err(format!("{} isn't a folder", dir.display()));
    }
    if dir.is_dir() {
        let mut entries = std::fs::read_dir(dir)
            .map_err(|e| format!("can't open {}: {e}", dir.display()))?
            .filter_map(|e| e.ok());
        let is_chummy = dir.join("settings.json").is_file() || dir.join("chummy.lock").is_file();
        if !is_chummy && entries.next().is_some() {
            return Err(format!(
                "{} already has other files in it. Use an empty folder, a new one, or an existing Chummy folder.",
                dir.display()
            ));
        }
    } else {
        std::fs::create_dir_all(dir).map_err(|e| format!("can't create {}: {e}", dir.display()))?;
    }
    std::fs::canonicalize(dir).map_err(|e| format!("can't use {}: {e}", dir.display()))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn args(a: &[&str]) -> Vec<OsString> {
        std::iter::once("chummy")
            .chain(a.iter().copied())
            .map(OsString::from)
            .collect()
    }

    #[test]
    fn reads_the_flag_both_ways() {
        let cwd = Path::new("/home/me");
        assert_eq!(data_dir_arg(args(&[]), cwd).unwrap(), None);
        assert_eq!(
            data_dir_arg(args(&["--data-dir", "/x/chummy"]), cwd).unwrap(),
            Some(PathBuf::from("/x/chummy"))
        );
        assert_eq!(
            data_dir_arg(args(&["--data-dir=portable"]), cwd).unwrap(),
            Some(PathBuf::from("/home/me/portable"))
        );
        assert!(data_dir_arg(args(&["--data-dir"]), cwd).is_err());
        assert!(data_dir_arg(args(&["--data-dir="]), cwd).is_err());
    }

    #[test]
    fn only_empty_new_or_chummy_folders() {
        let tmp = tempfile::tempdir().unwrap();
        let new = tmp.path().join("new");
        assert!(prepare_data_dir(&new).unwrap().is_dir());
        assert!(prepare_data_dir(&new).is_ok(), "empty is fine");
        std::fs::write(new.join("other.txt"), "x").unwrap();
        assert!(prepare_data_dir(&new).unwrap_err().contains("other files"));
        std::fs::write(new.join("settings.json"), "{}").unwrap();
        assert!(prepare_data_dir(&new).is_ok(), "a Chummy folder is fine");
        let file = tmp.path().join("f");
        std::fs::write(&file, "x").unwrap();
        assert!(prepare_data_dir(&file).is_err());
    }
}
