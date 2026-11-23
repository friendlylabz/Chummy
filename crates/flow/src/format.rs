//! The `chummy.loop` file format, version 2 (version 1 files still load).
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
/// Version 2 added block comments, link labels and notes. Version 1 files
/// read as version 2 with none of them.
pub const VERSION: u32 = 3;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LoopFile {
    /// Always `"chummy.loop"`.
    pub format: String,
    /// Format version; this build reads versions 1 to 3.
    pub version: u32,
    pub name: String,
    #[serde(default)]
    pub description: String,
    #[serde(default)]
    pub limits: RunLimits,
    pub blocks: Vec<Block>,
    #[serde(default)]
    pub links: Vec<Link>,
    /// Block functions: groups of blocks used as one block (type
    /// `function`) anywhere in this loop.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub functions: Vec<FunctionDef>,
    /// Notes on the canvas, for people. They don't run.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub notes: Vec<Note>,
}

/// A note on the editor's canvas.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Note {
    pub id: String,
    #[serde(default)]
    pub text: String,
    #[serde(default)]
    pub position: Position,
    /// Size on the canvas; 0 means the default.
    #[serde(default)]
    pub width: f64,
    #[serde(default)]
    pub height: f64,
}

/// A block made of other blocks.
///
/// Its inputs and outputs are ports of its own blocks that are left open:
/// a value arriving on a function input goes to the inner port it names,
/// and every value an inner block sends out of a port named by a function
/// output leaves the function there. A function block waits for all its
/// inputs, then runs its blocks (fresh each time, like a new run) until
/// none can run, then sends what reached its outputs, in order.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FunctionDef {
    /// Unique among the loop's functions. Letters, digits, `-` and `_`.
    pub id: String,
    pub name: String,
    #[serde(default)]
    pub description: String,
    #[serde(default)]
    pub inputs: Vec<FunctionInput>,
    #[serde(default)]
    pub outputs: Vec<FunctionOutput>,
    pub blocks: Vec<Block>,
    #[serde(default)]
    pub links: Vec<Link>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub notes: Vec<Note>,
}

/// A function input: values arriving here go to `to`, an inner input port.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FunctionInput {
    /// The port's name on the function block.
    pub name: String,
    #[serde(default)]
    pub label: String,
    pub to: PortRef,
}

/// A function output: values sent out of `from`, an inner output port,
/// leave the function here.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FunctionOutput {
    pub name: String,
    #[serde(default)]
    pub label: String,
    pub from: PortRef,
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
    /// What this block is for, in words. Shown on the block.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub comment: String,
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
    /// Shown on the link in the editor.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub label: String,
}

impl Link {
    pub fn new(from: (&str, &str), to: (&str, &str)) -> Self {
        Self {
            from: PortRef {
                block: from.0.into(),
                port: from.1.into(),
            },
            to: PortRef {
                block: to.0.into(),
                port: to.1.into(),
            },
            label: String::new(),
        }
    }
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
            functions: vec![],
            notes: vec![],
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
        let version = match raw.get("version").and_then(|v| v.as_u64()) {
            Some(v) => u32::try_from(v).map_err(|_| FlowError::UnsupportedVersion(u32::MAX))?,
            None => 0,
        };
        if version > VERSION {
            return Err(FlowError::UnsupportedVersion(version));
        }
        if version == 0 {
            return Err(FlowError::Parse("missing or invalid \"version\"".into()));
        }
        let mut raw = raw;
        if version < 3 {
            upgrade_tools(&mut raw);
        }
        serde_json::from_value(raw).map_err(|e| FlowError::Parse(e.to_string()))
    }

    pub fn to_json(&self) -> String {
        serde_json::to_string_pretty(self).unwrap_or_default() + "\n"
    }

    pub fn block(&self, id: &str) -> Option<&Block> {
        self.blocks.iter().find(|b| b.id == id)
    }

    pub fn function(&self, id: &str) -> Option<&FunctionDef> {
        self.functions.iter().find(|f| f.id == id)
    }
}

/// Before version 3, a model block (or Find tool call) without a `tools`
/// list had no tools, and without `network` stayed offline. Since version 3
/// leaving them out means "inherit", so older files say "off" explicitly.
fn upgrade_tools(raw: &mut serde_json::Value) {
    fn blocks(list: Option<&mut serde_json::Value>) {
        for b in list.and_then(|v| v.as_array_mut()).into_iter().flatten() {
            let kind = b.get("type").and_then(|t| t.as_str()).unwrap_or("");
            if kind != "llm" && kind != "find_call" {
                continue;
            }
            if b.get("params").is_none_or(|p| p.is_null()) {
                b["params"] = serde_json::json!({});
            }
            if let Some(p) = b.get_mut("params").and_then(|p| p.as_object_mut()) {
                p.entry("tools").or_insert_with(|| "off".into());
                p.entry("network").or_insert_with(|| "off".into());
            }
        }
    }
    blocks(raw.get_mut("blocks"));
    for f in raw
        .get_mut("functions")
        .and_then(|v| v.as_array_mut())
        .into_iter()
        .flatten()
    {
        blocks(f.get_mut("blocks"));
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
            comment: "Where it starts".into(),
        });
        l.notes.push(Note {
            id: "n1".into(),
            text: "Hello".into(),
            position: Position::default(),
            width: 0.0,
            height: 0.0,
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
        // A huge version must not wrap around to 1.
        assert!(
            LoopFile::from_json(
                r#"{"format":"chummy.loop","version":4294967297,"name":"x","blocks":[]}"#
            )
            .is_err()
        );
        assert!(
            LoopFile::from_json(r#"{"format":"chummy.loop","version":1.5,"name":"x","blocks":[]}"#)
                .is_err()
        );
        // Version 1 files still load.
        assert!(
            LoopFile::from_json(r#"{"format":"chummy.loop","version":1,"name":"x","blocks":[]}"#)
                .is_ok()
        );
        // Unknown fields are errors, not silently dropped.
        let extra = r#"{"format":"chummy.loop","version":1,"name":"x","blocks":[],"nodes":[]}"#;
        assert!(LoopFile::from_json(extra).is_err());
        let extra_block = r#"{"format":"chummy.loop","version":1,"name":"x",
            "blocks":[{"id":"a","type":"input","colour":"red"}]}"#;
        assert!(LoopFile::from_json(extra_block).is_err());
    }
}
