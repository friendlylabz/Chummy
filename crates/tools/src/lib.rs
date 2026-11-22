//! # chummy-tools
//!
//! Tools are single Python files, run as WebAssembly.
//!
//! - **Format** ([`meta`]): a Python script with a standard PEP 723 header
//!   (`requires-python`, `dependencies`) and a `[tool.chummy]` table (name,
//!   title, version, whether it wants the internet, timeout). One function is
//!   marked `@chummy.tool`; its signature and docstring become the
//!   description and parameters the model sees.
//! - **Dependencies** ([`lock`]): resolved to a PEP 751 `pylock.toml` of
//!   pure-Python wheels with SHA-256 hashes, then unpacked by Chummy itself.
//! - **Runtime** ([`python`]): the official CPython WASI build, downloaded
//!   once and checksum-verified.
//! - **Sandbox** ([`host`]): Wasmtime. A tool sees the agent's workspace and
//!   its own read-only files, nothing else. It has no network of its own;
//!   `chummy.http` goes through the app ([`net`]), which allows it only when
//!   the tool's internet access is on, and never to the local network.
//!
//! There is no unsandboxed mode.

pub mod host;
pub mod install;
pub mod lock;
pub mod meta;
pub mod net;
pub mod python;

use std::path::Path;
use std::time::Duration;

pub use host::{CallOutcome, CallSpec, Mode, ToolHost};
pub use install::{
    Builtin, ToolManifest, builtins, install, install_builtins, installed, manifest,
};
pub use lock::{Package, PyLock, UvSetup};
pub use meta::{ToolMeta, parse_meta};
pub use net::NetPolicy;
pub use python::{
    PYTHON_DOWNLOAD_BYTES, PYTHON_VERSION, PythonInfo, ToolPaths, install_python, python_status,
};

#[derive(Debug, thiserror::Error)]
pub enum ToolError {
    #[error("this tool file isn't valid: {0}")]
    Definition(String),
    #[error("{0}")]
    Dependencies(String),
    #[error("couldn't reach {0}; check your internet connection")]
    Network(String),
    #[error("{0}")]
    Runtime(String),
    #[error("the tool “{0}” isn't installed")]
    NotInstalled(String),
    #[error("the tool took longer than {0} seconds and was stopped")]
    Timeout(u32),
    #[error("the tool crashed: {0}")]
    Crashed(String),
    #[error("{what}: {source}")]
    Io {
        what: &'static str,
        #[source]
        source: std::io::Error,
    },
}

impl ToolError {
    pub(crate) fn io(what: &'static str) -> impl FnOnce(std::io::Error) -> Self {
        move |source| Self::Io { what, source }
    }
}

/// A finished tool call, ready to show the model.
#[derive(Debug, Clone, PartialEq)]
pub struct ToolResult {
    /// The text the model sees.
    pub text: String,
    /// The tool reported a problem (the text says what).
    pub is_error: bool,
    pub log: String,
}

impl ToolHost {
    /// Calls an installed tool. Problems the tool reports (a missing file,
    /// bad arguments) come back as an error result for the model; only
    /// sandbox failures are `Err`.
    pub async fn run(
        &self,
        name: &str,
        args: serde_json::Value,
        workspace: Option<&Path>,
        network: NetPolicy,
        env: Vec<(String, String)>,
    ) -> Result<ToolResult, ToolError> {
        let m = manifest(self.paths(), name).ok_or_else(|| ToolError::NotInstalled(name.into()))?;
        install::verify(self.paths(), &m)?;
        let dir = self.tool_dir(name);
        let out = self
            .call(CallSpec {
                tool_dir: &dir,
                mode: Mode::Call(args),
                workspace,
                network,
                timeout: Duration::from_secs(m.timeout_secs as u64),
                env,
            })
            .await?;
        Ok(match out.result {
            Ok(serde_json::Value::String(s)) => ToolResult {
                text: s,
                is_error: false,
                log: out.log,
            },
            Ok(v) => ToolResult {
                text: serde_json::to_string_pretty(&v).unwrap_or_default(),
                is_error: false,
                log: out.log,
            },
            Err((_, msg)) => ToolResult {
                text: format!("Error: {msg}"),
                is_error: true,
                log: out.log,
            },
        })
    }
}
