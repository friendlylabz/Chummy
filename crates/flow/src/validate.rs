//! Checking a loop and working out each block's ports.

use std::collections::{BTreeMap, HashMap, HashSet, VecDeque};

use serde::Serialize;

use crate::blocks::{BlockKind, PortSpec};
use crate::format::{Block, FORMAT, FunctionDef, Link, LoopFile, VERSION};

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
    /// The function the block is in; `None` for the loop itself.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub function: Option<String>,
    pub message: String,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize)]
pub struct BlockPorts {
    pub inputs: Vec<PortSpec>,
    pub outputs: Vec<PortSpec>,
}

/// A function's ports (as a block) and its own blocks' ports.
#[derive(Debug, Clone, Default, PartialEq, Serialize)]
pub struct FunctionPorts {
    pub ports: BlockPorts,
    pub blocks: BTreeMap<String, BlockPorts>,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize)]
pub struct Analysis {
    /// Ports of every block that parsed, by block id.
    pub ports: BTreeMap<String, BlockPorts>,
    /// Every function that could be read, by id.
    pub functions: BTreeMap<String, FunctionPorts>,
    pub issues: Vec<Issue>,
    /// True when there are no errors.
    pub ok: bool,
}

/// Most blocks in one loop, counting those inside functions.
pub const MAX_BLOCKS: usize = 5_000;
/// Most links in one loop, counting those inside functions.
pub const MAX_LINKS: usize = 20_000;
pub const MAX_FUNCTIONS: usize = 500;

pub(crate) fn valid_id(id: &str) -> bool {
    !id.is_empty()
        && id.len() <= 64
        && id
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
}

/// Ports of a block, looking up function signatures.
pub(crate) fn block_ports(
    k: &BlockKind,
    signatures: &HashMap<String, BlockPorts>,
) -> Option<BlockPorts> {
    match k {
        BlockKind::Function(p) => signatures.get(&p.function).cloned(),
        _ => Some(BlockPorts {
            inputs: k.inputs(),
            outputs: k.outputs(),
        }),
    }
}

struct Sink<'a> {
    issues: &'a mut Vec<Issue>,
    function: Option<String>,
}

impl Sink<'_> {
    fn push(&mut self, severity: Severity, block: Option<&str>, message: String) {
        self.issues.push(Issue {
            severity,
            block: block.map(str::to_string),
            function: self.function.clone(),
            message,
        });
    }
    fn err(&mut self, block: Option<&str>, message: String) {
        self.push(Severity::Error, block, message)
    }
    fn warn(&mut self, block: Option<&str>, message: String) {
        self.push(Severity::Warning, block, message)
    }
}

