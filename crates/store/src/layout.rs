use std::io;
use std::path::{Path, PathBuf};

/// The layout of the Chummy folder.
#[derive(Debug, Clone)]
pub struct ChummyDirs {
    /// The Chummy folder itself.
    pub root: PathBuf,
    /// `settings.json`: app settings.
    pub settings_file: PathBuf,
    /// Saved agent loops (`*.json`).
    pub loops: PathBuf,
    /// Weights downloaded for the built-in local engine.
    pub models: PathBuf,
    /// The built-in engine's Python runtime (installed on first use).
    pub runtime: PathBuf,
    /// Chat history, one file per agent.
    pub chats: PathBuf,
    /// Tools: the WebAssembly Python, installed tools and their packages.
    pub tools: PathBuf,
    /// Log files.
    pub logs: PathBuf,
    /// Scheduled jobs, their state and run history.
    pub jobs: PathBuf,
}

impl ChummyDirs {
    /// Describes the layout under `root` without touching the disk.
    pub fn at(root: impl Into<PathBuf>) -> Self {
        let root = root.into();
        Self {
            settings_file: root.join("settings.json"),
            loops: root.join("loops"),
            models: root.join("models"),
            runtime: root.join("runtime"),
            chats: root.join("chats"),
            tools: root.join("tools"),
            logs: root.join("logs"),
            jobs: root.join("jobs"),
            root,
        }
    }

    /// Creates every folder in the layout. On Unix the Chummy folder and
    /// everything in it is made private to the current user (see
    /// [`lock_to_owner`]).
    pub fn create(&self) -> io::Result<()> {
        for dir in [
            &self.root,
            &self.loops,
            &self.models,
            &self.runtime,
            &self.chats,
            &self.tools,
            &self.logs,
            &self.jobs,
        ] {
            std::fs::create_dir_all(dir)?;
        }
        restrict_to_owner(&self.root)?;
        lock_to_owner(&self.root)?;
        Ok(())
    }
}

/// Makes a folder and everything in it private to its owner: folders 0700,
/// files 0600, keeping the owner's execute bit (the engine's runtime has
/// programs in it). Links aren't followed. Runs at every start, so files
/// made since (or copied in by hand) are locked too. Returns how many
/// entries it changed. No-op on Windows, where the per-user app data folder
/// is already private to the user.
#[cfg(unix)]
pub fn lock_to_owner(root: &Path) -> io::Result<usize> {
    use std::os::unix::fs::PermissionsExt;
    let mut changed = 0;
    let mut todo = vec![root.to_path_buf()];
    while let Some(dir) = todo.pop() {
        for entry in std::fs::read_dir(&dir)? {
            let entry = entry?;
            let meta = std::fs::symlink_metadata(entry.path())?;
            if meta.file_type().is_symlink() {
                continue;
            }
            let mode = meta.permissions().mode() & 0o7777;
            let want = if meta.is_dir() {
                todo.push(entry.path());
                0o700
            } else {
                (mode & 0o100) | 0o600
            };
            if mode != want {
                std::fs::set_permissions(entry.path(), std::fs::Permissions::from_mode(want))?;
                changed += 1;
            }
        }
    }
    Ok(changed)
}

#[cfg(not(unix))]
pub fn lock_to_owner(_root: &Path) -> io::Result<usize> {
    Ok(0)
}

/// Whether the folder is locked to its owner (Unix: the folder is 0700).
#[cfg(unix)]
pub fn is_locked(root: &Path) -> bool {
    use std::os::unix::fs::PermissionsExt;
    std::fs::metadata(root).is_ok_and(|m| m.permissions().mode() & 0o077 == 0)
}

#[cfg(not(unix))]
pub fn is_locked(_root: &Path) -> bool {
    true
}

#[cfg(unix)]
pub(crate) fn restrict_to_owner(path: &Path) -> io::Result<()> {
    use std::os::unix::fs::PermissionsExt;
    let mode = if path.is_dir() { 0o700 } else { 0o600 };
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(mode))
}

#[cfg(not(unix))]
pub(crate) fn restrict_to_owner(_path: &Path) -> io::Result<()> {
    // Per-user app data folders on Windows are already private to the user.
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn creates_layout() {
        let tmp = tempfile::tempdir().unwrap();
        let dirs = ChummyDirs::at(tmp.path().join("chummy"));
        dirs.create().unwrap();
        assert!(dirs.loops.is_dir() && dirs.models.is_dir() && dirs.runtime.is_dir());
        assert_eq!(dirs.settings_file, tmp.path().join("chummy/settings.json"));
    }

    #[cfg(unix)]
    #[test]
    fn everything_inside_is_locked_to_the_owner() {
        use std::os::unix::fs::PermissionsExt;
        let tmp = tempfile::tempdir().unwrap();
        let dirs = ChummyDirs::at(tmp.path().join("chummy"));
        dirs.create().unwrap();
        let mode = |p: &Path| std::fs::metadata(p).unwrap().permissions().mode() & 0o7777;
        let set = |p: &Path, m: u32| {
            std::fs::set_permissions(p, std::fs::Permissions::from_mode(m)).unwrap()
        };
        std::fs::create_dir_all(dirs.runtime.join("bin")).unwrap();
        std::fs::write(&dirs.settings_file, "{}").unwrap();
        std::fs::write(dirs.runtime.join("bin/python"), "").unwrap();
        set(&dirs.settings_file, 0o644);
        set(&dirs.runtime.join("bin/python"), 0o755);
        set(&dirs.runtime.join("bin"), 0o755);
        set(&dirs.root, 0o755);
        // A link to something outside is left alone (and not followed).
        let outside = tmp.path().join("outside.txt");
        std::fs::write(&outside, "x").unwrap();
        set(&outside, 0o644);
        std::os::unix::fs::symlink(&outside, dirs.root.join("link")).unwrap();

        dirs.create().unwrap();
        assert!(is_locked(&dirs.root));
        assert_eq!(mode(&dirs.root), 0o700);
        assert_eq!(mode(&dirs.settings_file), 0o600);
        assert_eq!(mode(&dirs.runtime.join("bin")), 0o700);
        assert_eq!(
            mode(&dirs.runtime.join("bin/python")),
            0o700,
            "programs stay runnable"
        );
        assert_eq!(mode(&dirs.loops), 0o700);
        assert_eq!(mode(&outside), 0o644);
        assert_eq!(
            lock_to_owner(&dirs.root).unwrap(),
            0,
            "nothing left to change"
        );
    }
}
