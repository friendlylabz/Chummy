//! The `chummy.loop` file format, version 1.
//!
//! ```json
//! {
//!   "format": "chummy.loop",
//!   "version": 1,
//!   "name": "Simple chat",
//!   "description": "",
//!   "limits": { "max_steps": 200, "max_llm_calls": 20 },
//!   "blocks": [
//!     { "id": "message", "type": "input", "label": "Message",
//!       "position": { "x": 0, "y": 0 }, "params": {} },
//!     { "id": "answer", "type": "llm", "label": "Answer",
//!       "position": { "x": 260, "y": 0 },
//!       "params": { "prompt": "Reply helpfully:\n\n_INPUT_" } },
//!     { "id": "reply", "type": "output", "label": "Reply",
//!       "position": { "x": 520, "y": 0 }, "params": {} }
//!   ],
//!   "links": [
//!     { "from": { "block": "message", "port": "out" }, "to": { "block": "answer", "port": "in" } },
//!     { "from": { "block": "answer", "port": "out" }, "to": { "block": "reply", "port": "in" } }
//!   ]
//! }
//! ```
//!
//! Every object rejects unknown fields, so a file means exactly one thing.
//! Block parameters are checked against the block's type (see
//! [`crate::BlockKind`]); omitted parameters take their defaults.

use serde::{Deserialize, Serialize};

use crate::FlowError;
use crate::engine::RunLimits;

pub const FORMAT: &str = "chummy.loop";
pub const VERSION: u32 = 1;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LoopFile {
    /// Always `"chummy.loop"`.
    pub format: String,
    /// Format version; this build reads version 1.
    pub version: u32,
    pub name: String,
    #[serde(default)]
    pub description: String,
    #[serde(default)]
    pub limits: RunLimits,
    pub blocks: Vec<Block>,
    #[serde(default)]
    pub links: Vec<Link>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Block {
    /// Unique within the loop. Letters, digits, `-` and `_`.
    pub id: String,
    /// Block type from the catalog, e.g. `llm`.
    #[serde(rename = "type")]
    pub kind: String,
    /// Display name. Also names `input` blocks in test runs.
    #[serde(default)]
    pub label: String,
    #[serde(default)]
    pub position: Position,
    /// Type-specific parameters.
    #[serde(default = "empty_object")]
    pub params: serde_json::Value,
}

fn empty_object() -> serde_json::Value {
    serde_json::Value::Object(Default::default())
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Position {
    pub x: f64,
    pub y: f64,
}

/// One end of a link.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PortRef {
    pub block: String,
    pub port: String,
}

/// Carries every value from an output port to an input port.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Link {
    pub from: PortRef,
    pub to: PortRef,
}

impl LoopFile {
    pub fn new(name: impl Into<String>) -> Self {
        Self {
            format: FORMAT.into(),
            version: VERSION,
            name: name.into(),
            description: String::new(),
            limits: RunLimits::default(),
            blocks: vec![],
            links: vec![],
        }
    }

    /// Parses a loop file, checking the format marker and version first so
    /// the error says what's actually wrong.
    pub fn from_json(json: &str) -> Result<Self, FlowError> {
        let raw: serde_json::Value =
            serde_json::from_str(json).map_err(|e| FlowError::Parse(e.to_string()))?;
        match raw.get("format").and_then(|f| f.as_str()) {
            Some(FORMAT) => {}
            Some(other) => return Err(FlowError::WrongFormat(other.into())),
            None => {
                return Err(FlowError::Parse(
                    "missing \"format\": \"chummy.loop\"".into(),
                ));
            }
        }
        let version = raw.get("version").and_then(|v| v.as_u64()).unwrap_or(0) as u32;
        if version > VERSION {
            return Err(FlowError::UnsupportedVersion(version));
        }
        if version == 0 {
            return Err(FlowError::Parse("missing or invalid \"version\"".into()));
        }
        serde_json::from_value(raw).map_err(|e| FlowError::Parse(e.to_string()))
    }

    pub fn to_json(&self) -> String {
        serde_json::to_string_pretty(self).unwrap_or_default() + "\n"
    }

    pub fn block(&self, id: &str) -> Option<&Block> {
        self.blocks.iter().find(|b| b.id == id)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trips() {
        let mut l = LoopFile::new("t");
        l.blocks.push(Block {
            id: "a".into(),
            kind: "input".into(),
            label: "A".into(),
            position: Position { x: 1.0, y: 2.0 },
            params: empty_object(),
        });
        let back = LoopFile::from_json(&l.to_json()).unwrap();
        assert_eq!(l, back);
    }

    #[test]
    fn rejects_ambiguity() {
        assert!(matches!(
            LoopFile::from_json("{}"),
            Err(FlowError::Parse(_))
        ));
        assert!(matches!(
            LoopFile::from_json(r#"{"format":"other","version":1}"#),
            Err(FlowError::WrongFormat(_))
        ));
        assert!(matches!(
            LoopFile::from_json(r#"{"format":"chummy.loop","version":9,"name":"x","blocks":[]}"#),
            Err(FlowError::UnsupportedVersion(9))
        ));
        // Unknown fields are errors, not silently dropped.
        let extra = r#"{"format":"chummy.loop","version":1,"name":"x","blocks":[],"nodes":[]}"#;
        assert!(LoopFile::from_json(extra).is_err());
        let extra_block = r#"{"format":"chummy.loop","version":1,"name":"x",
            "blocks":[{"id":"a","type":"input","colour":"red"}]}"#;
        assert!(LoopFile::from_json(extra_block).is_err());
    }
}
