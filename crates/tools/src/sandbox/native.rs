//! The native sandbox: CPython for WebAssembly under Wasmtime, built into
//! Chummy (see [`crate::host`]). Nothing to install beyond the tool runtime
//! Chummy downloads itself. Tools can't start other programs here.

use std::sync::Arc;

use super::{BoxFut, Health, SandboxBackend, SandboxCall, SandboxKind};
use crate::ToolError;
use crate::host::{CallOutcome, CallSpec, ToolHost};
use crate::python::{ToolPaths, python_status};

pub struct NativeSandbox {
    host: Arc<ToolHost>,
}

impl NativeSandbox {
    pub fn new(host: Arc<ToolHost>) -> Self {
        Self { host }
    }

    /// Health without compiling Python (the app checks this often).
    pub fn health(paths: &ToolPaths) -> Health {
        match python_status(paths) {
            Some(info) => Health::ready(
                SandboxKind::Native,
                info.version,
                "Tools run in WebAssembly, built into Chummy.",
            ),
            None => Health::problem(
                SandboxKind::Native,
                "The tool runtime isn't set up yet. Set it up in Tools.",
            ),
        }
    }
}

impl SandboxBackend for NativeSandbox {
    fn kind(&self) -> SandboxKind {
        SandboxKind::Native
    }

    fn check(&self) -> BoxFut<'_, Health> {
        Box::pin(async move { Self::health(self.host.paths()) })
    }

    fn call<'a>(&'a self, call: SandboxCall<'a>) -> BoxFut<'a, Result<CallOutcome, ToolError>> {
        Box::pin(async move {
            let root = call.jail.map(|j| j.root().to_path_buf());
            self.host
                .call(CallSpec {
                    tool_dir: call.tool_dir,
                    mode: call.mode,
                    workspace: root.as_deref(),
                    network: call.network,
                    timeout: call.timeout,
                    env: call.env,
                })
                .await
        })
    }
}
