//! Checking a loop and working out each block's ports.

use std::collections::{BTreeMap, HashMap, HashSet, VecDeque};

use serde::Serialize;

use crate::blocks::{BlockKind, PortSpec};
use crate::format::{FORMAT, LoopFile, VERSION};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Severity {
    /// The loop can't run.
    Error,
    /// The loop runs, but probably not as intended.
    Warning,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Issue {
    pub severity: Severity,
    /// The block the issue is about, if any.
    pub block: Option<String>,
    pub message: String,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize)]
pub struct BlockPorts {
    pub inputs: Vec<PortSpec>,
    pub outputs: Vec<PortSpec>,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize)]
pub struct Analysis {
    /// Ports of every block that parsed, by block id.
    pub ports: BTreeMap<String, BlockPorts>,
    pub issues: Vec<Issue>,
    /// True when there are no errors.
    pub ok: bool,
}

fn valid_id(id: &str) -> bool {
    !id.is_empty()
        && id.len() <= 64
        && id
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
}

pub fn analyze(l: &LoopFile) -> Analysis {
    let mut issues = Vec::new();
    let mut err = |block: Option<&str>, msg: String| {
        issues.push(Issue {
            severity: Severity::Error,
            block: block.map(str::to_string),
            message: msg,
        })
    };

    if l.format != FORMAT || l.version != VERSION {
        err(
            None,
            format!(
                "This is format {} version {}; expected {FORMAT} version {VERSION}.",
                l.format, l.version
            ),
        );
    }
    if !(1..=10_000).contains(&l.limits.max_steps) {
        err(None, "Max steps must be between 1 and 10,000.".into());
    }
    if !(1..=1_000).contains(&l.limits.max_llm_calls) {
        err(None, "Max model calls must be between 1 and 1,000.".into());
    }

    // Blocks.
    let mut kinds: HashMap<&str, BlockKind> = HashMap::new();
    let mut seen = HashSet::new();
    for b in &l.blocks {
        let name = if b.label.is_empty() {
            b.id.as_str()
        } else {
            b.label.as_str()
        };
        if !valid_id(&b.id) {
            err(
                Some(&b.id),
                format!("Block id “{}” must be 1–64 letters, digits, - or _.", b.id),
            );
            continue;
        }
        if !seen.insert(b.id.as_str()) {
            err(Some(&b.id), format!("Two blocks share the id “{}”.", b.id));
            continue;
        }
        match BlockKind::parse(&b.kind, &b.params) {
            Ok(k) => {
                for problem in param_problems(&k) {
                    err(Some(&b.id), format!("{name}: {problem}"));
                }
                kinds.insert(b.id.as_str(), k);
            }
            Err(e) => err(Some(&b.id), format!("{name}: {e}")),
        }
    }

    let ports: BTreeMap<String, BlockPorts> = kinds
        .iter()
        .map(|(id, k)| {
            (
                id.to_string(),
                BlockPorts {
                    inputs: k.inputs(),
                    outputs: k.outputs(),
                },
            )
        })
        .collect();

    // Links.
    let mut link_set = HashSet::new();
    let mut connected_inputs: HashSet<(&str, &str)> = HashSet::new();
    let mut edges: HashMap<&str, Vec<&str>> = HashMap::new();
    for link in &l.links {
        let (f, t) = (&link.from, &link.to);
        let Some(fp) = ports.get(&f.block) else {
            err(
                None,
                format!("A link starts at a missing block “{}”.", f.block),
            );
            continue;
        };
        let Some(tp) = ports.get(&t.block) else {
            err(
                None,
                format!("A link ends at a missing block “{}”.", t.block),
            );
            continue;
        };
        let Some(out) = fp.outputs.iter().find(|p| p.name == f.port) else {
            err(
                Some(&f.block),
                format!("“{}” has no output “{}”.", label(l, &f.block), f.port),
            );
            continue;
        };
        let Some(inp) = tp.inputs.iter().find(|p| p.name == t.port) else {
            err(
                Some(&t.block),
                format!("“{}” has no input “{}”.", label(l, &t.block), t.port),
            );
            continue;
        };
        if !out.kind.fits(inp.kind) {
            err(
                Some(&t.block),
                format!(
                    "“{}” sends {} to “{}”, which expects {}.",
                    label(l, &f.block),
                    kind_name(out.kind),
                    label(l, &t.block),
                    kind_name(inp.kind)
                ),
            );
        }
        if !link_set.insert(link) {
            err(Some(&t.block), "The same link appears twice.".into());
        }
        connected_inputs.insert((t.block.as_str(), t.port.as_str()));
        edges
            .entry(f.block.as_str())
            .or_default()
            .push(t.block.as_str());
    }

    // Whole-loop checks.
    let is_start = |k: &BlockKind| {
        matches!(
            k,
            BlockKind::Input(_) | BlockKind::History(_) | BlockKind::Fixed(_)
        )
    };
    let starts: Vec<&str> = kinds
        .iter()
        .filter(|(_, k)| is_start(k))
        .map(|(id, _)| *id)
        .collect();
    if !l.blocks.is_empty() && starts.is_empty() {
        err(
            None,
            "Nothing starts this loop. Add a Message, Conversation or Fixed text block.".into(),
        );
    }
    let has_output = kinds.values().any(|k| matches!(k, BlockKind::Output(_)));

    let mut warn = |block: Option<&str>, msg: String| {
        issues.push(Issue {
            severity: Severity::Warning,
            block: block.map(str::to_string),
            message: msg,
        })
    };
    if !l.blocks.is_empty() && !has_output {
        warn(
            None,
            "There's no Reply block, so the agent won't answer in chat.".into(),
        );
    }
    for b in &l.blocks {
        let Some(p) = ports.get(&b.id) else { continue };
        for input in &p.inputs {
            if !connected_inputs.contains(&(b.id.as_str(), input.name.as_str())) {
                warn(
                    Some(&b.id),
                    format!(
                        "“{}”: {} has nothing connected, so this block will never run.",
                        label(l, &b.id),
                        input.label
                    ),
                );
            }
        }
    }
    // Reachability from start blocks.
    let mut reached: HashSet<&str> = starts.iter().copied().collect();
    let mut queue: VecDeque<&str> = starts.iter().copied().collect();
    while let Some(b) = queue.pop_front() {
        for &n in edges.get(b).into_iter().flatten() {
            if reached.insert(n) {
                queue.push_back(n);
            }
        }
    }
    for b in &l.blocks {
        if kinds.contains_key(b.id.as_str())
            && !reached.contains(b.id.as_str())
            && !starts.is_empty()
        {
            warn(
                Some(&b.id),
                format!(
                    "“{}” can't be reached from any starting block.",
                    label(l, &b.id)
                ),
            );
        }
    }

    let ok = !issues.iter().any(|i| i.severity == Severity::Error);
    Analysis { ports, issues, ok }
}

