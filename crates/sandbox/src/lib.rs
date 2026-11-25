//! # chummy-sandbox
//!
//! The single enforcement point for what an agent may touch.
//!
//! Every agent is locked to one folder the user chose and confirmed: its
//! workspace. The protection has three layers:
//!
//! 1. **Choosing a workspace** ([`validate_workspace`]). A folder is refused if
//!    it is a drive root or mount point, the home folder, a hidden settings
//!    folder in home, a system folder, or Chummy's own folder, or if it
//!    *contains* any of those. These rules apply even when the user asks
//!    otherwise. See [`Protected`].
//! 2. **Naming paths** ([`RelPath`]). Agents only ever give paths relative to
//!    the workspace. Absolute paths, `..`, drive prefixes and similar tricks are
//!    rejected before touching the disk.
//! 3. **Touching files** ([`Jail`]). All file access goes through a
//!    capability handle (`cap-std`) opened on the workspace. Every path is
//!    resolved beneath that handle, so symlinks, junctions and races that try
//!    to escape fail at the OS level, not just in a check we wrote.
//!
//! Beyond file access inside the workspace, everything needs an explicit
//! [`Grant`], and the default [`Grants`] set is empty.

mod jail;
mod paths;
mod protected;
mod workspace;

use std::collections::BTreeSet;

use serde::{Deserialize, Serialize};

pub use jail::{DirEntry, Jail, JailError};
pub use paths::{RelPath, RelPathError};
pub use protected::{Protected, ProtectedPath, Scope};
pub use workspace::{WorkspaceError, validate_workspace, workspace_advice};

/// True if one folder is the other or contains it (case-insensitive where the
/// platform is). Both paths should be canonical. Used to keep every agent's
/// workspace separate.
pub fn paths_overlap(a: &std::path::Path, b: &std::path::Path) -> bool {
    paths::is_within(a, b) || paths::is_within(b, a)
}

/// A capability beyond plain file access inside the workspace.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Grant {
    /// Outbound network access, optionally limited to listed hosts.
    Network { hosts: Vec<String> },
    /// Permission to run one named tool.
    Tool { name: String },
    /// Permission to execute a program on the host, outside the Wasm runtime.
    HostExec { program: String },
}

/// The set of grants an agent holds. Deny-by-default: [`Grants::default`] is empty.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Grants(BTreeSet<Grant>);

impl Grants {
    pub fn allows(&self, grant: &Grant) -> bool {
        self.0.contains(grant)
    }

    /// Records a grant. Callers must have obtained user confirmation first.
    pub fn insert(&mut self, grant: Grant) {
        self.0.insert(grant);
    }

    pub fn revoke(&mut self, grant: &Grant) {
        self.0.remove(grant);
    }

    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn grants_deny_by_default() {
        let grants = Grants::default();
        assert!(grants.is_empty());
        assert!(!grants.allows(&Grant::Network { hosts: vec![] }));

        // (was grants_insert_and_revoke)
        {
            let mut grants = Grants::default();
            let net = Grant::Network {
                hosts: vec!["example.com".into()],
            };
            grants.insert(net.clone());
            assert!(grants.allows(&net));
            grants.revoke(&net);
            assert!(!grants.allows(&net));
        }
    }

    #[test]
    fn overlap_is_either_direction() {
        use std::path::Path;
        assert!(paths_overlap(Path::new("/a/b"), Path::new("/a/b")));
        assert!(paths_overlap(Path::new("/a/b/c"), Path::new("/a/b")));
        assert!(paths_overlap(Path::new("/a"), Path::new("/a/b")));
        assert!(!paths_overlap(Path::new("/a/b"), Path::new("/a/bc")));
        assert!(!paths_overlap(Path::new("/a/x"), Path::new("/a/y")));
    }
}
