//! The loop engine.
//!
//! Execution model:
//!
//! - Every input port has a queue. A value arriving on any link into the port
//!   joins the back of its queue.
//! - A block is ready when **every** input port has at least one value. It then
//!   takes one value from the front of each queue and runs.
//! - A block's results go to **every** link from the output port that produced
//!   them (broadcast). Most blocks produce one value on each output; *For each*
//!   produces one per item, *Branch* uses only one of its two outputs, a model
//!   block sends either its Answer or its Tool call (plus Reasoning), and
//!   *Reply* produces none.
//! - Blocks without inputs (Message, Conversation, Fixed text) run once, at the
//!   start.
//! - Ready blocks run one at a time, first come, first served, so a run is
//!   deterministic. The run ends when nothing is ready.
//! - Cycles are allowed: a block runs again whenever its ports refill. Runs are
//!   bounded by [`RunLimits`] and can be stopped at any time.
//! - A **function** block runs its own blocks the same way, as a fresh run
//!   seeded with its inputs, and sends out what reached its outputs. Steps,
//!   model calls and tool calls inside count toward the loop's limits.

use std::collections::{HashMap, VecDeque};
use std::future::Future;
use std::pin::Pin;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use serde::{Deserialize, Serialize};
use serde_json::Value as Json;

use crate::blocks::{BadCall, BlockKind, LlmParams, OnToolError, ToolChoice};
use crate::exchange::{ExMessage, Exchange, Role, ToolCall, ToolInfo, check_arguments};
use crate::format::{Block, FunctionDef, Link, LoopFile};
use crate::text::{fill_prompt, split, strip_thinking, thinking, truncate};
use crate::validate::{Analysis, Severity, analyze};

/// A value flowing between blocks.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", content = "value", rename_all = "snake_case")]
pub enum Value {
    Text(String),
    List(Vec<String>),
    Exchange(Exchange),
}

impl Value {
    pub fn as_text(&self) -> String {
        match self {
            Value::Text(t) => t.clone(),
            Value::List(l) => l.join("\n"),
            Value::Exchange(e) => e.transcript(),
        }
    }

    pub fn as_list(&self) -> Vec<String> {
        match self {
            Value::Text(t) => vec![t.clone()],
            Value::List(l) => l.clone(),
            Value::Exchange(e) => e.messages.iter().map(|m| m.text.clone()).collect(),
        }
    }

    /// Short preview for the UI.
    pub fn preview(&self) -> String {
        const MAX: usize = 240;
        let s = match self {
            Value::Text(t) => t.clone(),
            Value::List(l) => format!(
                "[{} items] {}",
                l.len(),
                l.first().cloned().unwrap_or_default()
            ),
            Value::Exchange(e) => {
                let last = e.messages.last();
                format!(
                    "[exchange, {} messages] {}",
                    e.messages.len(),
                    last.map(|m| {
                        if m.calls.is_empty() {
                            format!("{}: {}", m.role.label(), m.text)
                        } else {
                            format!(
                                "calls {}",
                                m.calls
                                    .iter()
                                    .map(|c| c.name.as_str())
                                    .collect::<Vec<_>>()
                                    .join(", ")
                            )
                        }
                    })
                    .unwrap_or_default()
                )
            }
        };
        if s.chars().count() > MAX {
            s.chars().take(MAX).collect::<String>() + "…"
        } else {
            s
        }
    }
}

fn d_steps() -> u32 {
    200
}
fn d_llm() -> u32 {
    20
}
fn d_tools() -> u32 {
    20
}

/// Safety limits for one run. Stored in the loop file.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RunLimits {
    /// Most block runs allowed in one run of the loop.
    #[serde(default = "d_steps")]
    pub max_steps: u32,
    /// Most model calls allowed in one run of the loop.
    #[serde(default = "d_llm")]
    pub max_llm_calls: u32,
    /// Most tool calls allowed in one run of the loop.
    #[serde(default = "d_tools")]
    pub max_tool_calls: u32,
}

impl Default for RunLimits {
    fn default() -> Self {
        Self {
            max_steps: d_steps(),
            max_llm_calls: d_llm(),
            max_tool_calls: d_tools(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ChatTurn {
    /// "user" or "assistant".
    pub role: String,
    pub text: String,
}

/// What a run starts with.
#[derive(Debug, Clone, Default)]
pub struct RunInputs {
    /// The chat message; every Message block sends it.
    pub message: String,
    /// Earlier conversation, oldest first.
    pub history: Vec<ChatTurn>,
    /// Per-block text for specific Message blocks (editor test runs).
    pub overrides: HashMap<String, String>,
}

/// One model request from a model block.
#[derive(Debug, Clone, PartialEq)]
pub struct LlmCall {
    pub block: String,
    pub messages: Vec<ExMessage>,
    /// Tools offered to the model (empty: none).
    pub tools: Vec<ToolInfo>,
    pub tool_choice: ToolChoice,
    pub temperature: Option<f32>,
    pub max_tokens: u32,
    /// Model connection id; `None` means the agent's model.
    pub model: Option<String>,
}

/// A tool call the model tried to make but got wrong at the format level.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct MalformedCall {
    pub name: String,
    pub raw: String,
    pub error: String,
}

/// What the model said.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct LlmAnswer {
    pub text: String,
    /// Thinking the provider reported separately (not `<think>` tags, which
    /// the engine finds itself).
    pub reasoning: String,
    pub calls: Vec<ToolCall>,
    pub malformed: Vec<MalformedCall>,
}

/// A tool's result.
#[derive(Debug, Clone, PartialEq)]
pub struct ToolOutput {
    pub text: String,
    /// The tool reported a problem; `text` says what.
    pub is_error: bool,
}

/// What the engine needs from the outside world.
pub trait Host: Send + Sync {
    /// Asks a model. `on_text` receives the answer as it streams.
    fn llm(
        &self,
        call: LlmCall,
        on_text: &mut (dyn FnMut(&str) + Send),
    ) -> impl Future<Output = Result<LlmAnswer, String>> + Send;
    /// A tool that's installed and switched on, if there is one by that name.
    fn tool_info(&self, name: &str) -> Option<ToolInfo>;
    /// Runs a tool. `Err` means it couldn't run at all.
    fn run_tool(
        &self,
        name: &str,
        arguments: Json,
    ) -> impl Future<Output = Result<ToolOutput, String>> + Send;
    fn read_file(&self, path: &str) -> Result<String, String>;
    fn write_file(&self, path: &str, content: &str, append: bool) -> Result<(), String>;
    /// Names in a folder; folders end with `/`.
    fn list_files(&self, path: &str) -> Result<Vec<String>, String>;
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum BlockState {
    /// Some inputs have arrived; waiting for the rest.
    Waiting,
    Running,
    Done,
    Failed,
}

/// Progress reported while a run goes. Blocks inside a function are named
/// by path: `<function block id>/<inner block id>`.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Event {
    BlockState {
        block: String,
        label: String,
        state: BlockState,
    },
    BlockOutput {
        block: String,
        port: String,
        preview: String,
    },
    /// Streaming text from a model.
    LlmText { block: String, text: String },
    /// The model asked for a tool.
    ToolCall {
        block: String,
        tool: String,
        arguments: String,
    },
    ToolResult {
        block: String,
        tool: String,
        preview: String,
        is_error: bool,
    },
    /// Something worth knowing that isn't an error, e.g. a retry.
    Notice { block: String, text: String },
    /// A Reply block produced part of the answer.
    Reply { block: String, text: String },
}

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(tag = "status", rename_all = "snake_case")]
pub enum RunStatus {
    Completed,
    Stopped,
    LimitReached {
        message: String,
    },
    Failed {
        block: Option<String>,
        error: String,
    },
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct RunOutcome {
    pub status: RunStatus,
    /// Everything Reply blocks produced, in order.
    pub replies: Vec<String>,
    pub steps: u32,
    pub llm_calls: u32,
    pub tool_calls: u32,
    /// Blocks left with some, but not all, of their inputs.
    pub waiting: Vec<String>,
}

type Outputs = Vec<(String, Value)>;
type Emit<'e> = dyn FnMut(Event) + Send + 'e;

/// One graph being run: the loop, or one call of a function.
struct Frame<'a> {
    blocks: &'a [Block],
    /// Prepended to block ids in events ("" for the loop).
    prefix: String,
    kinds: HashMap<String, BlockKind>,
    in_ports: HashMap<String, Vec<String>>,
    queues: HashMap<String, Vec<VecDeque<Value>>>,
    routes: HashMap<(String, String), Vec<(String, usize)>>,
    ready: VecDeque<String>,
    /// How many times each block is in `ready`. Kept equal to the number of
    /// runs its queued inputs allow, so the queue is strictly first-come.
    scheduled: HashMap<String, usize>,
    remembered: HashMap<String, Value>,
    /// (inner block, port) → function output names.
    exposed: HashMap<(String, String), Vec<String>>,
    /// What reached the function's outputs.
    collected: Outputs,
}

impl<'a> Frame<'a> {
    fn new(blocks: &'a [Block], links: &'a [Link], prefix: String, analysis: &Analysis) -> Self {
        let mut f = Frame {
            blocks,
            prefix,
            kinds: HashMap::new(),
            in_ports: HashMap::new(),
            queues: HashMap::new(),
            routes: HashMap::new(),
            ready: VecDeque::new(),
            scheduled: HashMap::new(),
            remembered: HashMap::new(),
            exposed: HashMap::new(),
            collected: vec![],
        };
        for b in blocks {
            // Validation passed, so every block parses.
            let Ok(k) = BlockKind::parse(&b.kind, &b.params) else {
                continue;
            };
            let ins: Vec<String> = match &k {
                BlockKind::Function(p) => analysis
                    .functions
                    .get(&p.function)
                    .map(|fp| fp.ports.inputs.iter().map(|p| p.name.clone()).collect())
                    .unwrap_or_default(),
                _ => k.inputs().into_iter().map(|p| p.name).collect(),
            };
            f.queues
                .insert(b.id.clone(), vec![VecDeque::new(); ins.len()]);
            if ins.is_empty() {
                f.ready.push_back(b.id.clone());
            }
            f.in_ports.insert(b.id.clone(), ins);
            f.kinds.insert(b.id.clone(), k);
        }
        for link in links {
            let Some(ports) = f.in_ports.get(&link.to.block) else {
                continue;
            };
            let idx = ports.iter().position(|p| *p == link.to.port).unwrap_or(0);
            f.routes
                .entry((link.from.block.clone(), link.from.port.clone()))
                .or_default()
                .push((link.to.block.clone(), idx));
        }
        f
    }

