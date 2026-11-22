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
            root,
        }
    }

    /// Creates every folder in the layout. On Unix the Chummy folder is made
    /// private to the current user (mode 0700).
    pub fn create(&self) -> io::Result<()> {
        for dir in [
            &self.root,
            &self.loops,
            &self.models,
            &self.runtime,
            &self.chats,
            &self.tools,
            &self.logs,
        ] {
            std::fs::create_dir_all(dir)?;
        }
        restrict_to_owner(&self.root)?;
        Ok(())
    }
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
}
