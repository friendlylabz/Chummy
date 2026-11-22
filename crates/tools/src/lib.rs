//! # chummy-tools
//!
//! Script-based tools, run as WebAssembly modules.
//!
//! Each tool is a `.wasm` module plus a [`ToolManifest`]. Tools run under
//! Wasmtime with WASI. The only directory a tool can see is the agent's
//! workspace, mounted as a WASI preopen. A tool has no network access unless the
//! agent holds a matching [`chummy_sandbox::Grant`].
//!
//! Status: pass 1 defines the manifest. The Wasmtime host arrives in pass 4.

use chummy_sandbox::Grant;
use serde::{Deserialize, Serialize};

/// Describes a tool to the model and to the sandbox.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ToolManifest {
    /// Name the model calls, e.g. `read_file`.
    pub name: String,
    /// One short sentence for the model. Kept short for small models.
    pub description: String,
    /// JSON Schema for the tool's arguments.
    pub parameters: serde_json::Value,
    /// Grants the tool needs beyond workspace file access.
    #[serde(default)]
    pub requires: Vec<Grant>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn manifest_defaults_to_no_extra_grants() {
        let json = r#"{ "name": "read_file", "description": "Read a file.",
                        "parameters": { "type": "object" } }"#;
        let manifest: ToolManifest = serde_json::from_str(json).unwrap();
        assert!(manifest.requires.is_empty());
    }
}