    fn label(&self, id: &str) -> String {
        self.blocks
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
    }

    fn path(&self, id: &str) -> String {
        format!("{}{id}", self.prefix)
    }

    /// How many more runs the queued inputs allow.
    fn possible_runs(&self, id: &str) -> usize {
        self.queues[id].iter().map(|q| q.len()).min().unwrap_or(0)
    }

    /// Queues `id` once for every run its inputs now allow.
    fn schedule(&mut self, id: &str) -> bool {
        let possible = self.possible_runs(id);
        let s = self.scheduled.entry(id.to_string()).or_default();
        let added = *s < possible;
        while *s < possible {
            *s += 1;
            self.ready.push_back(id.to_string());
        }
        added
    }

    fn waiting(&self) -> Vec<String> {
        let mut w: Vec<String> = self
            .queues
            .iter()
            .filter(|(_, qs)| qs.iter().any(|q| !q.is_empty()) && qs.iter().any(|q| q.is_empty()))
            .map(|(id, _)| self.path(id))
            .collect();
        w.sort();
        w
    }
}

struct Runner<'a, H: Host> {
    file: &'a LoopFile,
    analysis: Analysis,
    host: &'a H,
    inputs: &'a RunInputs,
    cancel: &'a AtomicBool,
    replies: Vec<String>,
    steps: u32,
    llm_calls: u32,
    tool_calls: u32,
    /// Notices already shown, so a block in a cycle doesn't repeat them.
    noticed: std::collections::HashSet<String>,
}

/// Runs `l` to completion (or until stopped or limited).
pub async fn run<H: Host>(
    l: &LoopFile,
    host: &H,
    inputs: &RunInputs,
    cancel: &AtomicBool,
    mut on_event: impl FnMut(Event) + Send,
) -> RunOutcome {
    let analysis = analyze(l);
    if let Some(e) = analysis
        .issues
        .iter()
        .find(|i| i.severity == Severity::Error)
    {
        return RunOutcome {
            status: RunStatus::Failed {
                block: e.block.clone(),
                error: e.message.clone(),
            },
            replies: vec![],
            steps: 0,
            llm_calls: 0,
            tool_calls: 0,
            waiting: vec![],
        };
    }

    let mut r = Runner {
        file: l,
        analysis,
        host,
        inputs,
        cancel,
        replies: vec![],
        steps: 0,
        llm_calls: 0,
        tool_calls: 0,
        noticed: Default::default(),
    };
    let mut frame = Frame::new(&l.blocks, &l.links, String::new(), &r.analysis);
    let status = match r.drive(&mut frame, &mut on_event).await {
        Ok(()) => RunStatus::Completed,
        Err(Stop::Cancelled) => RunStatus::Stopped,
        Err(Stop::Limit(message)) => RunStatus::LimitReached { message },
        Err(Stop::Failed { block, error }) => RunStatus::Failed {
            block: Some(block),
            error,
        },
    };
    RunOutcome {
        status,
        replies: r.replies,
        steps: r.steps,
        llm_calls: r.llm_calls,
        tool_calls: r.tool_calls,
        waiting: frame.waiting(),
    }
}

