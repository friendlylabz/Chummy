//! # chummy-sandbox
//!
//! The single enforcement point for what an agent may touch.
//!
//! Every agent is locked to one folder that the user chose and confirmed (its
//! [`Workspace`]). Anything beyond reading and writing inside that folder (network
//! access, running a tool, executing on the host) needs an explicit [`Grant`],
//! and the default [`Grants`] set is empty.
//!
//! The UI never makes these decisions. It can only ask Rust to change them, and
//! Rust asks the user to confirm.
//!
//! Status: pass 1 defines the types. Path resolution (resolving the real path,
//! following symlinks, refusing escapes) arrives in pass 2.

use std::collections::BTreeSet;
use std::path::PathBuf;

use serde::{Deserialize, Serialize};

/// A folder the user picked and confirmed as an agent's whole world.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Workspace {
    /// Absolute, canonical path of the jail root.
    pub root: PathBuf,
    /// Whether the user explicitly confirmed this folder. Unconfirmed
    /// workspaces must never be handed to an agent.
    pub confirmed: bool,
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
    /// Returns true if `grant` has been given.
    pub fn allows(&self, grant: &Grant) -> bool {
        self.0.contains(grant)
    }

    /// Records a grant. Callers must have obtained user confirmation first.
    pub fn insert(&mut self, grant: Grant) {
        self.0.insert(grant);
    }

    /// Removes a grant.
    pub fn revoke(&mut self, grant: &Grant) {
        self.0.remove(grant);
    }

    /// True when the agent holds no grants at all.
    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }
}

/// Errors raised when an agent tries to leave its sandbox.
#[derive(Debug, thiserror::Error)]
pub enum SandboxError {
    #[error("no confirmed workspace folder is set")]
    NoWorkspace,
    #[error("path escapes the workspace: {0}")]
    Escape(PathBuf),
    #[error("not permitted without a grant: {0:?}")]
    Denied(Grant),
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn grants_deny_by_default() {
        let grants = Grants::default();
        assert!(grants.is_empty());
        assert!(!grants.allows(&Grant::Network { hosts: vec![] }));
        assert!(!grants.allows(&Grant::Tool {
            name: "read_file".into()
        }));
    }

    #[test]
    fn grants_insert_and_revoke() {
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