pub fn analyze(l: &LoopFile) -> Analysis {
    let mut issues = Vec::new();
    {
        let mut top = Sink {
            issues: &mut issues,
            function: None,
        };
        if l.format != FORMAT || !(1..=VERSION).contains(&l.version) {
            top.err(
                None,
                format!(
                    "This is format {} version {}; expected {FORMAT} version {VERSION}.",
                    l.format, l.version
                ),
            );
        }
        if !(1..=10_000).contains(&l.limits.max_steps) {
            top.err(None, "Max steps must be between 1 and 10,000.".into());
        }
        if !(1..=1_000).contains(&l.limits.max_llm_calls) {
            top.err(None, "Max model calls must be between 1 and 1,000.".into());
        }
        if !(1..=1_000).contains(&l.limits.max_tool_calls) {
            top.err(None, "Max tool calls must be between 1 and 1,000.".into());
        }
        // Sizes a person could build, with room to spare; anything bigger is
        // almost certainly a broken or hostile file.
        let blocks = l.blocks.len() + l.functions.iter().map(|f| f.blocks.len()).sum::<usize>();
        let links = l.links.len() + l.functions.iter().map(|f| f.links.len()).sum::<usize>();
        if blocks > MAX_BLOCKS || links > MAX_LINKS || l.functions.len() > MAX_FUNCTIONS {
            top.err(
                None,
                format!(
                    "This loop is too large ({blocks} blocks, {links} links, {} functions; the limits are {MAX_BLOCKS}, {MAX_LINKS} and {MAX_FUNCTIONS}).",
                    l.functions.len()
                ),
            );
        }
    }

    // Functions: ids, then the order to work out their ports in (a function
    // can use another, but never itself, directly or not).
    let mut fn_ids = HashSet::new();
    for f in &l.functions {
        let mut sink = Sink {
            issues: &mut issues,
            function: Some(f.id.clone()),
        };
        if !valid_id(&f.id) {
            sink.err(
                None,
                format!(
                    "Function id “{}” must be 1–64 letters, digits, - or _.",
                    f.id
                ),
            );
        } else if !fn_ids.insert(f.id.as_str()) {
            sink.err(None, format!("Two functions share the id “{}”.", f.id));
        }
    }
    let uses = |f: &FunctionDef| -> Vec<String> {
        f.blocks
            .iter()
            .filter(|b| b.kind == "function")
            .filter_map(|b| match BlockKind::parse(&b.kind, &b.params) {
                Ok(BlockKind::Function(p)) => Some(p.function),
                _ => None,
            })
            .collect()
    };
    let mut order: Vec<&FunctionDef> = Vec::new();
    let mut state: HashMap<&str, u8> = HashMap::new(); // 1 = visiting, 2 = done
    let mut cyclic: HashSet<String> = HashSet::new();
    fn visit<'a>(
        f: &'a FunctionDef,
        l: &'a LoopFile,
        uses: &dyn Fn(&FunctionDef) -> Vec<String>,
        state: &mut HashMap<&'a str, u8>,
        order: &mut Vec<&'a FunctionDef>,
        cyclic: &mut HashSet<String>,
    ) {
        match state.get(f.id.as_str()) {
            Some(2) => return,
            Some(1) => {
                cyclic.insert(f.id.clone());
                return;
            }
            _ => {}
        }
        state.insert(&f.id, 1);
        for u in uses(f) {
            if let Some(g) = l.function(&u) {
                visit(g, l, uses, state, order, cyclic);
                if cyclic.contains(&g.id) {
                    cyclic.insert(f.id.clone());
                }
            }
        }
        state.insert(&f.id, 2);
        order.push(f);
    }
    for f in &l.functions {
        visit(f, l, &uses, &mut state, &mut order, &mut cyclic);
    }

    let mut signatures: HashMap<String, BlockPorts> = HashMap::new();
    let mut functions = BTreeMap::new();
    for f in order {
        let mut sink = Sink {
            issues: &mut issues,
            function: Some(f.id.clone()),
        };
        if cyclic.contains(&f.id) {
            sink.err(
                None,
                format!(
                    "The function “{}” uses itself, directly or through another function.",
                    f.name
                ),
            );
            continue;
        }
        let inner = graph(&f.blocks, &f.links, Some(f), &signatures, &mut sink);
        let sig = signature(f, &inner, &mut sink);
        signatures.insert(f.id.clone(), sig.clone());
        functions.insert(
            f.id.clone(),
            FunctionPorts {
                ports: sig,
                blocks: inner,
            },
        );
    }

    let ports = {
        let mut sink = Sink {
            issues: &mut issues,
            function: None,
        };
        graph(&l.blocks, &l.links, None, &signatures, &mut sink)
    };

    let ok = !issues.iter().any(|i| i.severity == Severity::Error);
    Analysis {
        ports,
        functions,
        issues,
        ok,
    }
}

/// A function's ports as a block, from its inputs and outputs.
fn signature(f: &FunctionDef, inner: &BTreeMap<String, BlockPorts>, sink: &mut Sink) -> BlockPorts {
    let mut names = HashSet::new();
    let mut sig = BlockPorts::default();
    let block_label = |id: &str| {
        f.blocks
            .iter()
            .find(|b| b.id == id)
            .map(|b| {
                if b.label.is_empty() {
                    id.to_string()
                } else {
                    b.label.clone()
                }
            })
            .unwrap_or_else(|| id.to_string())
    };
    for i in &f.inputs {
        if !valid_id(&i.name) || !names.insert(format!("in:{}", i.name)) {
            sink.err(
                None,
                format!(
                    "“{}” has a bad or repeated input name “{}”.",
                    f.name, i.name
                ),
            );
            continue;
        }
        match inner
            .get(&i.to.block)
            .and_then(|p| p.inputs.iter().find(|x| x.name == i.to.port))
        {
            Some(p) => sig.inputs.push(PortSpec {
                name: i.name.clone(),
                label: if i.label.is_empty() {
                    format!("{} · {}", block_label(&i.to.block), p.label)
                } else {
                    i.label.clone()
                },
                kind: p.kind,
            }),
            None => sink.err(
                Some(&i.to.block),
                format!(
                    "“{}”'s input “{}” goes to a missing port {}.{}.",
                    f.name, i.name, i.to.block, i.to.port
                ),
            ),
        }
    }
    for o in &f.outputs {
        if !valid_id(&o.name) || !names.insert(format!("out:{}", o.name)) {
            sink.err(
                None,
                format!(
                    "“{}” has a bad or repeated output name “{}”.",
                    f.name, o.name
                ),
            );
            continue;
        }
        match inner
            .get(&o.from.block)
            .and_then(|p| p.outputs.iter().find(|x| x.name == o.from.port))
        {
            Some(p) => sig.outputs.push(PortSpec {
                name: o.name.clone(),
                label: if o.label.is_empty() {
                    format!("{} · {}", block_label(&o.from.block), p.label)
                } else {
                    o.label.clone()
                },
                kind: p.kind,
            }),
            None => sink.err(
                Some(&o.from.block),
                format!(
                    "“{}”'s output “{}” comes from a missing port {}.{}.",
                    f.name, o.name, o.from.block, o.from.port
                ),
            ),
        }
    }
    sig
}

