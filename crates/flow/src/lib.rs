//! # chummy-flow
//!
//! Agent loops as graphs.
//!
//! A loop is a versioned JSON document of nodes and edges. The shape mirrors
//! what the xyflow editor works with (`id`, `type`, `position`, `data`), so the
//! editor can save and load loops directly. Loops are interpreted here in Rust,
//! not in the editor window, so runs keep going when the editor is closed and
//! the scheduler can dispatch them with no window open.
//!
//! Status: pass 1 defines the document schema. The node registry and
//! interpreter arrive in pass 3.

use serde::{Deserialize, Serialize};

/// Current loop document schema version. Bump on breaking changes and add a
/// migration.
pub const SCHEMA_VERSION: u32 = 1;

/// A complete loop document, as saved to and loaded from `.json` files.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct LoopGraph {
    pub schema_version: u32,
    pub name: String,
    #[serde(default)]
    pub description: String,
    pub nodes: Vec<Node>,
    pub edges: Vec<Edge>,
}

impl LoopGraph {
    /// A new, empty loop.
    pub fn empty(name: impl Into<String>) -> Self {
        Self {
            schema_version: SCHEMA_VERSION,
            name: name.into(),
            description: String::new(),
            nodes: Vec::new(),
            edges: Vec::new(),
        }
    }

    /// Parses a loop from JSON, rejecting documents from a newer schema.
    pub fn from_json(json: &str) -> Result<Self, FlowError> {
        let graph: LoopGraph = serde_json::from_str(json)?;
        if graph.schema_version > SCHEMA_VERSION {
            return Err(FlowError::UnsupportedVersion(graph.schema_version));
        }
        Ok(graph)
    }
}

/// One block in a loop.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Node {
    pub id: String,
    /// Node type id from the registry, e.g. `llm_call`.
    #[serde(rename = "type")]
    pub kind: String,
    pub position: Position,
    /// Node-specific settings, validated by the node type.
    #[serde(default)]
    pub data: serde_json::Value,
}

/// Canvas position, used by the editor only.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct Position {
    pub x: f64,
    pub y: f64,
}

/// A connection from one node's output handle to another node's input handle.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Edge {
    pub id: String,
    pub source: String,
    pub target: String,
    #[serde(
        default,
        rename = "sourceHandle",
        skip_serializing_if = "Option::is_none"
    )]
    pub source_handle: Option<String>,
    #[serde(
        default,
        rename = "targetHandle",
        skip_serializing_if = "Option::is_none"
    )]
    pub target_handle: Option<String>,
}

#[derive(Debug, thiserror::Error)]
pub enum FlowError {
    #[error("invalid loop document: {0}")]
    Parse(#[from] serde_json::Error),
    #[error("loop uses schema version {0}, which is newer than this app supports")]
    UnsupportedVersion(u32),
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_xyflow_shaped_document() {
        let json = r#"{
            "schema_version": 1,
            "name": "demo",
            "nodes": [
                { "id": "a", "type": "input", "position": { "x": 0, "y": 0 } },
                { "id": "b", "type": "llm_call", "position": { "x": 200, "y": 0 },
                  "data": { "prompt": "Summarize." } }
            ],
            "edges": [ { "id": "a-b", "source": "a", "target": "b" } ]
        }"#;
        let graph = LoopGraph::from_json(json).unwrap();
        assert_eq!(graph.nodes.len(), 2);
        assert_eq!(graph.nodes[1].kind, "llm_call");
        assert_eq!(graph.edges[0].target, "b");
    }

    #[test]
    fn rejects_newer_schema() {
        let mut graph = LoopGraph::empty("future");
        graph.schema_version = SCHEMA_VERSION + 1;
        let json = serde_json::to_string(&graph).unwrap();
        assert!(matches!(
            LoopGraph::from_json(&json),
            Err(FlowError::UnsupportedVersion(_))
        ));
    }
}
