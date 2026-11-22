//! Places no workspace may be, or contain.

use std::path::{Path, PathBuf};

use crate::paths::is_within;

/// How far a protection reaches.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Scope {
    /// The folder, everything inside it, and every folder containing it.
    Subtree,
    /// Only the folder itself and folders containing it. Subfolders are fine
    /// (e.g. `/media` is refused, `/media/me/usb/projects` is allowed).
    ExactOnly,
}

#[derive(Debug, Clone)]
pub struct ProtectedPath {
    /// Canonical path when the folder exists, otherwise as given.
    pub path: PathBuf,
    /// Short, user-facing description ("Chummy's own folder").
    pub label: String,
    pub scope: Scope,
}

/// The set of protected locations for the current user.
#[derive(Debug, Clone, Default)]
pub struct Protected {
    pub(crate) home: Option<PathBuf>,
    pub(crate) paths: Vec<ProtectedPath>,
}

impl Protected {
    /// No protections at all. Only for tests of other behaviour.
    pub fn none() -> Self {
        Self::default()
    }

    /// The standard protections for this user and machine.
    ///
    /// `chummy_dirs` are Chummy's own folders (data folder, install folder);
    /// they are always fully off-limits.
    pub fn for_current_user<P: AsRef<Path>>(chummy_dirs: &[P]) -> Self {
        let mut p = Self {
            home: dirs::home_dir().map(|h| canonical_or(&h)),
            paths: vec![],
        };

        for dir in chummy_dirs {
            p.add(dir.as_ref(), "Chummy's own folder", Scope::Subtree);
        }

        if let Some(home) = p.home.clone() {
            p.add(&home, "your home folder", Scope::ExactOnly);
            for name in [
                ".ssh",
                ".gnupg",
                ".aws",
                ".azure",
                ".kube",
                ".docker",
                ".config",
                ".local",
                ".cache",
                ".password-store",
                ".mozilla",
                ".pki",
                "Library",
                "AppData",
            ] {
                p.add(
                    &home.join(name),
                    "a settings or credentials folder",
                    Scope::Subtree,
                );
            }
        }
        for dir in [
            dirs::config_dir(),
            dirs::data_dir(),
            dirs::data_local_dir(),
            dirs::cache_dir(),
        ]
        .into_iter()
        .flatten()
        {
            p.add(&dir, "an app data folder", Scope::Subtree);
        }

        #[cfg(unix)]
        {
            for sys in [
                "/bin",
                "/boot",
                "/dev",
                "/etc",
                "/lib",
                "/lib32",
                "/lib64",
                "/libx32",
                "/opt",
                "/proc",
                "/root",
                "/run",
                "/sbin",
                "/snap",
                "/srv",
                "/sys",
                "/usr",
                "/var",
                "/nix",
                "/System",
                "/Library",
                "/Applications",
                "/cores",
                "/private/etc",
                "/private/var",
            ] {
                p.add(Path::new(sys), "a system folder", Scope::Subtree);
            }
            for shared in [
                "/home", "/Users", "/mnt", "/media", "/Volumes", "/tmp", "/private",
            ] {
                p.add(
                    Path::new(shared),
                    "a shared top-level folder",
                    Scope::ExactOnly,
                );
            }
        }

        #[cfg(windows)]
        {
            for var in [
                "SystemRoot",
                "ProgramFiles",
                "ProgramFiles(x86)",
                "ProgramW6432",
                "ProgramData",
                "APPDATA",
                "LOCALAPPDATA",
            ] {
                if let Some(v) = std::env::var_os(var) {
                    p.add(Path::new(&v), "a system folder", Scope::Subtree);
                }
            }
            let drive = std::env::var_os("SystemDrive").unwrap_or_else(|| "C:".into());
            let root = PathBuf::from(format!("{}\\", drive.to_string_lossy()));
            for name in ["$Recycle.Bin", "System Volume Information", "Recovery"] {
                p.add(&root.join(name), "a system folder", Scope::Subtree);
            }
            p.add(
                &root.join("Users"),
                "a shared top-level folder",
                Scope::ExactOnly,
            );
        }

        p
    }

    /// Adds a protected location. Existing folders are canonicalized so that
    /// symlinked spellings (`/etc` vs `/private/etc`) are caught too; the
    /// original spelling is kept as well.
    pub fn add(&mut self, path: &Path, label: &str, scope: Scope) {
        let canon = canonical_or(path);
        for path in [canon, path.to_path_buf()] {
            if !self.paths.iter().any(|p| p.path == path) {
                self.paths.push(ProtectedPath {
                    path,
                    label: label.into(),
                    scope,
                });
            }
        }
    }

    /// The first rule `candidate` (canonical) breaks, if any.
    pub(crate) fn violation(&self, candidate: &Path) -> Option<(&ProtectedPath, bool)> {
        for p in &self.paths {
            // Candidate contains (or is) the protected folder.
            if is_within(&p.path, candidate) {
                return Some((p, false));
            }
            // Candidate is inside the protected folder. Exception: when the
            // user's home is itself inside a protected folder (root's /root,
            // Fedora Silverblue's /var/home), folders inside home are judged
            // by the home rules instead.
            if p.scope == Scope::Subtree && is_within(candidate, &p.path) {
                let in_home_below = self
                    .home
                    .as_deref()
                    .is_some_and(|h| is_within(h, &p.path) && is_within(candidate, h));
                if !in_home_below {
                    return Some((p, true));
                }
            }
        }
        None
    }
}

fn canonical_or(path: &Path) -> PathBuf {
    std::fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf())
}
