//! Per-agent loop configuration: changes to block parameters that an agent
//! applies to its loop, without changing the loop file.
//!
//! Blocks are named by id: `<block id>` for the loop's own blocks, and
//! `<function id>/<block id>` for blocks inside a function (which then apply
//! to every use of that function). Each entry is a set of parameters that
//! replace the loop's own; `null` removes one, so the block's default
//! applies.
//!
//! Configurations can be exported as `chummy.loopconfig` files.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};
use serde_json::Value as Json;

use crate::FlowError;
use crate::format::{Block, LoopFile};

pub const CONFIG_FORMAT: &str = "chummy.loopconfig";
pub const CONFIG_VERSION: u32 = 1;

/// Changed parameters, by block name.
pub type Overrides = BTreeMap<String, serde_json::Map<String, Json>>;

/// An exported configuration.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LoopConfig {
    pub format: String,
    pub version: u32,
    /// The name of the loop it was made for (for people).
    #[serde(rename = "loop")]
    pub loop_name: String,
    pub blocks: Overrides,
}

impl LoopConfig {
    pub fn new(loop_name: &str, blocks: Overrides) -> Self {
        Self {
            format: CONFIG_FORMAT.into(),
            version: CONFIG_VERSION,
            loop_name: loop_name.into(),
            blocks,
        }
    }

    pub fn from_json(text: &str) -> Result<Self, FlowError> {
        if text.len() > 1024 * 1024 {
            return Err(FlowError::Parse("the file is too large".into()));
        }
        let raw: Json = serde_json::from_str(text).map_err(|e| FlowError::Parse(e.to_string()))?;
        match raw.get("format").and_then(Json::as_str) {
            Some(CONFIG_FORMAT) => {}
            Some(other) => return Err(FlowError::WrongFormat(other.into())),
            None => {
                return Err(FlowError::Parse(format!(
                    "missing \"format\": \"{CONFIG_FORMAT}\""
                )));
            }
        }
        let version = raw.get("version").and_then(Json::as_u64).unwrap_or(0);
        if version > u64::from(CONFIG_VERSION) {
            return Err(FlowError::UnsupportedVersion(
                u32::try_from(version).unwrap_or(u32::MAX),
            ));
        }
        if version == 0 {
            return Err(FlowError::Parse("missing or invalid \"version\"".into()));
        }
        serde_json::from_value(raw).map_err(|e| FlowError::Parse(e.to_string()))
    }

    pub fn to_json(&self) -> String {
        serde_json::to_string_pretty(self).unwrap_or_default() + "\n"
    }
}

fn find_mut<'a>(l: &'a mut LoopFile, key: &str) -> Option<&'a mut Block> {
    match key.split_once('/') {
        None => l.blocks.iter_mut().find(|b| b.id == key),
        Some((f, b)) => l
            .functions
            .iter_mut()
            .find(|x| x.id == f)?
            .blocks
            .iter_mut()
            .find(|x| x.id == b),
    }
}

/// The block an override names, if the loop has it.
pub fn find_block<'a>(l: &'a LoopFile, key: &str) -> Option<&'a Block> {
    match key.split_once('/') {
        None => l.block(key),
        Some((f, b)) => l.function(f)?.blocks.iter().find(|x| x.id == b),
    }
}

impl LoopFile {
    /// This loop with `overrides` applied. Also returns the names of
    /// overrides for blocks the loop doesn't have (e.g. after the loop was
    /// edited), which are skipped.
    pub fn with_overrides(&self, overrides: &Overrides) -> (LoopFile, Vec<String>) {
        let mut l = self.clone();
        let mut missing = Vec::new();
        for (key, params) in overrides {
            let Some(block) = find_mut(&mut l, key) else {
                missing.push(key.clone());
                continue;
            };
            if !block.params.is_object() {
                block.params = Json::Object(Default::default());
            }
            let obj = block.params.as_object_mut().expect("an object");
            for (k, v) in params {
                if v.is_null() {
                    obj.remove(k);
                } else {
                    obj.insert(k.clone(), v.clone());
                }
            }
        }
        (l, missing)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::format::{FunctionDef, Position};
    use serde_json::json;

    fn block(id: &str, params: Json) -> Block {
        Block {
            id: id.into(),
            kind: "llm".into(),
            label: id.into(),
            position: Position::default(),
            params,
            comment: String::new(),
        }
    }

    #[test]
    fn overrides_apply_to_blocks_and_function_blocks() {
        let mut l = LoopFile::new("t");
        l.blocks
            .push(block("m", json!({ "prompt": "a", "temperature": 0.7 })));
        l.functions.push(FunctionDef {
            id: "f".into(),
            name: "F".into(),
            description: String::new(),
            inputs: vec![],
            outputs: vec![],
            blocks: vec![block("inner", json!({}))],
            links: vec![],
            notes: vec![],
        });
        let mut o = Overrides::new();
        o.insert(
            "m".into(),
            json!({ "prompt": "b", "temperature": null })
                .as_object()
                .unwrap()
                .clone(),
        );
        o.insert(
            "f/inner".into(),
            json!({ "max_tokens": 50 }).as_object().unwrap().clone(),
        );
        o.insert("gone".into(), Default::default());
        let (out, missing) = l.with_overrides(&o);
        assert_eq!(out.blocks[0].params, json!({ "prompt": "b" }));
        assert_eq!(
            out.functions[0].blocks[0].params,
            json!({ "max_tokens": 50 })
        );
        assert_eq!(missing, ["gone"]);
        assert!(find_block(&l, "f/inner").is_some() && find_block(&l, "f/x").is_none());
    }

    #[test]
    fn config_files_are_strict() {
        let mut o = Overrides::new();
        o.insert(
            "m".into(),
            json!({ "temperature": 0.2 }).as_object().unwrap().clone(),
        );
        let c = LoopConfig::new("Chummy assistant", o);
        assert_eq!(LoopConfig::from_json(&c.to_json()).unwrap(), c);
        assert!(
            LoopConfig::from_json(&c.to_json().replace("\"version\": 1", "\"version\": 2"))
                .is_err()
        );
        assert!(
            LoopConfig::from_json(&c.to_json().replace("chummy.loopconfig", "chummy.loop"))
                .is_err()
        );
        assert!(LoopConfig::from_json(&c.to_json().replace("\"blocks\"", "\"blcks\"")).is_err());
    }
}