/// Checks one graph (the loop, or a function's blocks) and returns its ports.
fn graph(
    blocks: &[Block],
    links: &[Link],
    func: Option<&FunctionDef>,
    signatures: &HashMap<String, BlockPorts>,
    sink: &mut Sink,
) -> BTreeMap<String, BlockPorts> {
    let label = |id: &str| -> String {
        blocks
            .iter()
            .find(|b| b.id == id)
            .map(|b| {
                if b.label.is_empty() {
                    id.to_string()
                } else {
                    b.label.clone()
                }
            })
            .unwrap_or_else(|| id.to_string())
    };
    let mut kinds: HashMap<&str, BlockKind> = HashMap::new();
    let mut ports: BTreeMap<String, BlockPorts> = BTreeMap::new();
    let mut seen = HashSet::new();
    for b in blocks {
        let name = label(&b.id);
        if !valid_id(&b.id) {
            sink.err(
                Some(&b.id),
                format!("Block id “{}” must be 1–64 letters, digits, - or _.", b.id),
            );
            continue;
        }
        if !seen.insert(b.id.as_str()) {
            sink.err(Some(&b.id), format!("Two blocks share the id “{}”.", b.id));
            continue;
        }
        match BlockKind::parse(&b.kind, &b.params) {
            Ok(k) => {
                for problem in param_problems(&k) {
                    sink.err(Some(&b.id), format!("{name}: {problem}"));
                }
                match block_ports(&k, signatures) {
                    Some(p) => {
                        ports.insert(b.id.clone(), p);
                    }
                    None => {
                        if let BlockKind::Function(p) = &k {
                            sink.err(
                                Some(&b.id),
                                format!(
                                    "{name}: the function “{}” doesn't exist or has problems.",
                                    p.function
                                ),
                            );
                        }
                    }
                }
                kinds.insert(b.id.as_str(), k);
            }
            Err(e) => sink.err(Some(&b.id), format!("{name}: {e}")),
        }
    }

    let mut link_set = HashSet::new();
    let mut connected_inputs: HashSet<(&str, &str)> = HashSet::new();
    let mut edges: HashMap<&str, Vec<&str>> = HashMap::new();
    for link in links {
        let (f, t) = (&link.from, &link.to);
        let Some(fp) = ports.get(&f.block) else {
            sink.err(
                None,
                format!("A link starts at a missing block “{}”.", f.block),
            );
            continue;
        };
        let Some(tp) = ports.get(&t.block) else {
            sink.err(
                None,
                format!("A link ends at a missing block “{}”.", t.block),
            );
            continue;
        };
        let Some(out) = fp.outputs.iter().find(|p| p.name == f.port) else {
            sink.err(
                Some(&f.block),
                format!("“{}” has no output “{}”.", label(&f.block), f.port),
            );
            continue;
        };
        let Some(inp) = tp.inputs.iter().find(|p| p.name == t.port) else {
            sink.err(
                Some(&t.block),
                format!("“{}” has no input “{}”.", label(&t.block), t.port),
            );
            continue;
        };
        if !out.kind.fits(inp.kind) {
            sink.err(
                Some(&t.block),
                format!(
                    "“{}” sends {} to “{}”, which expects {}.",
                    label(&f.block),
                    kind_name(out.kind),
                    label(&t.block),
                    kind_name(inp.kind)
                ),
            );
        }
        if !link_set.insert(link) {
            sink.err(Some(&t.block), "The same link appears twice.".into());
        }
        connected_inputs.insert((t.block.as_str(), t.port.as_str()));
        edges
            .entry(f.block.as_str())
            .or_default()
            .push(t.block.as_str());
    }

    // Function inputs count as connected, and as starting points.
    let exposed: HashSet<(&str, &str)> = func
        .map(|f| {
            f.inputs
                .iter()
                .map(|i| (i.to.block.as_str(), i.to.port.as_str()))
                .collect()
        })
        .unwrap_or_default();

    let mut starts: Vec<&str> = ports
        .iter()
        .filter(|(_, p)| p.inputs.is_empty())
        .map(|(id, _)| id.as_str())
        .collect();
    starts.extend(
        exposed
            .iter()
            .map(|(b, _)| *b)
            .filter(|b| ports.contains_key(*b)),
    );
    starts.sort();
    starts.dedup();

    if !blocks.is_empty() && starts.is_empty() {
        sink.err(
            None,
            match func {
                None => {
                    "Nothing starts this loop. Add a Message, Conversation or Fixed text block."
                        .into()
                }
                Some(f) => format!(
                    "Nothing in “{}” can start: give it an input or a Fixed text block.",
                    f.name
                ),
            },
        );
    }
    if func.is_none()
        && !blocks.is_empty()
        && !kinds.values().any(|k| matches!(k, BlockKind::Output(_)))
        && !blocks.iter().any(|b| b.kind == "function")
    {
        sink.warn(
            None,
            "There's no Reply block, so the agent won't answer in chat.".into(),
        );
    }
    for b in blocks {
        let Some(p) = ports.get(&b.id) else { continue };
        for input in &p.inputs {
            let key = (b.id.as_str(), input.name.as_str());
            if !connected_inputs.contains(&key) && !exposed.contains(&key) {
                sink.warn(
                    Some(&b.id),
                    format!(
                        "“{}”: {} has nothing connected, so this block will never run.",
                        label(&b.id),
                        input.label
                    ),
                );
            }
        }
    }
    let mut reached: HashSet<&str> = starts.iter().copied().collect();
    let mut queue: VecDeque<&str> = starts.iter().copied().collect();
    while let Some(b) = queue.pop_front() {
        for &n in edges.get(b).into_iter().flatten() {
            if reached.insert(n) {
                queue.push_back(n);
            }
        }
    }
    for b in blocks {
        if ports.contains_key(b.id.as_str())
            && !reached.contains(b.id.as_str())
            && !starts.is_empty()
        {
            sink.warn(
                Some(&b.id),
                format!(
                    "“{}” can't be reached from any starting block.",
                    label(&b.id)
                ),
            );
        }
    }
    ports
}