fn label<'a>(l: &'a LoopFile, id: &'a str) -> &'a str {
    l.block(id)
        .map(|b| {
            if b.label.is_empty() {
                id
            } else {
                b.label.as_str()
            }
        })
        .unwrap_or(id)
}

fn kind_name(k: crate::blocks::ValueKind) -> &'static str {
    match k {
        crate::blocks::ValueKind::Text => "a text",
        crate::blocks::ValueKind::List => "a list",
        crate::blocks::ValueKind::Any => "a value",
    }
}

/// Problems with a block's parameters beyond what parsing catches.
fn param_problems(k: &BlockKind) -> Vec<String> {
    let mut v = Vec::new();
    let regex_check = |pattern: &str, v: &mut Vec<String>| match regex::Regex::new(pattern) {
        _ if pattern.is_empty() => v.push("the regular expression is empty".into()),
        Err(e) => v.push(format!(
            "the regular expression is invalid: {}",
            e.to_string().lines().last().unwrap_or_default()
        )),
        Ok(_) => {}
    };
    match k {
        BlockKind::Combine(p) if !(1..=10).contains(&p.inputs) => {
            v.push("number of inputs must be 1–10".into())
        }
        BlockKind::Separate(p) if !(1..=10).contains(&p.outputs) => {
            v.push("number of outputs must be 1–10".into())
        }
        BlockKind::Split(p) if p.chunk_size == 0 => v.push("chunk size must be more than 0".into()),
        BlockKind::Llm(p) => {
            if p.temperature.is_some_and(|t| !(0.0..=2.0).contains(&t)) {
                v.push("temperature must be between 0 and 2".into());
            }
            if p.max_tokens == 0 {
                v.push("max answer length must be at least 1".into());
            }
        }
        BlockKind::Regex(p) | BlockKind::Branch(p) => regex_check(&p.pattern, &mut v),
        BlockKind::Capture(p) => {
            regex_check(&p.pattern, &mut v);
            if regex::Regex::new(&p.pattern).is_ok_and(|r| r.captures_len() < 2)
                && !p.pattern.is_empty()
            {
                v.push("the regular expression needs at least one ( ) group".into());
            }
        }
        _ => {}
    }
    v
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::format::{Block, Link, PortRef, Position};
    use serde_json::json;

    pub(crate) fn block(id: &str, kind: &str, params: serde_json::Value) -> Block {
        Block {
            id: id.into(),
            kind: kind.into(),
            label: String::new(),
            position: Position::default(),
            params,
        }
    }

    pub(crate) fn link(a: &str, ap: &str, b: &str, bp: &str) -> Link {
        Link {
            from: PortRef {
                block: a.into(),
                port: ap.into(),
            },
            to: PortRef {
                block: b.into(),
                port: bp.into(),
            },
        }
    }

    #[test]
    fn simple_loop_is_ok() {
        let mut l = LoopFile::new("t");
        l.blocks = vec![
            block("in", "input", json!({})),
            block("llm", "llm", json!({})),
            block("out", "output", json!({})),
        ];
        l.links = vec![
            link("in", "out", "llm", "in"),
            link("llm", "out", "out", "in"),
        ];
        let a = analyze(&l);
        assert!(a.ok, "{:?}", a.issues);
        assert!(a.issues.is_empty(), "{:?}", a.issues);
        assert_eq!(a.ports["llm"].inputs[0].name, "in");
    }

    #[test]
    fn catches_errors() {
        let mut l = LoopFile::new("t");
        l.blocks = vec![
            block("in", "input", json!({})),
            block("in", "output", json!({})),
            block("split", "split", json!({})),
            block("bad id!", "output", json!({})),
            block("re", "regex", json!({ "pattern": "(" })),
            block("o", "output", json!({})),
        ];
        l.links = vec![
            link("in", "out", "nope", "in"),
            link("in", "out", "split", "missing"),
            link("split", "out", "o", "in"), // list into text
        ];
        let a = analyze(&l);
        assert!(!a.ok);
        let msgs: Vec<&str> = a.issues.iter().map(|i| i.message.as_str()).collect();
        assert!(msgs.iter().any(|m| m.contains("share the id")));
        assert!(msgs.iter().any(|m| m.contains("missing block")));
        assert!(msgs.iter().any(|m| m.contains("no input “missing”")));
        assert!(
            msgs.iter()
                .any(|m| m.contains("sends a list") && m.contains("expects a text")),
            "{msgs:?}"
        );
        assert!(msgs.iter().any(|m| m.contains("invalid")));
        assert!(msgs.iter().any(|m| m.contains("letters, digits")));
    }

    #[test]
    fn warns_about_dead_ends() {
        let mut l = LoopFile::new("t");
        l.blocks = vec![
            block("in", "input", json!({})),
            block("c", "combine", json!({ "inputs": 2 })),
        ];
        l.links = vec![link("in", "out", "c", "in1")];
        let a = analyze(&l);
        assert!(a.ok);
        assert!(a.issues.iter().any(|i| i.message.contains("no Reply")));
        assert!(
            a.issues
                .iter()
                .any(|i| i.message.contains("In 2 has nothing connected"))
        );
    }

    #[test]
    fn needs_a_start() {
        let mut l = LoopFile::new("t");
        l.blocks = vec![block("o", "output", json!({}))];
        assert!(!analyze(&l).ok);
    }
}