async fn cancelled(flag: &AtomicBool) {
    while !flag.load(Ordering::Relaxed) {
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
}

enum Stop {
    Cancelled,
    Limit(String),
    /// `block` is the path of the block that failed.
    Failed {
        block: String,
        error: String,
    },
}

/// A block's own failure, before the engine adds where it happened.
enum BlockErr {
    Stop(Stop),
    Failed(String),
}

impl From<Stop> for BlockErr {
    fn from(s: Stop) -> Self {
        BlockErr::Stop(s)
    }
}

type BoxFut<'s, T> = Pin<Box<dyn Future<Output = T> + Send + 's>>;

impl<'a, H: Host> Runner<'a, H> {
    fn drive<'s>(
        &'s mut self,
        frame: &'s mut Frame<'a>,
        on_event: &'s mut Emit<'_>,
    ) -> BoxFut<'s, Result<(), Stop>> {
        Box::pin(async move {
            while let Some(id) = frame.ready.pop_front() {
                if self.cancel.load(Ordering::Relaxed) {
                    return Err(Stop::Cancelled);
                }
                if let Some(s) = frame.scheduled.get_mut(&id) {
                    *s = s.saturating_sub(1);
                }
                let args: Vec<Value> = frame
                    .queues
                    .get_mut(&id)
                    .map(|qs| qs.iter_mut().filter_map(|q| q.pop_front()).collect())
                    .unwrap_or_default();

                self.steps += 1;
                if self.steps > self.file.limits.max_steps {
                    return Err(Stop::Limit(format!(
                        "Stopped after {} steps, the loop's limit.",
                        self.file.limits.max_steps
                    )));
                }

                let path = frame.path(&id);
                let label = frame.label(&id);
                on_event(Event::BlockState {
                    block: path.clone(),
                    label: label.clone(),
                    state: BlockState::Running,
                });
                let outputs = match self.execute(frame, &id, args, on_event).await {
                    Ok(o) => o,
                    Err(BlockErr::Stop(s)) => return Err(s),
                    Err(BlockErr::Failed(e)) => {
                        on_event(Event::BlockState {
                            block: path.clone(),
                            label: label.clone(),
                            state: BlockState::Failed,
                        });
                        return Err(Stop::Failed {
                            block: path,
                            error: format!("{label}: {e}"),
                        });
                    }
                };
                on_event(Event::BlockState {
                    block: path.clone(),
                    label,
                    state: BlockState::Done,
                });

                for (port, value) in outputs {
                    on_event(Event::BlockOutput {
                        block: path.clone(),
                        port: port.clone(),
                        preview: value.preview(),
                    });
                    let key = (id.clone(), port);
                    if let Some(names) = frame.exposed.get(&key) {
                        for n in names.clone() {
                            frame.collected.push((n, value.clone()));
                        }
                    }
                    let targets = frame.routes.get(&key).cloned().unwrap_or_default();
                    for (to, idx) in targets {
                        frame.queues.get_mut(&to).unwrap()[idx].push_back(value.clone());
                        if !frame.schedule(&to) && frame.possible_runs(&to) == 0 {
                            on_event(Event::BlockState {
                                block: frame.path(&to),
                                label: frame.label(&to),
                                state: BlockState::Waiting,
                            });
                        }
                    }
                }
            }
            Ok(())
        })
    }

    async fn execute(
        &mut self,
        frame: &mut Frame<'a>,
        id: &str,
        args: Vec<Value>,
        on_event: &mut Emit<'_>,
    ) -> Result<Outputs, BlockErr> {
        let one = |v: Value| Ok(vec![("out".to_string(), v)]);
        let arg = |i: usize| args.get(i).cloned().unwrap_or(Value::Text(String::new()));
        let path = frame.path(id);
        let kind = frame.kinds[id].clone();
        match kind {
            BlockKind::Input(_) => {
                let text = self
                    .inputs
                    .overrides
                    .get(id)
                    .cloned()
                    .unwrap_or_else(|| self.inputs.message.clone());
                one(Value::Text(text))
            }
            BlockKind::History(p) => {
                let n = p.max_messages as usize;
                let h = &self.inputs.history;
                let recent = &h[h.len().saturating_sub(n)..];
                let text = if recent.is_empty() {
                    "(This is the start of the conversation.)".to_string()
                } else {
                    recent
                        .iter()
                        .map(|t| {
                            format!(
                                "{}: {}",
                                if t.role == "user" {
                                    "User"
                                } else {
                                    "Assistant"
                                },
                                t.text
                            )
                        })
                        .collect::<Vec<_>>()
                        .join("\n\n")
                };
                one(Value::Text(text))
            }
            BlockKind::Fixed(p) => one(Value::Text(p.text)),
            BlockKind::Output(_) => {
                let text = arg(0).as_text();
                self.replies.push(text.clone());
                on_event(Event::Reply { block: path, text });
                Ok(vec![])
            }
            BlockKind::Llm(p) => self.model_block(&path, &p, arg(0), on_event).await,
            BlockKind::RunTool(p) => {
                let Value::Exchange(mut ex) = arg(0) else {
                    return Err(BlockErr::Failed(
                        "it needs an exchange from a model block's Tool call output".into(),
                    ));
                };
                let calls = ex.pending_calls();
                if calls.is_empty() {
                    on_event(Event::Notice {
                        block: path.clone(),
                        text: "No tool calls to run; passing the exchange on.".into(),
                    });
                }
                for call in calls {
                    if self.tool_calls >= self.file.limits.max_tool_calls {
                        return Err(Stop::Limit(format!(
                            "Stopped after {} tool calls, the loop's limit.",
                            self.file.limits.max_tool_calls
                        ))
                        .into());
                    }
                    self.tool_calls += 1;
                    on_event(Event::ToolCall {
                        block: path.clone(),
                        tool: call.name.clone(),
                        arguments: call.arguments.to_string(),
                    });
                    let fut = self.host.run_tool(&call.name, call.arguments.clone());
                    let result = tokio::select! {
                        r = fut => r,
                        _ = cancelled(self.cancel) => return Err(Stop::Cancelled.into()),
                    };
                    let out = match result {
                        Ok(o) => o,
                        Err(e) => ToolOutput {
                            text: format!("Error: {e}"),
                            is_error: true,
                        },
                    };
                    if out.is_error && p.on_error == OnToolError::Fail {
                        return Err(BlockErr::Failed(format!("{}: {}", call.name, out.text)));
                    }
                    let text = truncate(&out.text, p.max_result_chars as usize);
                    on_event(Event::ToolResult {
                        block: path.clone(),
                        tool: call.name.clone(),
                        preview: Value::Text(text.clone()).preview(),
                        is_error: out.is_error,
                    });
                    ex.messages.push(ExMessage {
                        call_id: Some(call.id.clone()),
                        name: Some(call.name.clone()),
                        ..ExMessage::new(Role::Tool, text)
                    });
                }
                one(Value::Exchange(ex))
            }
            BlockKind::Exchange(p) => {
                let mut ex = Exchange::default();
                let system = p.system.trim();
                let has_system = matches!(arg(0), Value::Exchange(ref e) if e.messages.iter().any(|m| m.role == Role::System));
                if !system.is_empty() && !has_system {
                    ex.messages.push(ExMessage::new(Role::System, system));
                }
                let roles = if p.roles.is_empty() {
                    vec![Role::User]
                } else {
                    p.roles.clone()
                };
                match arg(0) {
                    Value::Text(t) => ex.messages.push(ExMessage::new(roles[0], t)),
                    Value::List(items) => {
                        for (i, t) in items.into_iter().enumerate() {
                            ex.messages.push(ExMessage::new(roles[i % roles.len()], t));
                        }
                    }
                    Value::Exchange(e) => ex.messages.extend(e.messages),
                }
                one(Value::Exchange(ex))
            }
            BlockKind::Augment(p) => {
                let Value::Exchange(mut ex) = arg(0) else {
                    return Err(BlockErr::Failed(
                        "the Exchange input needs an exchange".into(),
                    ));
                };
                let fmt = |t: &str| {
                    if p.format.contains("_INPUT_") {
                        p.format.replace("_INPUT_", t)
                    } else {
                        fill_prompt(&p.format, t)
                    }
                };
                match arg(1) {
                    Value::Text(t) => ex.messages.push(ExMessage::new(p.role, fmt(&t))),
                    Value::List(items) => {
                        for t in items {
                            ex.messages.push(ExMessage::new(p.role, fmt(&t)));
                        }
                    }
                    Value::Exchange(e) => ex
                        .messages
                        .extend(e.messages.into_iter().filter(|m| m.role != Role::System)),
                }
                one(Value::Exchange(ex))
            }
            BlockKind::ReadExchange(p) => {
                let Value::Exchange(ex) = arg(0) else {
                    return Err(BlockErr::Failed("it needs an exchange".into()));
                };
                let last = ex
                    .messages
                    .iter()
                    .rev()
                    .find(|m| p.role.matches(m.role) && !(m.text.is_empty() && !m.calls.is_empty()))
                    .map(|m| m.text.clone())
                    .unwrap_or_default();
                Ok(vec![
                    ("last".into(), Value::Text(last)),
                    ("all".into(), Value::Text(ex.transcript())),
                ])
            }
            BlockKind::Function(p) => {
                self.function_block(frame, id, &p.function, args, on_event)
                    .await
            }
            BlockKind::Combine(p) => {
                let mut out = p.format.clone();
                for i in (1..=args.len()).rev() {
                    out = out.replace(&format!("_INPUT{i}_"), &args[i - 1].as_text());
                }
                one(Value::Text(out))
            }
            BlockKind::Forward(_) => one(arg(0)),
            BlockKind::Remember(_) => {
                let v = frame
                    .remembered
                    .entry(id.to_string())
                    .or_insert_with(|| arg(0))
                    .clone();
                one(v)
            }
            BlockKind::Split(p) => one(Value::List(split(
                &arg(0).as_text(),
                p.chunk_size as usize,
                p.overlap as usize,
                p.deviation as usize,
            ))),
            BlockKind::ForEach(p) => {
                let items = arg(0).as_list();
                let n = if p.limit == 0 {
                    items.len()
                } else {
                    p.limit as usize
                };
                Ok(items
                    .into_iter()
                    .take(n)
                    .map(|t| ("out".to_string(), Value::Text(t)))
                    .collect())
            }
            BlockKind::Separate(p) => Ok(arg(0)
                .as_list()
                .into_iter()
                .take(p.outputs as usize)
                .enumerate()
                .map(|(i, t)| (format!("out{}", i + 1), Value::Text(t)))
                .collect()),
            BlockKind::Join(p) => {
                let sep = p.separator.replace("\\n", "\n").replace("\\t", "\t");
                one(Value::Text(arg(0).as_list().join(&sep)))
            }
            BlockKind::Regex(p) => {
                let re =
                    regex::Regex::new(&p.pattern).map_err(|e| BlockErr::Failed(e.to_string()))?;
                let text = arg(0).as_text();
                one(Value::List(
                    re.find_iter(&text)
                        .map(|m| m.as_str().to_string())
                        .collect(),
                ))
            }
            BlockKind::Capture(p) => {
                let re =
                    regex::Regex::new(&p.pattern).map_err(|e| BlockErr::Failed(e.to_string()))?;
                let text = arg(0).as_text();
                let groups = (re.captures_len() - 1).min(10);
                let mut cols = vec![Vec::new(); groups];
                for caps in re.captures_iter(&text) {
                    for (g, col) in cols.iter_mut().enumerate() {
                        col.push(
                            caps.get(g + 1)
                                .map(|m| m.as_str().to_string())
                                .unwrap_or_default(),
                        );
                    }
                }
                Ok(cols
                    .into_iter()
                    .enumerate()
                    .map(|(i, c)| (format!("group{}", i + 1), Value::List(c)))
                    .collect())
            }
            BlockKind::Branch(p) => {
                let re =
                    regex::Regex::new(&p.pattern).map_err(|e| BlockErr::Failed(e.to_string()))?;
                let port = if re.is_match(&arg(1).as_text()) {
                    "yes"
                } else {
                    "no"
                };
                Ok(vec![(port.to_string(), arg(0))])
            }
            BlockKind::ReadFile(_) => one(Value::Text(
                self.host
                    .read_file(arg(0).as_text().trim())
                    .map_err(BlockErr::Failed)?,
            )),
            BlockKind::WriteFile(p) => {
                let path = arg(0).as_text().trim().to_string();
                self.host
                    .write_file(&path, &arg(1).as_text(), p.append)
                    .map_err(BlockErr::Failed)?;
                one(Value::Text(path))
            }
            BlockKind::ListFiles(_) => one(Value::List(
                self.host
                    .list_files(arg(0).as_text().trim())
                    .map_err(BlockErr::Failed)?,
            )),
        }
    }

    async fn function_block(
        &mut self,
        frame: &Frame<'a>,
        id: &str,
        function: &str,
        args: Vec<Value>,
        on_event: &mut Emit<'_>,
    ) -> Result<Outputs, BlockErr> {
        let file = self.file;
        let def: &'a FunctionDef = file
            .function(function)
            .ok_or_else(|| BlockErr::Failed(format!("the function “{function}” is missing")))?;
        let mut sub = Frame::new(
            &def.blocks,
            &def.links,
            format!("{}/", frame.path(id)),
            &self.analysis,
        );
        for o in &def.outputs {
            sub.exposed
                .entry((o.from.block.clone(), o.from.port.clone()))
                .or_default()
                .push(o.name.clone());
        }
        let in_names: Vec<String> = self
            .analysis
            .functions
            .get(function)
            .map(|f| f.ports.inputs.iter().map(|p| p.name.clone()).collect())
            .unwrap_or_default();
        for (name, value) in in_names.iter().zip(args) {
            for i in def.inputs.iter().filter(|i| &i.name == name) {
                let idx = sub
                    .in_ports
                    .get(&i.to.block)
                    .and_then(|ps| ps.iter().position(|p| *p == i.to.port));
                if let (Some(idx), Some(qs)) = (idx, sub.queues.get_mut(&i.to.block)) {
                    qs[idx].push_back(value.clone());
                }
            }
        }
        let seeded: Vec<String> = def.inputs.iter().map(|i| i.to.block.clone()).collect();
        for b in seeded {
            sub.schedule(&b);
        }
        self.drive(&mut sub, on_event).await?;
        Ok(sub.collected)
    }

    async fn model_block(
        &mut self,
        path: &str,
        p: &LlmParams,
        input: Value,
        on_event: &mut Emit<'_>,
    ) -> Result<Outputs, BlockErr> {
        let system = p.system.trim();
        // The exchange this block continues, and what it sends the model.
        let (base, mut messages) = match input {
            Value::Exchange(ex) => {
                let mut msgs = ex.messages.clone();
                if !system.is_empty() {
                    msgs.retain(|m| m.role != Role::System);
                    msgs.insert(0, ExMessage::new(Role::System, system));
                }
                (ex, msgs)
            }
            other => {
                let mut ex = Exchange::default();
                if !system.is_empty() {
                    ex.messages.push(ExMessage::new(Role::System, system));
                }
                ex.messages.push(ExMessage::new(
                    Role::User,
                    fill_prompt(&p.prompt, &other.as_text()),
                ));
                let msgs = ex.messages.clone();
                (ex, msgs)
            }
        };
        let mut tools = Vec::new();
        for name in &p.tools {
            match self.host.tool_info(name) {
                Some(t) => tools.push(t),
                None => {
                    let text = format!(
                        "The tool “{name}” isn't switched on (Settings → Tools), so the model wasn't offered it."
                    );
                    if self.noticed.insert(text.clone()) {
                        on_event(Event::Notice {
                            block: path.to_string(),
                            text,
                        });
                    }
                }
            }
        }

        let mut attempt = 0;
        loop {
            if self.llm_calls >= self.file.limits.max_llm_calls {
                return Err(Stop::Limit(format!(
                    "Stopped after {} model calls, the loop's limit.",
                    self.file.limits.max_llm_calls
                ))
                .into());
            }
            self.llm_calls += 1;
            let call = LlmCall {
                block: path.to_string(),
                messages: messages.clone(),
                tools: tools.clone(),
                tool_choice: p.tool_choice,
                temperature: p.temperature,
                max_tokens: p.max_tokens,
                model: p.model.clone().filter(|m| !m.is_empty()),
            };
            let answer = self.ask(path, call, on_event).await?;
            let mut reasoning = answer.reasoning.trim().to_string();
            let inline = thinking(&answer.text);
            if !inline.is_empty() {
                if !reasoning.is_empty() {
                    reasoning.push_str("\n\n");
                }
                reasoning.push_str(&inline);
            }
            let text = strip_thinking(&answer.text);

            if tools.is_empty() {
                return Ok(vec![
                    ("out".into(), Value::Text(text)),
                    ("reasoning".into(), Value::Text(reasoning)),
                ]);
            }

            // Check the calls.
            let mut good = Vec::new();
            let mut problems = Vec::new();
            for m in &answer.malformed {
                problems.push(format!(
                    "the call to {} wasn't valid JSON ({})",
                    m.name, m.error
                ));
            }
            for c in &answer.calls {
                match tools.iter().find(|t| t.name == c.name) {
                    None => problems.push(format!(
                        "there's no tool called “{}” (you can use: {})",
                        c.name,
                        tools
                            .iter()
                            .map(|t| t.name.as_str())
                            .collect::<Vec<_>>()
                            .join(", ")
                    )),
                    Some(t) => match check_arguments(&t.parameters, &c.arguments) {
                        Ok(args) => good.push(ToolCall {
                            arguments: args,
                            ..c.clone()
                        }),
                        Err(e) => problems.push(format!("in the call to {}, {e}", c.name)),
                    },
                }
            }
            if good.is_empty() && problems.is_empty() && p.tool_choice == ToolChoice::Required {
                problems.push(format!(
                    "you must call one of these tools: {}",
                    tools
                        .iter()
                        .map(|t| t.name.as_str())
                        .collect::<Vec<_>>()
                        .join(", ")
                ));
            }

            let send_calls = |good: Vec<ToolCall>, text: String, reasoning: String| {
                let mut ex = base.clone();
                ex.messages.push(ExMessage {
                    calls: good,
                    ..ExMessage::new(Role::Assistant, text)
                });
                Ok(vec![
                    ("tool".into(), Value::Exchange(ex)),
                    ("reasoning".into(), Value::Text(reasoning)),
                ])
            };
            if problems.is_empty() {
                if good.is_empty() {
                    return Ok(vec![
                        ("out".into(), Value::Text(text)),
                        ("reasoning".into(), Value::Text(reasoning)),
                    ]);
                }
                return send_calls(good, text, reasoning);
            }

            let problem = problems.join("; ");
            match p.on_bad_call {
                BadCall::Answer => {
                    if !good.is_empty() {
                        return send_calls(good, text, reasoning);
                    }
                    on_event(Event::Notice {
                        block: path.to_string(),
                        text: format!("Bad tool call ignored: {problem}."),
                    });
                    return Ok(vec![
                        ("out".into(), Value::Text(text)),
                        ("reasoning".into(), Value::Text(reasoning)),
                    ]);
                }
                BadCall::Retry if attempt < p.retries => {
                    attempt += 1;
                    on_event(Event::Notice {
                        block: path.to_string(),
                        text: format!(
                            "Bad tool call ({problem}). Asking again ({attempt}/{}).",
                            p.retries
                        ),
                    });
                    let said = if text.is_empty() {
                        let names: Vec<&str> = answer
                            .calls
                            .iter()
                            .map(|c| c.name.as_str())
                            .chain(answer.malformed.iter().map(|m| m.name.as_str()))
                            .collect();
                        if names.is_empty() {
                            "(no tool call)".to_string()
                        } else {
                            format!("(tried to call {})", names.join(", "))
                        }
                    } else {
                        text.clone()
                    };
                    messages.push(ExMessage::new(Role::Assistant, said));
                    messages.push(ExMessage::new(
                        Role::User,
                        format!(
                            "That didn't work: {problem}. Try again: call a tool with correct arguments{}.",
                            if p.tool_choice == ToolChoice::Required {
                                ""
                            } else {
                                ", or answer without a tool"
                            }
                        ),
                    ));
                }
                _ => {
                    return Err(BlockErr::Failed(format!(
                        "the model made a bad tool call: {problem}"
                    )));
                }
            }
        }
    }

    /// One model request, streaming its text as events.
    async fn ask(
        &self,
        path: &str,
        call: LlmCall,
        on_event: &mut Emit<'_>,
    ) -> Result<LlmAnswer, BlockErr> {
        let block = path.to_string();
        let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel::<String>();
        let mut on_text = move |t: &str| {
            let _ = tx.send(t.to_string());
        };
        let fut = self.host.llm(call, &mut on_text);
        tokio::pin!(fut);
        let answer = loop {
            tokio::select! {
                r = &mut fut => break r,
                Some(t) = rx.recv() => on_event(Event::LlmText { block: block.clone(), text: t }),
                _ = cancelled(self.cancel) => return Err(Stop::Cancelled.into()),
            }
        };
        while let Ok(t) = rx.try_recv() {
            on_event(Event::LlmText {
                block: block.clone(),
                text: t,
            });
        }
        answer.map_err(BlockErr::Failed)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::format::{Block, Link, PortRef, Position};
    use serde_json::Value as Json;
    use serde_json::json;
    use std::sync::Mutex;

    /// A model that answers from a script and records prompts.
    #[derive(Default)]
    struct Fake {
        answers: Mutex<VecDeque<LlmAnswer>>,
        prompts: Mutex<Vec<String>>,
        calls: Mutex<Vec<LlmCall>>,
        files: Mutex<HashMap<String, String>>,
        tool_runs: Mutex<Vec<(String, Json)>>,
    }

    fn text(s: &str) -> LlmAnswer {
        LlmAnswer {
            text: s.into(),
            ..Default::default()
        }
    }

    fn calls(name: &str, args: Json) -> LlmAnswer {
        LlmAnswer {
            calls: vec![ToolCall {
                id: format!("c-{name}"),
                name: name.into(),
                arguments: args,
            }],
            ..Default::default()
        }
    }

    impl Fake {
        fn with(answers: &[&str]) -> Self {
            Self::script(answers.iter().map(|s| text(s)).collect())
        }
        fn script(answers: Vec<LlmAnswer>) -> Self {
            let f = Self::default();
            f.answers.lock().unwrap().extend(answers);
            f
        }
    }

    impl Host for Fake {
        async fn llm(
            &self,
            call: LlmCall,
            on_text: &mut (dyn FnMut(&str) + Send),
        ) -> Result<LlmAnswer, String> {
            let prompt = call
                .messages
                .last()
                .map(|m| m.text.clone())
                .unwrap_or_default();
            self.prompts.lock().unwrap().push(prompt.clone());
            self.calls.lock().unwrap().push(call);
            let a = self
                .answers
                .lock()
                .unwrap()
                .pop_front()
                .unwrap_or_else(|| text(&format!("echo: {prompt}")));
            on_text(&a.text);
            Ok(a)
        }
        fn tool_info(&self, name: &str) -> Option<ToolInfo> {
            (name == "echo").then(|| ToolInfo {
                name: "echo".into(),
                description: "Echoes text.".into(),
                parameters: json!({
                    "type": "object",
                    "properties": { "text": { "type": "string" } },
                    "required": ["text"],
                    "additionalProperties": false
                }),
            })
        }
        async fn run_tool(&self, name: &str, arguments: Json) -> Result<ToolOutput, String> {
            self.tool_runs
                .lock()
                .unwrap()
                .push((name.into(), arguments.clone()));
            Ok(ToolOutput {
                text: format!("echoed {}", arguments["text"].as_str().unwrap_or("")),
                is_error: false,
            })
        }
        fn read_file(&self, path: &str) -> Result<String, String> {
            self.files
                .lock()
                .unwrap()
                .get(path)
                .cloned()
                .ok_or_else(|| format!("{path}: not found"))
        }
        fn write_file(&self, path: &str, content: &str, append: bool) -> Result<(), String> {
            let mut f = self.files.lock().unwrap();
            let e = f.entry(path.to_string()).or_default();
            if !append {
                e.clear();
            }
            e.push_str(content);
            Ok(())
        }
        fn list_files(&self, _path: &str) -> Result<Vec<String>, String> {
            Ok(self.files.lock().unwrap().keys().cloned().collect())
        }
    }

    fn b(id: &str, kind: &str, params: serde_json::Value) -> Block {
        Block {
            id: id.into(),
            kind: kind.into(),
            label: id.into(),
            position: Position::default(),
            params,
        }
    }
    fn ln(a: &str, ap: &str, c: &str, cp: &str) -> Link {
        Link {
            from: PortRef {
                block: a.into(),
                port: ap.into(),
            },
            to: PortRef {
                block: c.into(),
                port: cp.into(),
            },
        }
    }
    fn lp(blocks: Vec<Block>, links: Vec<Link>) -> LoopFile {
        let mut l = LoopFile::new("t");
        l.blocks = blocks;
        l.links = links;
        l
    }
    async fn go(l: &LoopFile, host: &Fake, msg: &str) -> (RunOutcome, Vec<Event>) {
        let mut ev = vec![];
        let inputs = RunInputs {
            message: msg.into(),
            ..Default::default()
        };
        let out = run(l, host, &inputs, &AtomicBool::new(false), |e| ev.push(e)).await;
        (out, ev)
    }

    #[tokio::test]
    async fn linear_loop_replies() {
        let l = lp(
            vec![
                b("in", "input", json!({})),
                b("llm", "llm", json!({ "prompt": "Q: _INPUT_" })),
                b("out", "output", json!({})),
            ],
            vec![ln("in", "out", "llm", "in"), ln("llm", "out", "out", "in")],
        );
        let host = Fake::with(&["<think>hmm</think>A!"]);
        let (o, ev) = go(&l, &host, "hello").await;
        assert_eq!(o.status, RunStatus::Completed);
        assert_eq!(o.replies, vec!["A!"]);
        assert_eq!(host.prompts.lock().unwrap()[0], "Q: hello");
        assert!(ev.iter().any(|e| matches!(e, Event::LlmText { .. })));
        assert_eq!((o.steps, o.llm_calls), (3, 1));
    }

    #[tokio::test]
    async fn combine_waits_for_all_inputs() {
        let l = lp(
            vec![
                b("in", "input", json!({})),
                b("fx", "fixed", json!({ "text": "world" })),
                b(
                    "c",
                    "combine",
                    json!({ "inputs": 2, "format": "_INPUT1_, _INPUT2_" }),
                ),
                b("out", "output", json!({})),
            ],
            vec![
                ln("in", "out", "c", "in1"),
                ln("fx", "out", "c", "in2"),
                ln("c", "out", "out", "in"),
            ],
        );
        let (o, ev) = go(&l, &Fake::default(), "hello").await;
        assert_eq!(o.replies, vec!["hello, world"]);
        assert!(ev.contains(&Event::BlockState {
            block: "c".into(),
            label: "c".into(),
            state: BlockState::Waiting
        }));
    }

    #[tokio::test]
    async fn broadcasts_and_queues_multiple_arrivals() {
        // Two links into one port: the block runs once per arrival.
        let l = lp(
            vec![
                b("a", "fixed", json!({ "text": "one" })),
                b("b", "fixed", json!({ "text": "two" })),
                b("f", "forward", json!({})),
                b("o1", "output", json!({})),
            ],
            vec![
                ln("a", "out", "f", "in"),
                ln("b", "out", "f", "in"),
                ln("f", "out", "o1", "in"),
            ],
        );
        let (o, _) = go(&l, &Fake::default(), "").await;
        assert_eq!(o.replies, vec!["one", "two"]);
    }

    #[tokio::test]
    async fn self_check_cycle_retries_until_yes() {
        // draft → check → branch(yes → reply, no → improve → back to check)
        let l = lp(
            vec![
                b("in", "input", json!({})),
                b("draft", "llm", json!({ "prompt": "draft _INPUT_" })),
                b("check", "llm", json!({ "prompt": "check _INPUT_" })),
                b("br", "branch", json!({ "pattern": "(?i)^yes" })),
                b("improve", "llm", json!({ "prompt": "improve _INPUT_" })),
                b("out", "output", json!({})),
            ],
            vec![
                ln("in", "out", "draft", "in"),
                ln("draft", "out", "br", "value"),
                ln("draft", "out", "check", "in"),
                ln("check", "out", "br", "test"),
                ln("br", "yes", "out", "in"),
                ln("br", "no", "improve", "in"),
                ln("improve", "out", "br", "value"),
                ln("improve", "out", "check", "in"),
            ],
        );
        let host = Fake::with(&["v1", "NO", "v2", "Yes, good"]);
        let (o, _) = go(&l, &host, "task").await;
        assert_eq!(o.status, RunStatus::Completed);
        assert_eq!(o.replies, vec!["v2"]);
        assert_eq!(o.llm_calls, 4);
    }

    #[tokio::test]
    async fn infinite_cycles_hit_the_limit() {
        let mut l = lp(
            vec![
                b("in", "input", json!({})),
                b("llm", "llm", json!({})),
                b("out", "output", json!({})),
            ],
            vec![
                ln("in", "out", "llm", "in"),
                ln("llm", "out", "llm", "in"),
                ln("llm", "out", "out", "in"),
            ],
        );
        l.limits.max_llm_calls = 3;
        let (o, _) = go(&l, &Fake::default(), "x").await;
        assert!(matches!(o.status, RunStatus::LimitReached { .. }));
        assert_eq!(o.llm_calls, 3);
        assert_eq!(o.replies.len(), 2);

        l.limits = RunLimits {
            max_steps: 5,
            max_llm_calls: 100,
            max_tool_calls: 10,
        };
        let (o, _) = go(&l, &Fake::default(), "x").await;
        assert!(matches!(o.status, RunStatus::LimitReached { .. }));
    }

    #[tokio::test]
    async fn list_blocks_work() {
        let l = lp(
            vec![
                b("in", "input", json!({})),
                b("re", "regex", json!({ "pattern": "\\d+" })),
                b("each", "for_each", json!({})),
                b("pick", "separate", json!({ "outputs": 2 })),
                b("join", "join", json!({ "separator": "+" })),
                b("cap", "capture", json!({ "pattern": "(\\w)=(\\d)" })),
                b("o", "output", json!({})),
            ],
            vec![
                ln("in", "out", "re", "in"),
                ln("re", "out", "each", "in"),
                ln("re", "out", "pick", "in"),
                ln("re", "out", "join", "in"),
                ln("each", "out", "o", "in"),
                ln("pick", "out2", "o", "in"),
                ln("join", "out", "o", "in"),
                ln("in", "out", "cap", "in"),
                ln("cap", "group2", "join", "in"),
            ],
        );
        let (o, _) = go(&l, &Fake::default(), "a=1 b=2 c=3").await;
        assert_eq!(o.status, RunStatus::Completed, "{o:?}");
        assert_eq!(o.replies, vec!["1", "2", "3", "2", "1+2+3", "1+2+3"]);
    }

    #[tokio::test]
    async fn workspace_blocks_use_the_host() {
        let l = lp(
            vec![
                b("p", "fixed", json!({ "text": "notes.md" })),
                b("c", "fixed", json!({ "text": "hello" })),
                b("w", "write_file", json!({})),
                b("r", "read_file", json!({})),
                b("o", "output", json!({})),
            ],
            vec![
                ln("p", "out", "w", "path"),
                ln("c", "out", "w", "content"),
                ln("w", "out", "r", "path"),
                ln("r", "out", "o", "in"),
            ],
        );
        let host = Fake::default();
        let (o, _) = go(&l, &host, "").await;
        assert_eq!(o.replies, vec!["hello"]);

        let l = lp(
            vec![
                b("p", "fixed", json!({ "text": "missing.txt" })),
                b("r", "read_file", json!({})),
                b("o", "output", json!({})),
            ],
            vec![ln("p", "out", "r", "path"), ln("r", "out", "o", "in")],
        );
        let (o, _) = go(&l, &host, "").await;
        assert!(
            matches!(o.status, RunStatus::Failed { ref error, .. } if error.contains("not found"))
        );
    }

    #[tokio::test]
    async fn invalid_loops_do_not_run_and_waiting_is_reported() {
        let l = lp(vec![b("o", "output", json!({}))], vec![]);
        let (o, _) = go(&l, &Fake::default(), "").await;
        assert!(matches!(o.status, RunStatus::Failed { .. }));

        let l = lp(
            vec![
                b("in", "input", json!({})),
                b("c", "combine", json!({ "inputs": 2 })),
            ],
            vec![ln("in", "out", "c", "in1")],
        );
        let (o, _) = go(&l, &Fake::default(), "").await;
        assert_eq!(o.waiting, vec!["c"]);
    }

    #[tokio::test]
    async fn history_and_overrides() {
        let l = lp(
            vec![
                b("h", "history", json!({ "max_messages": 1 })),
                b("in", "input", json!({})),
                b("o", "output", json!({})),
            ],
            vec![ln("h", "out", "o", "in"), ln("in", "out", "o", "in")],
        );
        let inputs = RunInputs {
            message: "new".into(),
            history: vec![
                ChatTurn {
                    role: "user".into(),
                    text: "old".into(),
                },
                ChatTurn {
                    role: "assistant".into(),
                    text: "reply".into(),
                },
            ],
            overrides: HashMap::from([("in".to_string(), "override".to_string())]),
        };
        let out = run(
            &l,
            &Fake::default(),
            &inputs,
            &AtomicBool::new(false),
            |_| {},
        )
        .await;
        assert_eq!(out.replies, vec!["Assistant: reply", "override"]);
    }

    #[tokio::test]
    async fn can_be_stopped() {
        let l = lp(
            vec![b("in", "input", json!({})), b("o", "output", json!({}))],
            vec![ln("in", "out", "o", "in")],
        );
        let out = run(
            &l,
            &Fake::default(),
            &RunInputs::default(),
            &AtomicBool::new(true),
            |_| {},
        )
        .await;
        assert_eq!(out.status, RunStatus::Stopped);
    }

    fn tool_loop() -> LoopFile {
        // message → model(tools) → tool → run → back to model; answer → reply
        lp(
            vec![
                b("in", "input", json!({})),
                b(
                    "m",
                    "llm",
                    json!({ "prompt": "Do: _INPUT_", "tools": ["echo"] }),
                ),
                b("run", "run_tool", json!({})),
                b("out", "output", json!({})),
                b("think", "output", json!({})),
            ],
            vec![
                ln("in", "out", "m", "in"),
                ln("m", "tool", "run", "in"),
                ln("run", "out", "m", "in"),
                ln("m", "out", "out", "in"),
            ],
        )
    }

    #[tokio::test]
    async fn model_calls_tools_and_continues_the_exchange() {
        let host = Fake::script(vec![
            LlmAnswer {
                reasoning: "I should echo".into(),
                ..calls("echo", json!({ "text": "hi" }))
            },
            text("<think>done now</think>All done: echoed hi"),
        ]);
        let (o, ev) = go(&tool_loop(), &host, "say hi").await;
        assert_eq!(o.status, RunStatus::Completed, "{o:?}");
        assert_eq!(o.replies, vec!["All done: echoed hi"]);
        assert_eq!((o.llm_calls, o.tool_calls), (2, 1));
        assert_eq!(host.tool_runs.lock().unwrap()[0].1, json!({ "text": "hi" }));
        // The second request continued the exchange: prompt, call, result.
        let second = &host.calls.lock().unwrap()[1];
        let roles: Vec<Role> = second.messages.iter().map(|m| m.role).collect();
        assert_eq!(roles, vec![Role::User, Role::Assistant, Role::Tool]);
        assert_eq!(second.messages[0].text, "Do: say hi");
        assert_eq!(second.messages[2].text, "echoed hi");
        assert_eq!(second.tools.len(), 1);
        // Answer was silent while the tool was used; reasoning went out both times.
        let outs: Vec<(String, String)> = ev
            .iter()
            .filter_map(|e| match e {
                Event::BlockOutput {
                    block,
                    port,
                    preview,
                } if block == "m" => Some((port.clone(), preview.clone())),
                _ => None,
            })
            .collect();
        assert_eq!(outs[0].0, "tool");
        assert_eq!(outs[1], ("reasoning".into(), "I should echo".into()));
        assert_eq!(outs[2], ("out".into(), "All done: echoed hi".into()));
        assert_eq!(outs[3], ("reasoning".into(), "done now".into()));
        assert!(
            ev.iter()
                .any(|e| matches!(e, Event::ToolResult { tool, .. } if tool == "echo"))
        );
    }

    #[tokio::test]
    async fn bad_tool_calls_are_retried_or_fail_or_become_answers() {
        // Unknown tool, then bad arguments (coerced number is fine), then good.
        let host = Fake::script(vec![
            calls("nope", json!({})),
            calls("echo", json!({ "words": "x" })),
            calls("echo", json!({ "text": 5 })),
            text("ok"),
        ]);
        let (o, ev) = go(&tool_loop(), &host, "x").await;
        assert_eq!(o.replies, vec!["ok"], "{o:?}");
        assert_eq!(host.tool_runs.lock().unwrap()[0].1, json!({ "text": "5" }));
        let notices = ev
            .iter()
            .filter(|e| matches!(e, Event::Notice { .. }))
            .count();
        assert_eq!(notices, 2);
        let retry = host.calls.lock().unwrap()[1].clone();
        assert!(
            retry
                .messages
                .last()
                .unwrap()
                .text
                .contains("no tool called “nope”")
        );

        // Out of retries → the run fails.
        let mut l = tool_loop();
        l.blocks[1].params = json!({ "tools": ["echo"], "retries": 1 });
        let host = Fake::script(vec![calls("nope", json!({})), calls("nope", json!({}))]);
        let (o, _) = go(&l, &host, "x").await;
        assert!(
            matches!(o.status, RunStatus::Failed { ref error, .. } if error.contains("bad tool call")),
            "{o:?}"
        );

        // "answer": the text is used.
        l.blocks[1].params = json!({ "tools": ["echo"], "on_bad_call": "answer" });
        let host = Fake::script(vec![LlmAnswer {
            text: "plain".into(),
            ..calls("nope", json!({}))
        }]);
        let (o, _) = go(&l, &host, "x").await;
        assert_eq!(o.replies, vec!["plain"]);

        // "required": answering without a tool counts as a bad call.
        l.blocks[1].params =
            json!({ "tools": ["echo"], "tool_choice": "required", "on_bad_call": "fail" });
        let host = Fake::with(&["no tools for me"]);
        let (o, _) = go(&l, &host, "x").await;
        assert!(matches!(o.status, RunStatus::Failed { .. }));

        // A tool that isn't switched on isn't offered; the block still answers.
        l.blocks[1].params = json!({ "tools": ["missing"] });
        let host = Fake::with(&["fine"]);
        let (o, ev) = go(&l, &host, "x").await;
        assert_eq!(o.replies, vec!["fine"]);
        assert!(host.calls.lock().unwrap()[0].tools.is_empty());
        assert!(ev.iter().any(
            |e| matches!(e, Event::Notice { text, .. } if text.contains("isn't switched on"))
        ));
    }

    #[tokio::test]
    async fn tool_call_limit() {
        let mut l = tool_loop();
        l.limits.max_tool_calls = 2;
        let host = Fake::script(
            (0..5)
                .map(|_| calls("echo", json!({ "text": "a" })))
                .collect(),
        );
        let (o, _) = go(&l, &host, "x").await;
        assert!(
            matches!(o.status, RunStatus::LimitReached { ref message } if message.contains("tool calls")),
            "{o:?}"
        );
        assert_eq!(o.tool_calls, 2);
    }

    #[tokio::test]
    async fn exchange_blocks() {
        let l = lp(
            vec![
                b("in", "input", json!({})),
                b("goal", "fixed", json!({ "text": "Be brief." })),
                b(
                    "start",
                    "exchange",
                    json!({ "system": "You help.", "roles": ["user"] }),
                ),
                b(
                    "add",
                    "augment",
                    json!({ "role": "assistant", "format": "Noted: _INPUT_" }),
                ),
                b("m", "llm", json!({})),
                b("read", "read_exchange", json!({ "role": "assistant" })),
                b("o", "output", json!({})),
            ],
            vec![
                ln("in", "out", "start", "in"),
                ln("start", "out", "add", "exchange"),
                ln("goal", "out", "add", "in"),
                ln("add", "out", "m", "in"),
                ln("add", "out", "read", "in"),
                ln("read", "last", "o", "in"),
                ln("m", "out", "o", "in"),
            ],
        );
        let host = Fake::with(&["model answer"]);
        let (o, _) = go(&l, &host, "hello").await;
        assert_eq!(o.status, RunStatus::Completed, "{o:?}");
        assert_eq!(o.replies, vec!["model answer", "Noted: Be brief."]);
        let sent = &host.calls.lock().unwrap()[0].messages;
        assert_eq!(
            sent.iter()
                .map(|m| (m.role, m.text.as_str()))
                .collect::<Vec<_>>(),
            vec![
                (Role::System, "You help."),
                (Role::User, "hello"),
                (Role::Assistant, "Noted: Be brief."),
            ]
        );
    }

    fn func(
        id: &str,
        blocks: Vec<Block>,
        links: Vec<Link>,
        ins: &[(&str, &str, &str)],
        outs: &[(&str, &str, &str)],
    ) -> FunctionDef {
        FunctionDef {
            id: id.into(),
            name: id.into(),
            description: String::new(),
            inputs: ins
                .iter()
                .map(|(n, blk, port)| crate::format::FunctionInput {
                    name: (*n).into(),
                    label: String::new(),
                    to: PortRef {
                        block: (*blk).into(),
                        port: (*port).into(),
                    },
                })
                .collect(),
            outputs: outs
                .iter()
                .map(|(n, blk, port)| crate::format::FunctionOutput {
                    name: (*n).into(),
                    label: String::new(),
                    from: PortRef {
                        block: (*blk).into(),
                        port: (*port).into(),
                    },
                })
                .collect(),
            blocks,
            links,
        }
    }

    #[tokio::test]
    async fn function_blocks_run_their_blocks_per_call() {
        // shout(x) = combine("_INPUT1_!" , suffix) ; used twice, and nested.
        let shout = func(
            "shout",
            vec![
                b("sfx", "fixed", json!({ "text": "!" })),
                b(
                    "c",
                    "combine",
                    json!({ "inputs": 2, "format": "_INPUT1__INPUT2_" }),
                ),
                b("m", "remember", json!({})),
            ],
            vec![ln("sfx", "out", "c", "in2"), ln("c", "out", "m", "in")],
            &[("in1", "c", "in1")],
            &[("out1", "m", "out")],
        );
        let twice = func(
            "twice",
            vec![
                b("a", "function", json!({ "function": "shout" })),
                b("b2", "function", json!({ "function": "shout" })),
            ],
            vec![ln("a", "out1", "b2", "in1")],
            &[("x", "a", "in1")],
            &[("y", "b2", "out1")],
        );
        let mut l = lp(
            vec![
                b("in", "input", json!({})),
                b("each", "split", json!({ "chunk_size": 100 })),
                b("rx", "regex", json!({ "pattern": "\\w+" })),
                b("fe", "for_each", json!({ "limit": 2 })),
                b("f", "function", json!({ "function": "twice" })),
                b("o", "output", json!({})),
            ],
            vec![
                ln("in", "out", "rx", "in"),
                ln("rx", "out", "fe", "in"),
                ln("fe", "out", "f", "x"),
                ln("f", "y", "o", "in"),
            ],
        );
        l.blocks.remove(1);
        l.functions = vec![shout, twice];
        let a = analyze(&l);
        assert!(a.ok, "{:?}", a.issues);
        assert_eq!(a.functions["twice"].ports.inputs[0].name, "x");
        let (o, ev) = go(&l, &Fake::default(), "hey you there").await;
        assert_eq!(o.status, RunStatus::Completed, "{o:?}");
        // Remember is fresh per call, so each word is shouted on its own.
        assert_eq!(o.replies, vec!["hey!!", "you!!"]);
        assert!(
            ev.iter()
                .any(|e| matches!(e, Event::BlockState { block, .. } if block == "f/a/c"))
        );
    }

    #[test]
    fn functions_cannot_use_themselves() {
        let mut l = lp(
            vec![
                b("in", "input", json!({})),
                b("f", "function", json!({ "function": "loopy" })),
            ],
            vec![ln("in", "out", "f", "x")],
        );
        l.functions = vec![func(
            "loopy",
            vec![b("inner", "function", json!({ "function": "loopy" }))],
            vec![],
            &[],
            &[],
        )];
        let a = analyze(&l);
        assert!(!a.ok);
        assert!(
            a.issues.iter().any(|i| i.message.contains("uses itself")),
            "{:?}",
            a.issues
        );

        // A bad port mapping is reported on the function.
        l.functions = vec![func(
            "bad",
            vec![b("c", "forward", json!({}))],
            vec![],
            &[("x", "c", "nope")],
            &[],
        )];
        l.blocks[1].params = json!({ "function": "bad" });
        let a = analyze(&l);
        assert!(a.issues.iter().any(|i| i.function.as_deref() == Some("bad") && i.message.contains("missing port")), "{:?}", a.issues);
    }
}