fn kind_name(k: crate::blocks::ValueKind) -> &'static str {
    match k {
        crate::blocks::ValueKind::Text => "a text",
        crate::blocks::ValueKind::List => "a list",
        crate::blocks::ValueKind::Exchange => "an exchange",
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
            if p.max_tokens.is_some_and(|m| !(1..=32_768).contains(&m)) {
                v.push("max answer length must be 1–32,768 tokens".into());
            }
            if p.retries > 5 {
                v.push("times to ask again must be 0–5".into());
            }
            if p.tools.each.keys().any(|t| t.trim().is_empty()) {
                v.push("the tool list has a blank name".into());
            }
        }
        BlockKind::RunTool(p) if p.max_result_chars < 100 => {
            v.push("longest result must be at least 100 characters".into())
        }
        BlockKind::Store(p) if !crate::text::valid_memory_key(p.key.trim()) => {
            v.push("the memory name must be 1–40 letters, digits or _".into())
        }
        BlockKind::Recall(p) if !crate::text::valid_memory_key(p.key.trim()) => {
            v.push("the memory name must be 1–40 letters, digits or _".into())
        }
        BlockKind::Outline(p) if !crate::text::valid_memory_key(p.key.trim()) => {
            v.push("the memory name must be 1–40 letters, digits or _".into())
        }
        BlockKind::Store(p) if p.max_chars < 100 => {
            v.push("the longest memory must be at least 100 characters".into())
        }
        BlockKind::AskUser(p) => {
            if p.timeout_minutes > 10_080 {
                v.push("give up after must be at most 10,080 minutes (a week)".into());
            }
            if p.prompt.trim().is_empty() {
                v.push("write the question".into());
            }
        }
        BlockKind::Exchange(p) if p.roles.is_empty() => v.push("choose at least one role".into()),
        BlockKind::Function(p) if p.function.is_empty() => v.push("no function chosen".into()),
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
    use crate::format::{Block, Link, Position};
    use serde_json::json;

    pub(crate) fn block(id: &str, kind: &str, params: serde_json::Value) -> Block {
        Block {
            id: id.into(),
            kind: kind.into(),
            label: String::new(),
            position: Position::default(),
            params,
            comment: String::new(),
        }
    }

    pub(crate) fn link(a: &str, ap: &str, b: &str, bp: &str) -> Link {
        Link::new((a, ap), (b, bp))
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
