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
//! - A run can be **paused** ([`run_with`] with a pause flag): it stops at the
//!   next block boundary, in whatever function call it is in, and hands back
//!   its whole state as a [`RunState`]. [`run_with`] given that state carries
//!   on exactly where it stopped. A block waiting on the person (a question,
//!   a tool approval, a daily token limit) is interrupted and runs again on
//!   resuming, unless a tool already ran in it; a model that can't be reached
//!   pauses the run at the model block, which asks again on resuming.

use std::collections::{BTreeMap, HashMap, VecDeque};
use std::future::Future;
use std::pin::Pin;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use serde::{Deserialize, Serialize};
use serde_json::Value as Json;

use crate::blocks::{
    BadCall, BlockKind, FAILED, FAILED_EXCHANGE, LlmParams, OnLimit, OnToolError, ToolChoice,
    ToolMode, failure_message,
};
use crate::blocks::{Switch, ToolSwitches};
use crate::exchange::{ExMessage, Exchange, Role, ToolCall, ToolInfo, check_arguments};
use crate::format::{Block, FunctionDef, Link, LoopFile};
use crate::generation::GenerationOverrides;
use crate::log::{LOG_VALUE_CHARS, ModelLog, PortLog, StepLog, StepStatus, logged, now_ms};
use crate::prompted;
use crate::text::{
    Memory, fill_template, split, strip_thinking, thinking, truncate, valid_memory_key,
};
use crate::validate::{Analysis, Severity, analyze};

/// A value flowing between blocks.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", content = "value", rename_all = "snake_case")]
pub enum Value {
    Text(String),
    List(Vec<String>),
    Exchange(Exchange),
}

/// The largest value a block may make. Far beyond any real text, it stops
/// a loop that keeps doubling a value (Combine fed its own output, say)
/// before it takes all the computer's memory.
pub const MAX_VALUE_BYTES: usize = 64 * 1024 * 1024;

impl Value {
    /// About how many bytes the value holds.
    pub fn bytes(&self) -> usize {
        match self {
            Value::Text(t) => t.len(),
            Value::List(l) => l.iter().map(|x| x.len() + 1).sum(),
            Value::Exchange(e) => e
                .messages
                .iter()
                .map(|m| {
                    m.text.len()
                        + m.calls
                            .iter()
                            .map(|c| c.name.len() + c.arguments.to_string().len())
                            .sum::<usize>()
                })
                .sum(),
        }
    }

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
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
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
    /// `None`: the model connection's own setting.
    pub max_tokens: Option<u32>,
    /// The block's own reasoning effort, sampling and the rest (empty: all
    /// inherited).
    pub generation: GenerationOverrides,
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

/// Why a model call failed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LlmFailure {
    pub message: String,
    /// The model couldn't be reached at all (no connection, timed out, the
    /// server is down or overloaded), as opposed to refusing the request.
    /// A run that can pause pauses on this instead of failing.
    pub unavailable: bool,
}

impl LlmFailure {
    pub fn unavailable(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
            unavailable: true,
        }
    }
}

impl From<String> for LlmFailure {
    fn from(message: String) -> Self {
        Self {
            message,
            unavailable: false,
        }
    }
}

impl From<&str> for LlmFailure {
    fn from(message: &str) -> Self {
        message.to_string().into()
    }
}

/// A tool's result.
#[derive(Debug, Clone, PartialEq)]
pub struct ToolOutput {
    pub text: String,
    /// The tool reported a problem; `text` says what.
    pub is_error: bool,
}

/// What a run wants from the person using Chummy.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum AskKind {
    /// A question from an *Ask me* block; answered with text.
    Question,
    /// A tool call waiting to be allowed or refused.
    ToolApproval { tool: String, arguments: Json },
    /// A model has used its daily tokens: allow `extra` more today?
    Budget {
        model: String,
        used: u64,
        limit: u64,
        extra: u64,
    },
}

/// Whether a model may be called now.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Budget {
    Ok,
    /// Its daily tokens are used up.
    Exhausted {
        /// The model connection's name, for people.
        model: String,
        used: u64,
        limit: u64,
        /// Until the allowance refreshes.
        resets_in: Duration,
        /// How much more to offer when asking the user.
        extra: u64,
    },
}

/// A request for the user, from a block.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Ask {
    /// Path of the asking block.
    pub block: String,
    /// Its label.
    pub label: String,
    #[serde(flatten)]
    pub kind: AskKind,
    /// The question, or what the tool call is about to do.
    pub text: String,
}

/// The user's reply to an [`Ask`].
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", content = "text", rename_all = "snake_case")]
pub enum Answer {
    /// The answer to a question.
    Text(String),
    /// Run the tool call.
    Allow,
    /// Don't run it, optionally saying why (the model is told).
    Deny(String),
}

/// What the engine needs from the outside world.
pub trait Host: Send + Sync {
    /// Asks a model. `on_text` receives the answer as it streams.
    fn llm(
        &self,
        call: LlmCall,
        on_text: &mut (dyn FnMut(&str) + Send),
    ) -> impl Future<Output = Result<LlmAnswer, LlmFailure>> + Send;
    /// A tool that's installed and that the app and the agent allow
    /// (Security and Privacy and the agent's tools), or `None`.
    fn tool_info(&self, name: &str) -> Option<ToolInfo>;
    /// Every tool the app allows, for blocks that inherit.
    fn tool_names(&self) -> Vec<String> {
        vec![]
    }
    /// Whether the agent's own switch allows this tool.
    fn agent_allows(&self, _tool: &str) -> bool {
        true
    }
    /// Runs a tool. `Err` means it couldn't run at all.
    /// Runs a tool. `network`: the calling block's internet switch (the host
    /// resolves inherit with the agent's switch, and applies the app's).
    fn run_tool(
        &self,
        name: &str,
        arguments: Json,
        network: Switch,
    ) -> impl Future<Output = Result<ToolOutput, String>> + Send;
    fn read_file(&self, path: &str) -> Result<String, String>;
    fn write_file(&self, path: &str, content: &str, append: bool) -> Result<(), String>;
    /// Names in a folder; folders end with `/`.
    fn list_files(&self, path: &str) -> Result<Vec<String>, String>;
    /// Puts `ask` in front of the user and waits for the answer. `Err`
    /// means nobody can be asked (the run then stops with that message).
    /// The engine handles stopping and timeouts; this just waits.
    fn ask(&self, ask: Ask) -> impl Future<Output = Result<Answer, String>> + Send;
    /// Whether calls to `tool` must be allowed by the user first, whatever
    /// the Run tool block says (e.g. tools that would go online).
    fn must_confirm(&self, _tool: &str, _network: Switch) -> bool {
        false
    }
    /// Whether the model (`None`: the agent's) may be called now.
    fn budget(&self, _model: Option<&str>) -> Budget {
        Budget::Ok
    }
    /// The user allowed `tokens` more for the model today.
    fn grant(&self, _model: Option<&str>, _tokens: u64) {}
    /// How long the model's (`None`: the agent's) context is, and when a
    /// loop should make room in it.
    fn context(&self, _model: Option<&str>) -> ContextLimits {
        ContextLimits::default()
    }
}

/// The longest tool result a Run tool block passes on: its own limit, but
/// no more than about half the model's context (at ~4 characters a token),
/// and never under 12,000 characters, so one result can't fill the context
/// of a small model.
pub fn result_cap(max_result_chars: u32, window_tokens: u32) -> usize {
    let half_context = (window_tokens as usize).saturating_mul(2).max(12_000);
    (max_result_chars as usize).min(half_context)
}

/// Context length assumed when nothing says otherwise.
pub const DEFAULT_CONTEXT_WINDOW: u32 = 8192;

/// A model's context length and the point at which loops should make room
/// (Context full? blocks), both in tokens.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct ContextLimits {
    pub window: u32,
    pub compact_at: u32,
}

impl Default for ContextLimits {
    fn default() -> Self {
        Self {
            window: DEFAULT_CONTEXT_WINDOW,
            compact_at: DEFAULT_CONTEXT_WINDOW / 10 * 8,
        }
    }
}

/// Token figures for one run (one turn of a chat), all estimates.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TurnUsage {
    /// Tokens this turn added to context: the message, every model answer
    /// (with its tool calls) and every tool result.
    pub tokens_added: u64,
    /// Tokens in the latest model call's context: what it was sent plus its
    /// answer.
    pub context_tokens: u64,
    /// That model's context length (0: no model was called).
    pub context_window: u32,
    /// That model's connection id (`None`: the agent's model).
    pub model: Option<String>,
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
    /// A Pane output block added a line to the run's pane.
    Pane { block: String, text: String },
    /// A block is waiting for the user (a question, or a tool to allow).
    Asked { block: String, text: String },
    /// The wait ended (answered, timed out or stopped).
    Answered { block: String },
    /// A block finished (or failed): what went in and what came out.
    Step(Box<StepLog>),
    /// A model was called: what was sent and what came back.
    ModelCall(Box<ModelLog>),
}

/// Longest line a Pane output block shows.
pub const PANE_LINE_CHARS: usize = 1000;

/// One input of a Pane output block as one line: whitespace (newlines
/// included) folded to single spaces, trimmed, and cut to
/// [`PANE_LINE_CHARS`] with an ellipsis.
pub fn pane_line(text: &str) -> String {
    let line = text.split_whitespace().collect::<Vec<_>>().join(" ");
    if line.chars().count() > PANE_LINE_CHARS {
        let mut cut: String = line.chars().take(PANE_LINE_CHARS - 1).collect();
        cut.push('…');
        cut
    } else {
        line
    }
}

/// Longest argument value shown in full when asking to run a tool.
const APPROVAL_VALUE_CHARS: usize = 2000;

/// Makes text safe to show for an approval: control and direction-changing
/// characters (which can make text look like something else) are shown as
/// visible codes instead.
fn visible(s: &str) -> String {
    s.chars()
        .map(|c| match c {
            '\n' | '\t' => c.to_string(),
            c if c.is_control()
                || matches!(c, '\u{200b}'..='\u{200f}' | '\u{202a}'..='\u{202e}' | '\u{2066}'..='\u{2069}' | '\u{feff}') =>
            {
                format!("<U+{:04X}>", c as u32)
            }
            c => c.to_string(),
        })
        .collect()
}

/// What a tool call will do, for the user to allow. Every argument is shown;
/// a value too long to show says how much was left out, so nothing can hide
/// past the end.
fn describe_call(tool: &str, arguments: &Json) -> String {
    let args = match arguments.as_object() {
        Some(o) if !o.is_empty() => o
            .iter()
            .map(|(k, v)| {
                let v = match v {
                    Json::String(s) => s.clone(),
                    other => other.to_string(),
                };
                let n = v.chars().count();
                let shown = if n > APPROVAL_VALUE_CHARS {
                    format!(
                        "{} … ({} more characters not shown)",
                        v.chars().take(APPROVAL_VALUE_CHARS).collect::<String>(),
                        n - APPROVAL_VALUE_CHARS
                    )
                } else {
                    v
                };
                format!("{}: {}", visible(k), visible(&shown))
            })
            .collect::<Vec<_>>()
            .join("\n"),
        _ => String::new(),
    };
    let tool = visible(tool);
    if args.is_empty() {
        format!("Run the tool “{tool}”?")
    } else {
        format!("Run the tool “{tool}” with:\n{args}")
    }
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
    /// Paused: [`RunOutcome::paused`] holds what it needs to carry on.
    Paused {
        /// Why, when the run paused itself (a model it couldn't reach);
        /// `None` when it was asked to.
        reason: Option<String>,
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
    pub usage: TurnUsage,
    /// A paused run's state, to give [`run_with`] to carry on. Never sent to
    /// windows: it holds everything the run has seen.
    #[serde(skip)]
    pub paused: Option<Box<RunState>>,
}

/// Version of [`RunState`]; a state of any other version isn't resumed.
pub const RUN_STATE_VERSION: u32 = 1;

/// Everything a paused run needs to carry on: the state of the loop and of
/// each function call it was inside, and what the run had gathered.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RunState {
    pub version: u32,
    /// The loop's graph first, then one per function call it was inside,
    /// innermost last.
    frames: Vec<FrameState>,
    replies: Vec<String>,
    steps: u32,
    llm_calls: u32,
    tool_calls: u32,
    noticed: Vec<String>,
    memory: BTreeMap<String, String>,
    outlines: BTreeMap<String, crate::outline::Outline>,
    usage: TurnUsage,
}

impl RunState {
    /// The replies the run made before pausing.
    pub fn replies(&self) -> &[String] {
        &self.replies
    }

    pub fn steps(&self) -> u32 {
        self.steps
    }
}

/// One graph's state at a block boundary.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct FrameState {
    queues: BTreeMap<String, Vec<VecDeque<Value>>>,
    ready: VecDeque<String>,
    scheduled: BTreeMap<String, usize>,
    remembered: BTreeMap<String, Value>,
    collected: Vec<(String, Value)>,
    /// The block that was running and runs (or carries on) first: a
    /// function block whose call is the next frame, or a block that was
    /// interrupted and runs again.
    pending: Option<PendingBlock>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct PendingBlock {
    block: String,
    args: Vec<Value>,
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
    /// A block to run (or carry on) before anything else, when resuming.
    pending: Option<PendingBlock>,
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
            pending: None,
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

    /// This graph's state, to pause.
    fn save(&self, pending: Option<PendingBlock>) -> FrameState {
        FrameState {
            queues: self
                .queues
                .iter()
                .map(|(k, v)| (k.clone(), v.clone()))
                .collect(),
            ready: self.ready.clone(),
            scheduled: self
                .scheduled
                .iter()
                .filter(|(_, n)| **n > 0)
                .map(|(k, v)| (k.clone(), *v))
                .collect(),
            remembered: self
                .remembered
                .iter()
                .map(|(k, v)| (k.clone(), v.clone()))
                .collect(),
            collected: self.collected.clone(),
            pending,
        }
    }

    /// Takes up a paused state of this graph. Everything in it must fit
    /// this graph's blocks and ports exactly.
    fn restore(&mut self, s: FrameState) -> Result<(), String> {
        let bad = |what: &str| Err(format!("the paused run doesn't fit this loop ({what})"));
        if s.queues.len() != self.queues.len() {
            return bad("blocks");
        }
        for (id, qs) in &s.queues {
            match self.queues.get(id) {
                Some(mine) if mine.len() == qs.len() => {}
                _ => return bad("inputs"),
            }
        }
        let known = |id: &String| self.kinds.contains_key(id);
        if !s.ready.iter().all(known)
            || !s.scheduled.keys().all(known)
            || !s.remembered.keys().all(known)
        {
            return bad("blocks to run");
        }
        // Each block with inputs is in `ready` exactly as often as
        // `scheduled` says, and no more often than its queued inputs allow;
        // a block without inputs (they start the run) at most once.
        let mut counts: BTreeMap<&String, usize> = BTreeMap::new();
        for id in &s.ready {
            *counts.entry(id).or_default() += 1;
        }
        let inputs = |id: &String| self.in_ports.get(id).map_or(0, Vec::len);
        let fits = counts.iter().all(|(id, n)| {
            if inputs(id) == 0 {
                *n == 1 && !s.scheduled.contains_key(*id)
            } else {
                s.scheduled.get(*id) == Some(n) && s.queues[*id].iter().all(|q| q.len() >= *n)
            }
        });
        if !fits || s.scheduled.keys().any(|id| !counts.contains_key(id)) {
            return bad("schedule");
        }
        if let Some(p) = &s.pending {
            let Some(ports) = self.in_ports.get(&p.block) else {
                return bad("the block it was running");
            };
            if p.args.len() > ports.len().max(1) {
                return bad("the block it was running");
            }
        }
        self.queues = s.queues.into_iter().collect();
        self.ready = s.ready;
        self.scheduled = s.scheduled.into_iter().collect();
        self.remembered = s.remembered.into_iter().collect();
        self.collected = s.collected;
        self.pending = s.pending;
        Ok(())
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
    /// Saved with Save to memory; read as `{{name}}`.
    memory: Memory,
    /// Plan outlines, by memory name.
    outlines: HashMap<String, crate::outline::Outline>,
    usage: TurnUsage,
    /// Set to pause at the next block boundary; `None`: this run can't pause.
    pause: Option<&'a AtomicBool>,
    /// Set (with `pause`) to pause even partway through a model call (as
    /// when the program shuts down): the call is dropped, uncounted, and the
    /// block runs again on resuming. Only while nothing in the block would
    /// happen twice.
    hurry: Option<&'a AtomicBool>,
    /// A tool has run in the current block, so it can't be run again from
    /// its inputs: pausing waits for it to finish.
    acted: bool,
    /// The paused function calls still to carry on, outermost first.
    resume: VecDeque<FrameState>,
    /// The block about to run is a function block carrying on its paused
    /// call (the front of `resume`).
    resuming: bool,
}

/// Runs `l` to completion (or until stopped or limited).
pub async fn run<H: Host>(
    l: &LoopFile,
    host: &H,
    inputs: &RunInputs,
    cancel: &AtomicBool,
    on_event: impl FnMut(Event) + Send,
) -> RunOutcome {
    run_with(l, host, inputs, None, cancel, None, None, on_event).await
}

fn ended(status: RunStatus) -> RunOutcome {
    RunOutcome {
        status,
        replies: vec![],
        steps: 0,
        llm_calls: 0,
        tool_calls: 0,
        waiting: vec![],
        usage: TurnUsage::default(),
        paused: None,
    }
}

/// Runs `l`, or carries on the paused run `resume` of it, until it ends,
/// is stopped (`cancel`) or pauses (`pause`, which also lets a run pause
/// when its model can't be reached). Without `pause` the run can't pause.
/// `hurry` (with `pause`) also cuts a model call short to pause.
#[allow(clippy::too_many_arguments)]
pub async fn run_with<H: Host>(
    l: &LoopFile,
    host: &H,
    inputs: &RunInputs,
    resume: Option<RunState>,
    cancel: &AtomicBool,
    pause: Option<&AtomicBool>,
    hurry: Option<&AtomicBool>,
    mut on_event: impl FnMut(Event) + Send,
) -> RunOutcome {
    let analysis = analyze(l);
    if let Some(e) = analysis
        .issues
        .iter()
        .find(|i| i.severity == Severity::Error)
    {
        return ended(RunStatus::Failed {
            block: e.block.clone(),
            error: e.message.clone(),
        });
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
        memory: Memory::new(),
        outlines: HashMap::new(),
        usage: TurnUsage {
            tokens_added: 4 + crate::text::estimate_tokens(&inputs.message),
            ..Default::default()
        },
        hurry: pause.and(hurry),
        pause,
        acted: false,
        resume: VecDeque::new(),
        resuming: false,
    };
    let mut frame = Frame::new(&l.blocks, &l.links, String::new(), &r.analysis);
    if let Some(state) = resume
        && let Err(error) = r.take_up(&mut frame, state)
    {
        return ended(RunStatus::Failed { block: None, error });
    }
    let mut paused = None;
    let status = match r.drive(&mut frame, &mut on_event).await {
        Ok(()) | Err(Stop::Finished) => RunStatus::Completed,
        Err(Stop::Cancelled) => RunStatus::Stopped,
        Err(Stop::Limit(message)) => RunStatus::LimitReached { message },
        Err(Stop::Failed { block, error }) => RunStatus::Failed {
            block: Some(block),
            error,
        },
        Err(Stop::Paused(p)) => {
            let reason = p.reason.clone();
            paused = Some(Box::new(r.state(p.frames)));
            RunStatus::Paused { reason }
        }
    };
    RunOutcome {
        status,
        replies: r.replies,
        steps: r.steps,
        llm_calls: r.llm_calls,
        tool_calls: r.tool_calls,
        waiting: frame.waiting(),
        usage: r.usage,
        paused,
    }
}

async fn cancelled(flag: &AtomicBool) {
    while !flag.load(Ordering::Relaxed) {
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
}

/// Waits until `flag` is set; forever without one.
async fn pause_asked(flag: Option<&AtomicBool>) {
    match flag {
        Some(flag) => {
            while !flag.load(Ordering::Relaxed) {
                tokio::time::sleep(Duration::from_millis(100)).await;
            }
        }
        None => std::future::pending().await,
    }
}

/// Where a run paused: the graphs it was in, outermost first, filled in as
/// the pause passes out through each function call.
#[derive(Default)]
struct Paused {
    frames: Vec<FrameState>,
    reason: Option<String>,
}

enum Stop {
    Cancelled,
    Paused(Box<Paused>),
    /// A Final chat output ended the run: it completed.
    Finished,
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
    /// An *Ask me* wait ran out of time.
    TimedOut,
}

impl From<Stop> for BlockErr {
    fn from(s: Stop) -> Self {
        BlockErr::Stop(s)
    }
}

type BoxFut<'s, T> = Pin<Box<dyn Future<Output = T> + Send + 's>>;

impl<'a, H: Host> Runner<'a, H> {
    /// Takes up a paused run: what it had gathered, and the loop's graph
    /// (`top`) as it was. The function calls it was inside are taken up as
    /// their blocks carry on.
    fn take_up(&mut self, top: &mut Frame<'a>, state: RunState) -> Result<(), String> {
        if state.version != RUN_STATE_VERSION {
            return Err("This paused run was saved by another version of Chummy.".into());
        }
        // Functions can't call themselves, so calls nest no deeper than
        // there are functions.
        if state.frames.is_empty() || state.frames.len() > self.file.functions.len() + 1 {
            return Err("the paused run doesn't fit this loop (function calls)".into());
        }
        let mut frames: VecDeque<FrameState> = state.frames.into();
        let first = frames.pop_front().expect("checked above");
        top.restore(first)?;
        self.resume = frames;
        self.replies = state.replies;
        self.steps = state.steps;
        self.llm_calls = state.llm_calls;
        self.tool_calls = state.tool_calls;
        self.noticed = state.noticed.into_iter().collect();
        self.memory = state.memory.into_iter().collect();
        self.outlines = state.outlines.into_iter().collect();
        self.usage = state.usage;
        Ok(())
    }

    /// The run's state, to pause, given its graphs' states.
    fn state(&self, frames: Vec<FrameState>) -> RunState {
        let mut noticed: Vec<String> = self.noticed.iter().cloned().collect();
        noticed.sort();
        RunState {
            version: RUN_STATE_VERSION,
            frames,
            replies: self.replies.clone(),
            steps: self.steps,
            llm_calls: self.llm_calls,
            tool_calls: self.tool_calls,
            noticed,
            memory: self
                .memory
                .iter()
                .map(|(k, v)| (k.clone(), v.clone()))
                .collect(),
            outlines: self
                .outlines
                .iter()
                .map(|(k, v)| (k.clone(), v.clone()))
                .collect(),
            usage: self.usage.clone(),
        }
    }

    fn pause_asked(&self) -> bool {
        self.pause.is_some_and(|p| p.load(Ordering::Relaxed))
    }

    /// Whether a wait in the current block may be cut short to pause (it
    /// then runs again on resuming): only while nothing it did would happen
    /// twice.
    fn interruptible(&self) -> bool {
        self.pause.is_some() && !self.acted
    }

    fn drive<'s>(
        &'s mut self,
        frame: &'s mut Frame<'a>,
        on_event: &'s mut Emit<'_>,
    ) -> BoxFut<'s, Result<(), Stop>> {
        Box::pin(async move {
            // A resumed graph first carries on the block it was running.
            let mut pending = frame.pending.take();
            loop {
                let (id, args, resumed) = match pending.take() {
                    Some(p) => (p.block, p.args, true),
                    None => {
                        let Some(id) = frame.ready.pop_front() else {
                            break;
                        };
                        if self.cancel.load(Ordering::Relaxed) {
                            return Err(Stop::Cancelled);
                        }
                        // Between blocks: the place to pause.
                        if self.pause_asked() {
                            frame.ready.push_front(id);
                            return Err(Stop::Paused(Box::new(Paused {
                                frames: vec![frame.save(None)],
                                reason: None,
                            })));
                        }
                        if let Some(s) = frame.scheduled.get_mut(&id) {
                            *s = s.saturating_sub(1);
                        }
                        let args: Vec<Value> = frame
                            .queues
                            .get_mut(&id)
                            .map(|qs| qs.iter_mut().filter_map(|q| q.pop_front()).collect())
                            .unwrap_or_default();
                        (id, args, false)
                    }
                };

                // A block carried on after a pause was counted when it began.
                if !resumed {
                    self.steps += 1;
                    if self.steps > self.file.limits.max_steps {
                        return Err(Stop::Limit(format!(
                            "Stopped after {} steps, the loop's limit.",
                            self.file.limits.max_steps
                        )));
                    }
                }

                let path = frame.path(&id);
                let label = frame.label(&id);
                on_event(Event::BlockState {
                    block: path.clone(),
                    label: label.clone(),
                    state: BlockState::Running,
                });
                let mut step = self.step_log(frame, &id, &path, &label, &args);
                // Kept for a Failed (exchange) output, if one is linked.
                let fx_key = (id.clone(), FAILED_EXCHANGE.to_string());
                let fx_wired =
                    frame.routes.contains_key(&fx_key) || frame.exposed.contains_key(&fx_key);
                let held = match args.first() {
                    Some(Value::Exchange(ex)) if fx_wired => Some(ex.clone()),
                    _ => None,
                };
                // Kept for blocks that may pause partway (function calls,
                // and blocks that wait or call a model), to run them again.
                let kind = &frame.kinds[&id];
                let is_function = matches!(kind, BlockKind::Function(_));
                let keep = (self.pause.is_some()
                    && matches!(
                        kind,
                        BlockKind::Function(_)
                            | BlockKind::Llm(_)
                            | BlockKind::RunTool(_)
                            | BlockKind::AskUser(_)
                    ))
                .then(|| args.clone());
                let counts = (self.llm_calls, self.tool_calls, self.usage.clone());
                self.acted = false;
                self.resuming = resumed && is_function;
                let clock = std::time::Instant::now();
                let result = self.execute(frame, &id, args, on_event).await;
                self.resuming = false;
                let result = result.and_then(|o| {
                    match o.iter().find(|(_, v)| v.bytes() > MAX_VALUE_BYTES) {
                        Some((port, _)) => Err(BlockErr::Stop(Stop::Limit(format!(
                            "“{label}” made a value over {} MB (on {port}), so the run stopped. \
                             Does the loop keep adding a value to itself?",
                            MAX_VALUE_BYTES / (1024 * 1024)
                        )))),
                        None => Ok(o),
                    }
                });
                let outputs = match result {
                    Ok(o) => o,
                    Err(BlockErr::Stop(Stop::Paused(mut p))) => {
                        let Some(args) = keep else {
                            // Only blocks kept above can pause partway.
                            return Err(Stop::Failed {
                                block: path,
                                error: format!("{label}: it couldn't pause here"),
                            });
                        };
                        if p.frames.is_empty() {
                            // This block was cut short: it runs again from
                            // the same inputs, so what it counted is undone.
                            (self.llm_calls, self.tool_calls, self.usage) = counts;
                            on_event(Event::Notice {
                                block: path.clone(),
                                text: match &p.reason {
                                    Some(why) => format!(
                                        "Paused: {} This block runs again when the run is resumed.",
                                        sentence(why)
                                    ),
                                    None => {
                                        "Paused before this block finished. It runs again when the run is resumed."
                                            .into()
                                    }
                                },
                            });
                            on_event(Event::BlockState {
                                block: path.clone(),
                                label: label.clone(),
                                state: BlockState::Waiting,
                            });
                        }
                        p.frames
                            .insert(0, frame.save(Some(PendingBlock { block: id, args })));
                        return Err(Stop::Paused(p));
                    }
                    Err(BlockErr::Stop(Stop::Finished)) => {
                        // The final answer is out: done, and so is the run.
                        step.ms = clock.elapsed().as_millis() as u64;
                        on_event(Event::Step(Box::new(step)));
                        on_event(Event::BlockState {
                            block: path.clone(),
                            label: label.clone(),
                            state: BlockState::Done,
                        });
                        return Err(Stop::Finished);
                    }
                    Err(BlockErr::Stop(s)) => {
                        step.status = StepStatus::Stopped;
                        step.error = Some(match &s {
                            Stop::Finished | Stop::Paused(_) => String::new(),
                            Stop::Cancelled => "The run was stopped.".into(),
                            Stop::Limit(m) => m.clone(),
                            Stop::Failed { error, .. } => error.clone(),
                        });
                        step.ms = clock.elapsed().as_millis() as u64;
                        on_event(Event::Step(Box::new(step)));
                        return Err(s);
                    }
                    Err(e @ (BlockErr::Failed(_) | BlockErr::TimedOut)) => {
                        let e = match e {
                            BlockErr::Failed(e) => e,
                            _ => "it ran out of time".to_string(),
                        };
                        // A linked Failed output takes the error instead of
                        // stopping the run.
                        let key = (id.clone(), FAILED.to_string());
                        let caught = frame.kinds[&id].can_fail_softly()
                            && (frame.routes.contains_key(&key)
                                || frame.exposed.contains_key(&key)
                                || fx_wired);
                        step.ms = clock.elapsed().as_millis() as u64;
                        step.error = Some(e.clone());
                        if caught {
                            step.status = StepStatus::Caught;
                            step.outputs.push(PortLog {
                                port: FAILED.into(),
                                value: logged(&Value::Text(e.clone()), LOG_VALUE_CHARS),
                            });
                            on_event(Event::Step(Box::new(step)));
                            on_event(Event::Notice {
                                block: path.clone(),
                                text: format!("Failed, handled by the loop: {e}"),
                            });
                            on_event(Event::BlockState {
                                block: path.clone(),
                                label: label.clone(),
                                state: BlockState::Done,
                            });
                            let value = Value::Text(e);
                            on_event(Event::BlockOutput {
                                block: path.clone(),
                                port: FAILED.into(),
                                preview: value.preview(),
                            });
                            if let Some(names) = frame.exposed.get(&key) {
                                for n in names.clone() {
                                    frame.collected.push((n, value.clone()));
                                }
                            }
                            for (to, idx) in frame.routes.get(&key).cloned().unwrap_or_default() {
                                frame.queues.get_mut(&to).unwrap()[idx].push_back(value.clone());
                                frame.schedule(&to);
                            }
                            // The exchange it was given, with the failure as
                            // its last message (only when it was given one).
                            if let (Some(mut ex), Value::Text(e)) = (held, &value) {
                                ex.messages
                                    .push(ExMessage::new(Role::User, failure_message(e)));
                                let value = Value::Exchange(ex);
                                on_event(Event::BlockOutput {
                                    block: path.clone(),
                                    port: FAILED_EXCHANGE.into(),
                                    preview: value.preview(),
                                });
                                if let Some(names) = frame.exposed.get(&fx_key) {
                                    for n in names.clone() {
                                        frame.collected.push((n, value.clone()));
                                    }
                                }
                                for (to, idx) in
                                    frame.routes.get(&fx_key).cloned().unwrap_or_default()
                                {
                                    frame.queues.get_mut(&to).unwrap()[idx]
                                        .push_back(value.clone());
                                    frame.schedule(&to);
                                }
                            }
                            continue;
                        }
                        step.status = StepStatus::Failed;
                        on_event(Event::Step(Box::new(step)));
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
                step.ms = clock.elapsed().as_millis() as u64;
                step.outputs = outputs
                    .iter()
                    .map(|(port, v)| PortLog {
                        port: port.clone(),
                        value: logged(v, LOG_VALUE_CHARS),
                    })
                    .collect();
                on_event(Event::Step(Box::new(step)));

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

    /// The start of a step's log: which block, and what went in.
    fn step_log(
        &self,
        frame: &Frame<'a>,
        id: &str,
        path: &str,
        label: &str,
        args: &[Value],
    ) -> StepLog {
        let kind = &frame.kinds[id];
        let function = match kind {
            BlockKind::Function(p) => self
                .file
                .functions
                .iter()
                .find(|f| f.id == p.function)
                .map(|f| f.name.clone()),
            _ => None,
        };
        let names = frame.in_ports.get(id).cloned().unwrap_or_default();
        StepLog {
            n: self.steps,
            block: path.to_string(),
            label: label.to_string(),
            kind: kind.type_name().to_string(),
            function,
            inputs: names
                .into_iter()
                .zip(args)
                .map(|(port, v)| PortLog {
                    port,
                    value: logged(v, LOG_VALUE_CHARS),
                })
                .collect(),
            outputs: vec![],
            status: StepStatus::Done,
            error: None,
            started_at: now_ms(),
            ms: 0,
        }
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
            BlockKind::Final(_) => {
                let text = arg(0).as_text();
                self.replies.push(text.clone());
                on_event(Event::Reply { block: path, text });
                Err(BlockErr::Stop(Stop::Finished))
            }
            BlockKind::Pane(_) => {
                let text = pane_line(&arg(0).as_text());
                if !text.is_empty() {
                    on_event(Event::Pane { block: path, text });
                }
                Ok(vec![])
            }
            BlockKind::Stop(_) => {
                // No Failed output, so this always ends the run.
                let reason = arg(0).as_text();
                let reason = reason.trim();
                Err(BlockErr::Failed(if reason.is_empty() {
                    "the loop stopped here".into()
                } else {
                    reason.to_string()
                }))
            }
            BlockKind::Llm(p) => {
                // Tools are only offered when the Tool output goes somewhere, so a
                // block that inherits the agent's tools without a Run tool after it
                // just answers.
                let key = (id.to_string(), "tool".to_string());
                let wired = frame.routes.get(&key).is_some_and(|r| !r.is_empty())
                    || frame.exposed.get(&key).is_some_and(|r| !r.is_empty());
                self.model_block(&path, &p, wired, arg(0), on_event).await
            }
            BlockKind::RunTool(p) => {
                let Value::Exchange(mut ex) = arg(0) else {
                    return Err(BlockErr::Failed(
                        "it needs an exchange from a model block's Tool call output".into(),
                    ));
                };
                let calls = ex.pending_calls();
                let ctx_window = self.host.context(None).window;
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
                    let refused = if p.confirm || self.host.must_confirm(&call.name, call.network) {
                        let ask = Ask {
                            block: path.clone(),
                            label: frame.label(id),
                            kind: AskKind::ToolApproval {
                                tool: call.name.clone(),
                                arguments: call.arguments.clone(),
                            },
                            text: describe_call(&call.name, &call.arguments),
                        };
                        match self.wait_for(ask, None, on_event).await? {
                            Answer::Allow => None,
                            Answer::Deny(why) => Some(why),
                            Answer::Text(t) => Some(t),
                        }
                    } else {
                        None
                    };
                    let result = if let Some(why) = refused {
                        let why = why.trim();
                        Ok(ToolOutput {
                            text: if why.is_empty() {
                                "Error: the user didn't allow this tool call.".into()
                            } else {
                                format!("Error: the user didn't allow this tool call: {why}")
                            },
                            is_error: true,
                        })
                    } else {
                        // From here the block can't simply run again.
                        self.acted = true;
                        let fut =
                            self.host
                                .run_tool(&call.name, call.arguments.clone(), call.network);
                        tokio::select! {
                            r = fut => r,
                            _ = cancelled(self.cancel) => return Err(Stop::Cancelled.into()),
                        }
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
                    let text = truncate(&out.text, result_cap(p.max_result_chars, ctx_window));
                    self.usage.tokens_added += 4 + crate::text::estimate_tokens(&text);
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
            BlockKind::AskUser(p) => {
                let text = fill_template(&p.prompt, Some(&arg(0).as_text()), &self.memory);
                // A blank question (often a thinking model that only thought) is never
                // put to the person: the block fails, so its Error path is taken.
                if text.trim().is_empty() {
                    return Err(BlockErr::Failed(
                        "there was no question to ask (the text was empty)".into(),
                    ));
                }
                let ask = Ask {
                    block: path.clone(),
                    label: frame.label(id),
                    kind: AskKind::Question,
                    text,
                };
                let limit = (p.timeout_minutes > 0)
                    .then(|| Duration::from_secs(u64::from(p.timeout_minutes) * 60));
                match self.wait_for(ask, limit, on_event).await {
                    Ok(Answer::Text(t)) => one(Value::Text(t)),
                    Ok(Answer::Allow) => one(Value::Text("yes".into())),
                    Ok(Answer::Deny(t)) => {
                        one(Value::Text(if t.is_empty() { "no".into() } else { t }))
                    }
                    Err(BlockErr::TimedOut) if !p.default_answer.is_empty() => {
                        on_event(Event::Notice {
                            block: path,
                            text: "No answer in time; using the default answer.".into(),
                        });
                        one(Value::Text(p.default_answer))
                    }
                    Err(BlockErr::TimedOut) => Err(BlockErr::Failed(format!(
                        "nobody answered within {} minute{}",
                        p.timeout_minutes,
                        if p.timeout_minutes == 1 { "" } else { "s" }
                    ))),
                    Err(e) => Err(e),
                }
            }
            BlockKind::DropLast(p) => {
                let Value::Exchange(mut ex) = arg(0) else {
                    return Err(BlockErr::Failed("it needs an exchange".into()));
                };
                // Never the system message at the start.
                let keep_from =
                    usize::from(ex.messages.first().is_some_and(|m| m.role == Role::System));
                let n = (p.count as usize).min(ex.messages.len() - keep_from);
                ex.messages.truncate(ex.messages.len() - n);
                Ok(vec![("out".into(), Value::Exchange(ex))])
            }
            BlockKind::DeclineCalls(_) => {
                let Value::Exchange(mut ex) = arg(0) else {
                    return Err(BlockErr::Failed(
                        "it needs an exchange from a model block's Tool call output".into(),
                    ));
                };
                let reason = arg(1).as_text();
                let reason = reason.trim();
                for call in ex.pending_calls() {
                    ex.messages.push(ExMessage {
                        call_id: Some(call.id.clone()),
                        name: Some(call.name.clone()),
                        ..ExMessage::new(
                            Role::Tool,
                            if reason.is_empty() {
                                "Not run.".to_string()
                            } else {
                                format!("Not run: {reason}")
                            },
                        )
                    });
                }
                one(Value::Exchange(ex))
            }
            BlockKind::FindCall(p) => {
                let Value::Exchange(mut ex) = arg(0) else {
                    return Err(BlockErr::Failed(
                        "the Exchange input needs an exchange".into(),
                    ));
                };
                let text = arg(1).as_text();
                let (rest, found, bad) = prompted::find_calls(&text);
                let mut calls = Vec::new();
                let mut problems: Vec<String> = bad
                    .iter()
                    .map(|b| format!("a tool call couldn't be read ({})", b.error))
                    .collect();
                let usable = self.block_tools(&path, &p.tools, on_event);
                for f in found {
                    // Like a model block: only the tools its switches allow.
                    match usable.iter().find(|t| t.name == f.name) {
                        None => problems.push(format!("there's no tool called “{}” here", f.name)),
                        Some(t) => match check_arguments(&t.parameters, &f.arguments) {
                            Ok(args) => calls.push(ToolCall {
                                id: format!("call_{}_{}", self.steps, calls.len() + 1),
                                name: f.name,
                                arguments: args,
                                network: p.network,
                            }),
                            Err(e) => problems.push(format!("in the call to {}, {e}", f.name)),
                        },
                    }
                }
                if calls.is_empty() {
                    if !problems.is_empty() {
                        on_event(Event::Notice {
                            block: path,
                            text: format!("No usable tool call: {}.", problems.join("; ")),
                        });
                    }
                    return Ok(vec![("none".into(), Value::Text(text))]);
                }
                ex.messages.push(ExMessage {
                    calls,
                    ..ExMessage::new(Role::Assistant, rest)
                });
                Ok(vec![("call".into(), Value::Exchange(ex))])
            }
            BlockKind::Store(p) => {
                let key = p.key.trim();
                if !valid_memory_key(key) {
                    return Err(BlockErr::Failed(format!(
                        "“{key}” isn't a valid memory name"
                    )));
                }
                let text = arg(0).as_text();
                let max = p.max_chars.max(1) as usize;
                let n = text.chars().count();
                let kept = if n > max {
                    text.chars().skip(n - max).collect()
                } else {
                    text.clone()
                };
                self.memory.insert(key.to_string(), kept);
                one(Value::Text(text))
            }
            BlockKind::Outline(p) => {
                use crate::blocks::OutlineAction;
                let key = p.key.trim().to_string();
                let o = self.outlines.entry(key.clone()).or_default();
                match p.action {
                    OutlineAction::Start => o.start(&arg(0).as_text()),
                    OutlineAction::AddSteps => {
                        let mut steps = arg(1).as_list();
                        if p.limit > 0 {
                            steps.truncate(p.limit as usize);
                        }
                        o.add_steps(&arg(0).as_text(), &steps);
                    }
                    OutlineAction::Mark => {
                        if !o.mark(&arg(0).as_text(), p.status) {
                            on_event(Event::Notice {
                                block: path.clone(),
                                text: "That task isn't in the outline (or is already finished)."
                                    .into(),
                            });
                        }
                    }
                }
                let text = o.render();
                self.memory.insert(key, text.clone());
                one(Value::Text(text))
            }
            BlockKind::Recall(p) => one(Value::Text(
                self.memory.get(p.key.trim()).cloned().unwrap_or_default(),
            )),
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
                let memory = &self.memory;
                let fmt = |t: &str| fill_template(&p.format, Some(t), memory);
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
                let newest = ex
                    .messages
                    .iter()
                    .rev()
                    .find(|m| p.role.matches(m.role))
                    .map(|m| m.text.clone())
                    .unwrap_or_default();
                let calls = ex
                    .pending_calls()
                    .iter()
                    .map(|c| describe_call(&c.name, &c.arguments))
                    .collect::<Vec<_>>()
                    .join("\n\n");
                Ok(vec![
                    ("last".into(), Value::Text(last)),
                    (
                        "all".into(),
                        Value::Text(if p.system_in_transcript {
                            ex.transcript()
                        } else {
                            Exchange {
                                messages: ex
                                    .messages
                                    .iter()
                                    .filter(|m| m.role != Role::System)
                                    .cloned()
                                    .collect(),
                            }
                            .transcript()
                        }),
                    ),
                    ("calls".into(), Value::Text(calls)),
                    ("newest".into(), Value::Text(newest)),
                ])
            }
            BlockKind::ContextCheck(p) => {
                let Value::Exchange(ex) = arg(0) else {
                    return Err(BlockErr::Failed("it needs an exchange".into()));
                };
                // System messages are filled in at each model call, so count
                // them as they'd be sent now.
                let sent: Vec<ExMessage> = ex
                    .messages
                    .iter()
                    .map(|m| {
                        if m.role == Role::System {
                            ExMessage::new(Role::System, fill_template(&m.text, None, &self.memory))
                        } else {
                            m.clone()
                        }
                    })
                    .collect();
                let tokens = crate::exchange::estimate_messages(&sent);
                let model = p.model.as_deref().filter(|m| !m.is_empty());
                let limits = self.host.context(model);
                let at = match p.percent {
                    Some(pc) => u64::from(limits.window) * u64::from(pc.clamp(1, 100)) / 100,
                    None => u64::from(limits.compact_at),
                };
                if tokens >= at {
                    on_event(Event::Notice {
                        block: path,
                        text: format!(
                            "The exchange is about {tokens} tokens, past {at} of the model's {}.",
                            limits.window
                        ),
                    });
                    Ok(vec![("full".into(), Value::Exchange(ex))])
                } else {
                    Ok(vec![("room".into(), Value::Exchange(ex))])
                }
            }
            BlockKind::Function(p) => {
                self.function_block(frame, id, &p.function, args, on_event)
                    .await
            }
            BlockKind::Combine(p) => {
                // Memory first, then the inputs, so neither rewrites the other.
                let values: Vec<String> = args.iter().map(Value::as_text).collect();
                one(Value::Text(crate::text::fill_numbered_with(
                    &p.format,
                    &values,
                    &self.memory,
                )))
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

    /// Waits for, asks about, or stops at a model's daily token limit, as
    /// the block says.
    async fn check_budget(
        &mut self,
        path: &str,
        model: Option<&str>,
        on_limit: OnLimit,
        on_event: &mut Emit<'_>,
    ) -> Result<(), BlockErr> {
        loop {
            let Budget::Exhausted {
                model: name,
                used,
                limit,
                resets_in,
                extra,
            } = self.host.budget(model)
            else {
                return Ok(());
            };
            let what = format!("{name} has used its {limit} tokens for today");
            match on_limit {
                OnLimit::Stop => {
                    return Err(Stop::Limit(format!(
                        "Stopped: {what}. Raise its limit in Settings → Models."
                    ))
                    .into());
                }
                OnLimit::Wait => {
                    let text = format!("{what}; waiting until it refreshes.");
                    if self.noticed.insert(text.clone()) {
                        on_event(Event::Notice {
                            block: path.to_string(),
                            text,
                        });
                    }
                    // Check again at least every minute (the limit may be raised).
                    let nap = resets_in.clamp(Duration::from_secs(1), Duration::from_secs(60));
                    let can_pause = self.interruptible();
                    tokio::select! {
                        _ = tokio::time::sleep(nap) => {}
                        _ = cancelled(self.cancel) => return Err(Stop::Cancelled.into()),
                        _ = pause_asked(self.pause), if can_pause => return Err(paused_here(None)),
                    }
                }
                OnLimit::Ask => {
                    let ask = Ask {
                        block: path.to_string(),
                        label: "Daily token limit".into(),
                        kind: AskKind::Budget {
                            model: name.clone(),
                            used,
                            limit,
                            extra,
                        },
                        text: format!("{what} ({used} used). Allow {extra} more tokens today?"),
                    };
                    match self.wait_for(ask, None, on_event).await? {
                        Answer::Allow => self.host.grant(model, extra),
                        _ => {
                            return Err(Stop::Limit(format!(
                                "Stopped: {what}, and more wasn't allowed."
                            ))
                            .into());
                        }
                    }
                }
            }
        }
    }

    /// Asks the user and waits, until answered, stopped or `limit` passes.
    async fn wait_for(
        &mut self,
        ask: Ask,
        limit: Option<Duration>,
        on_event: &mut Emit<'_>,
    ) -> Result<Answer, BlockErr> {
        on_event(Event::Asked {
            block: ask.block.clone(),
            text: ask.text.clone(),
        });
        let block = ask.block.clone();
        let fut = self.host.ask(ask);
        let timeout = async {
            match limit {
                Some(d) => tokio::time::sleep(d).await,
                None => std::future::pending().await,
            }
        };
        let can_pause = self.interruptible();
        let r = tokio::select! {
            // Stopping a run also withdraws its question, which can arrive
            // first; it's still a stop, not a failure.
            r = fut => r.map_err(|e| {
                if self.cancel.load(Ordering::Relaxed) {
                    BlockErr::Stop(Stop::Cancelled)
                } else {
                    BlockErr::Failed(e)
                }
            }),
            _ = cancelled(self.cancel) => Err(Stop::Cancelled.into()),
            _ = timeout => Err(BlockErr::TimedOut),
            // Nothing's been done that can't be done again: pause now, and
            // ask again on resuming.
            _ = pause_asked(self.pause), if can_pause => Err(paused_here(None)),
        };
        on_event(Event::Answered { block });
        r
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
        // Carrying on a call that paused: its graph as it was.
        if std::mem::take(&mut self.resuming) {
            let state = self.resume.pop_front().ok_or_else(|| {
                BlockErr::Failed("the paused run doesn't fit this loop (function calls)".into())
            })?;
            sub.restore(state).map_err(BlockErr::Failed)?;
            self.drive(&mut sub, on_event).await?;
            return Ok(sub.collected);
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

    /// The tools a block may use: each tool's switch (inherit: the agent's
    /// switch; on: even if the agent's is off; off: never), always within
    /// what the app allows.
    fn block_tools(
        &mut self,
        path: &str,
        switches: &ToolSwitches,
        on_event: &mut Emit<'_>,
    ) -> Vec<ToolInfo> {
        let mut names = self.host.tool_names();
        for n in switches.each.keys() {
            if !names.contains(n) {
                names.push(n.clone());
            }
        }
        let mut out = Vec::new();
        let mut notes = Vec::new();
        for name in names {
            let s = switches.switch_for(&name);
            if s == Switch::Off {
                continue;
            }
            let Some(info) = self.host.tool_info(&name) else {
                if switches.each.contains_key(&name) {
                    notes.push(format!(
                        "The tool “{name}” is off in Chummy (Security and Privacy or the agent's tools), so the model wasn't offered it."
                    ));
                }
                continue;
            };
            if !s.resolve(self.host.agent_allows(&name)) {
                continue;
            }
            out.push(info);
        }
        for text in notes {
            if self.noticed.insert(format!("{path}\u{0}{text}")) {
                on_event(Event::Notice {
                    block: path.to_string(),
                    text,
                });
            }
        }
        out
    }

    async fn model_block(
        &mut self,
        path: &str,
        p: &LlmParams,
        tool_wired: bool,
        input: Value,
        on_event: &mut Emit<'_>,
    ) -> Result<Outputs, BlockErr> {
        let tools = if tool_wired || p.tool_mode == ToolMode::Off {
            self.block_tools(path, &p.tools, on_event)
        } else {
            Vec::new()
        };
        // `_TOOLS_` in the prompt or system message lists the block's tools,
        // and `{{name}}` reads memory.
        let listed = prompted::tool_list(&tools);
        let system = fill_template(&p.system.replace("_TOOLS_", &listed), None, &self.memory);
        let system = system.trim();
        let use_tools = !tools.is_empty() && p.tool_mode != ToolMode::Off;
        let prompted_mode = use_tools && p.tool_mode == ToolMode::Prompted;

        // The exchange this block continues, and what it sends the model.
        let continues = matches!(input, Value::Exchange(_));
        let (base, mut messages) = match input {
            Value::Exchange(ex) => {
                let mut msgs = ex.messages.clone();
                if !system.is_empty() {
                    msgs.retain(|m| m.role != Role::System);
                    msgs.insert(0, ExMessage::new(Role::System, system));
                } else {
                    // An exchange's own system message can read memory too,
                    // as it is at the time of each call.
                    for m in msgs.iter_mut().filter(|m| m.role == Role::System) {
                        m.text = fill_template(&m.text, None, &self.memory);
                    }
                }
                (ex, msgs)
            }
            other => {
                let mut ex = Exchange::default();
                if !system.is_empty() {
                    ex.messages.push(ExMessage::new(Role::System, system));
                }
                let prompt = p.prompt.replace("_TOOLS_", &listed);
                ex.messages.push(ExMessage::new(
                    Role::User,
                    fill_template(&prompt, Some(&other.as_text()), &self.memory),
                ));
                let msgs = ex.messages.clone();
                (ex, msgs)
            }
        };
        // A reminder goes last, for this call only: not part of `base`.
        if continues && !p.reminder.trim().is_empty() {
            let note = fill_template(&p.reminder, None, &self.memory);
            if !note.trim().is_empty() {
                messages.push(ExMessage::new(Role::User, note.trim()));
            }
        }
        if prompted_mode {
            messages = prompted::plain_history(&messages);
            let section = prompted::tool_section(&tools);
            match messages.iter_mut().find(|m| m.role == Role::System) {
                Some(sys) => sys.text = format!("{}\n\n{section}", sys.text),
                None => messages.insert(0, ExMessage::new(Role::System, section)),
            }
        }
        let offered = if use_tools && !prompted_mode {
            tools.clone()
        } else {
            vec![]
        };
        let model = p.model.clone().filter(|m| !m.is_empty());

        let mut attempt = 0;
        loop {
            self.check_budget(path, model.as_deref(), p.on_limit, on_event)
                .await?;
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
                tools: offered.clone(),
                tool_choice: p.tool_choice,
                temperature: p.temperature,
                max_tokens: p.max_tokens,
                generation: p.generation.clone(),
                model: model.clone(),
            };
            let sent = Value::Exchange(Exchange {
                messages: call.messages.clone(),
            });
            let offered_names: Vec<String> = if use_tools {
                tools.iter().map(|t| t.name.clone()).collect()
            } else {
                vec![]
            };
            let started_at = now_ms();
            let clock = std::time::Instant::now();
            let mut answer = self.ask(path, call, on_event).await?;
            if prompted_mode {
                let (rest, found, bad) = prompted::find_calls(&strip_thinking(&answer.text));
                let inline = thinking(&answer.text);
                answer.text = if inline.is_empty() {
                    rest
                } else {
                    format!("<think>{inline}</think>{rest}")
                };
                answer.calls.extend(found.into_iter().map(|f| ToolCall {
                    id: String::new(),
                    name: f.name,
                    arguments: f.arguments,
                    network: Switch::Off,
                }));
                answer
                    .malformed
                    .extend(bad.into_iter().map(|b| MalformedCall {
                        name: if b.name.is_empty() {
                            "a tool".into()
                        } else {
                            b.name
                        },
                        raw: b.raw,
                        error: b.error,
                    }));
            }
            let mut reasoning = answer.reasoning.trim().to_string();
            let inline = thinking(&answer.text);
            if !inline.is_empty() {
                if !reasoning.is_empty() {
                    reasoning.push_str("\n\n");
                }
                reasoning.push_str(&inline);
            }
            let text = strip_thinking(&answer.text);
            let answered = crate::exchange::estimate_messages(&[ExMessage {
                calls: answer.calls.clone(),
                ..ExMessage::new(Role::Assistant, text.clone())
            }]);
            self.usage.tokens_added += answered;
            self.usage.context_tokens = crate::exchange::estimate_messages(&messages) + answered;
            self.usage.context_window = self.host.context(model.as_deref()).window;
            self.usage.model = model.clone();
            on_event(Event::ModelCall(Box::new(ModelLog {
                block: path.to_string(),
                attempt: attempt + 1,
                model: model.clone(),
                tool_mode: match p.tool_mode {
                    ToolMode::Native => "native",
                    ToolMode::Prompted => "prompted",
                    ToolMode::Off => "off",
                }
                .into(),
                tools: offered_names,
                sent: logged(&sent, LOG_VALUE_CHARS * 2),
                answer: logged(&Value::Text(text.clone()), LOG_VALUE_CHARS),
                reasoning: crate::log::cut(&reasoning, LOG_VALUE_CHARS).0,
                calls: answer
                    .calls
                    .iter()
                    .map(|c| format!("{}({})", c.name, c.arguments))
                    .chain(
                        answer
                            .malformed
                            .iter()
                            .map(|m| format!("{} (unreadable: {})", m.name, m.error)),
                    )
                    .collect(),
                started_at,
                ms: clock.elapsed().as_millis() as u64,
            })));

            if !use_tools {
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
                        Ok(args) => {
                            // Ids must be unique within the exchange; some
                            // servers send empty or repeated ones.
                            let taken = |id: &str| {
                                id.is_empty()
                                    || good.iter().any(|g: &ToolCall| g.id == id)
                                    || base
                                        .messages
                                        .iter()
                                        .any(|m| m.calls.iter().any(|x| x.id == id))
                            };
                            let mut id = c.id.clone();
                            let mut n = good.len() + 1;
                            while taken(&id) {
                                id = format!("call_{}_{n}", self.tool_calls + self.llm_calls);
                                n += 1;
                            }
                            good.push(ToolCall {
                                id,
                                arguments: args,
                                network: p.network,
                                name: c.name.clone(),
                            })
                        }
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
        // Nothing in this block has happened that can't happen again: a
        // hurried pause may drop the call (it's asked again on resuming).
        let can_hurry = self.interruptible();
        let answer = loop {
            tokio::select! {
                r = &mut fut => break r,
                Some(t) = rx.recv() => on_event(Event::LlmText { block: block.clone(), text: t }),
                _ = cancelled(self.cancel) => return Err(Stop::Cancelled.into()),
                _ = pause_asked(self.hurry), if can_hurry => return Err(paused_here(None)),
            }
        };
        while let Ok(t) = rx.try_recv() {
            on_event(Event::LlmText {
                block: block.clone(),
                text: t,
            });
        }
        answer.map_err(|f| {
            // A model that can't be reached pauses a run that can pause (it
            // asks again on resuming) instead of ending it.
            if f.unavailable && self.pause.is_some() {
                paused_here(Some(f.message))
            } else {
                BlockErr::Failed(f.message)
            }
        })
    }
}

/// `text` as a sentence: trimmed, with a full stop unless it already ends
/// with one (or a question or exclamation mark).
pub fn sentence(text: &str) -> String {
    let t = text.trim();
    if t.is_empty() || t.ends_with(['.', '?', '!']) {
        t.to_string()
    } else {
        format!("{t}.")
    }
}

/// Pausing partway through the current block, which runs again on resuming.
fn paused_here(reason: Option<String>) -> BlockErr {
    BlockErr::Stop(Stop::Paused(Box::new(Paused {
        frames: vec![],
        reason,
    })))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::format::{Block, Link, PortRef, Position};
    use crate::log::{StepLog, StepStatus};
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
        asks: Mutex<Vec<Ask>>,
        answers_for_user: Mutex<VecDeque<Answer>>,
        never_answer: bool,
        confirm_tools: Vec<String>,
        /// Set when asked, like a user stopping the run (which withdraws
        /// the question).
        stop_on_ask: Option<std::sync::Arc<AtomicBool>>,
        /// The model's daily tokens are used up until granted more.
        exhausted: Mutex<bool>,
        granted: Mutex<Vec<u64>>,
        /// The next this-many model calls fail, like a server that's down.
        fail_next: Mutex<u32>,
        /// The next this-many model calls can't reach the model at all.
        unreachable_next: Mutex<u32>,
        online_runs: Mutex<Vec<Switch>>,
        /// Set after this many model calls (0: never), like a person
        /// pressing Pause while the run works.
        pause_after_call: usize,
        /// Set when the this-many'th question or approval is asked (0:
        /// never), which is then answered only after a moment.
        pause_at_ask: usize,
        pause: std::sync::Arc<AtomicBool>,
        /// The this-many'th model call (0: none) never answers, and asks for
        /// a hurried pause, like a program shutting down mid-call.
        hang_on_call: usize,
        hurry: std::sync::Arc<AtomicBool>,
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
                network: Switch::Off,
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
        ) -> Result<LlmAnswer, LlmFailure> {
            let prompt = call
                .messages
                .last()
                .map(|m| m.text.clone())
                .unwrap_or_default();
            self.prompts.lock().unwrap().push(prompt.clone());
            self.calls.lock().unwrap().push(call);
            {
                let mut f = self.fail_next.lock().unwrap();
                if *f > 0 {
                    *f -= 1;
                    return Err("the model server is down".into());
                }
                let mut u = self.unreachable_next.lock().unwrap();
                if *u > 0 {
                    *u -= 1;
                    return Err(LlmFailure::unavailable("couldn't reach the model server"));
                }
            }
            if self.pause_after_call == self.calls.lock().unwrap().len() {
                self.pause.store(true, Ordering::Relaxed);
            }
            if self.hang_on_call == self.calls.lock().unwrap().len() {
                self.pause.store(true, Ordering::Relaxed);
                self.hurry.store(true, Ordering::Relaxed);
                std::future::pending::<()>().await;
            }
            let a = self
                .answers
                .lock()
                .unwrap()
                .pop_front()
                .unwrap_or_else(|| text(&format!("echo: {prompt}")));
            on_text(&a.text);
            Ok(a)
        }
        fn tool_names(&self) -> Vec<String> {
            vec!["echo".into()]
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
        async fn run_tool(
            &self,
            name: &str,
            arguments: Json,
            online: Switch,
        ) -> Result<ToolOutput, String> {
            self.online_runs.lock().unwrap().push(online);
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
        async fn ask(&self, ask: Ask) -> Result<Answer, String> {
            self.asks.lock().unwrap().push(ask);
            if self.pause_at_ask > 0 {
                if self.asks.lock().unwrap().len() == self.pause_at_ask {
                    self.pause.store(true, Ordering::Relaxed);
                }
                // The person takes a moment to answer.
                tokio::time::sleep(Duration::from_millis(400)).await;
            }
            if let Some(stop) = &self.stop_on_ask {
                stop.store(true, Ordering::Relaxed);
                return Err("The question was withdrawn.".into());
            }
            if self.never_answer {
                std::future::pending::<()>().await;
            }
            self.answers_for_user
                .lock()
                .unwrap()
                .pop_front()
                .ok_or_else(|| "nobody to ask".to_string())
        }
        fn must_confirm(&self, tool: &str, _online: Switch) -> bool {
            self.confirm_tools.iter().any(|t| t == tool)
        }
        fn budget(&self, _model: Option<&str>) -> Budget {
            if *self.exhausted.lock().unwrap() {
                Budget::Exhausted {
                    model: "Fake".into(),
                    used: 100,
                    limit: 100,
                    resets_in: Duration::from_secs(3600),
                    extra: 25,
                }
            } else {
                Budget::Ok
            }
        }
        fn grant(&self, _model: Option<&str>, tokens: u64) {
            self.granted.lock().unwrap().push(tokens);
            *self.exhausted.lock().unwrap() = false;
        }
    }

    fn b(id: &str, kind: &str, params: serde_json::Value) -> Block {
        Block {
            id: id.into(),
            kind: kind.into(),
            label: id.into(),
            position: Position::default(),
            params,
            comment: String::new(),
        }
    }
    fn ln(a: &str, ap: &str, c: &str, cp: &str) -> Link {
        Link::new((a, ap), (c, cp))
    }
    fn lp(blocks: Vec<Block>, links: Vec<Link>) -> LoopFile {
        let mut l = LoopFile::new("t");
        l.blocks = blocks;
        l.links = links;
        l
    }
    async fn go(l: &LoopFile, host: &impl Host, msg: &str) -> (RunOutcome, Vec<Event>) {
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
    async fn each_model_block_sends_its_own_generation_settings() {
        let l = lp(
            vec![
                b("in", "input", json!({})),
                b(
                    "think",
                    "llm",
                    json!({ "prompt": "_INPUT_", "generation": { "reasoning_effort": "high" } }),
                ),
                b("plain", "llm", json!({ "prompt": "_INPUT_" })),
                b("out", "output", json!({})),
            ],
            vec![
                ln("in", "out", "think", "in"),
                ln("think", "out", "plain", "in"),
                ln("plain", "out", "out", "in"),
            ],
        );
        let host = Fake::with(&["a", "b"]);
        let (o, _) = go(&l, &host, "hi").await;
        assert_eq!(o.status, RunStatus::Completed);
        let calls = host.calls.lock().unwrap();
        assert_eq!(
            calls[0].generation.reasoning_effort,
            Some(Some(crate::Effort::High))
        );
        assert!(
            calls[1].generation.is_empty(),
            "the other block inherits everything"
        );
    }

    #[tokio::test]
    async fn failed_exchange_carries_the_failure_and_drop_last_takes_it_back_out() {
        let l = lp(
            vec![
                b("in", "input", json!({})),
                b("start", "exchange", json!({ "system": "S" })),
                b("ask", "llm", json!({})),
                b("ok", "output", json!({})),
                b("said", "read_exchange", json!({ "role": "user" })),
                b("show_fail", "output", json!({})),
                b("drop", "drop_last", json!({})),
                b("again", "read_exchange", json!({ "role": "any" })),
                b("show_back", "output", json!({})),
            ],
            vec![
                ln("in", "out", "start", "in"),
                ln("start", "out", "ask", "in"),
                ln("ask", "out", "ok", "in"),
                ln("ask", "failed_exchange", "said", "in"),
                ln("said", "last", "show_fail", "in"),
                ln("ask", "failed_exchange", "drop", "in"),
                ln("drop", "out", "again", "in"),
                ln("again", "all", "show_back", "in"),
            ],
        );
        let host = Fake::default();
        *host.fail_next.lock().unwrap() = 1;
        let (o, _) = go(&l, &host, "hello").await;
        // Caught (only Failed (exchange) is linked), not a failed run.
        assert_eq!(o.status, RunStatus::Completed, "{o:?}");
        assert_eq!(
            o.replies[0],
            "(The last step failed: the model server is down)"
        );
        // Dropped again: back to what the model was given.
        assert!(o.replies[1].contains("hello"), "{:?}", o.replies);
        assert!(!o.replies[1].contains("failed"), "{:?}", o.replies);
        // A text input has no exchange to carry: nothing goes out there.
        let l2 = lp(
            vec![
                b("in", "input", json!({})),
                b("ask", "llm", json!({})),
                b("o", "output", json!({})),
                b("drop", "drop_last", json!({})),
                b("read", "read_exchange", json!({ "role": "any" })),
                b("o2", "output", json!({})),
            ],
            vec![
                ln("in", "out", "ask", "in"),
                ln("ask", "out", "o", "in"),
                ln("ask", "failed_exchange", "drop", "in"),
                ln("drop", "out", "read", "in"),
                ln("read", "all", "o2", "in"),
            ],
        );
        *host.fail_next.lock().unwrap() = 1;
        let (o, _) = go(&l2, &host, "hi").await;
        assert_eq!(o.status, RunStatus::Completed, "{o:?}");
        assert!(o.replies.is_empty());
    }

    #[tokio::test]
    async fn final_chat_output_ends_the_run_as_completed() {
        // "Along the way" goes out first; the final answer ends the run, so
        // the later model call never happens.
        let l = lp(
            vec![
                b("in", "input", json!({})),
                b(
                    "say",
                    "combine",
                    json!({ "inputs": 1, "format": "Along the way" }),
                ),
                b("add", "output", json!({})),
                b(
                    "done",
                    "combine",
                    json!({ "inputs": 1, "format": "The answer" }),
                ),
                b("final", "final", json!({})),
                b("later", "llm", json!({})),
                b("o", "output", json!({})),
            ],
            vec![
                ln("in", "out", "say", "in1"),
                ln("say", "out", "add", "in"),
                ln("say", "out", "done", "in1"),
                ln("done", "out", "final", "in"),
                ln("done", "out", "later", "in"),
                ln("later", "out", "o", "in"),
            ],
        );
        let host = Fake::with(&["never"]);
        let (o, ev) = go(&l, &host, "x").await;
        assert_eq!(o.status, RunStatus::Completed, "{o:?}");
        assert_eq!(o.replies, ["Along the way", "The answer"]);
        assert_eq!(o.llm_calls, 0);
        assert!(host.calls.lock().unwrap().is_empty());
        // Its step is logged as done, not stopped.
        assert!(ev.iter().any(|e| matches!(e, Event::Step(s)
            if s.block == "final" && s.status == crate::log::StepStatus::Done)));
    }

    #[tokio::test]
    async fn a_reminder_is_sent_last_with_an_exchange_only() {
        let l = |reminder: &str, via_exchange: bool| {
            let mut blocks = vec![
                b("in", "input", json!({})),
                b("goal", "store", json!({ "key": "goal" })),
                b("start", "exchange", json!({ "system": "S" })),
                b("ask", "llm", json!({ "reminder": reminder })),
                b("o", "output", json!({})),
            ];
            let mut links = vec![ln("in", "out", "goal", "in"), ln("ask", "out", "o", "in")];
            if via_exchange {
                links.push(ln("goal", "out", "start", "in"));
                links.push(ln("start", "out", "ask", "in"));
            } else {
                blocks.remove(2);
                links.push(ln("goal", "out", "ask", "in"));
            }
            lp(blocks, links)
        };
        let host = Fake::with(&["ok", "ok", "ok"]);
        go(&l("Your next step: {{goal}}", true), &host, "tidy up").await;
        go(&l("{{nothing_saved}}", true), &host, "x").await;
        go(&l("Your next step: {{goal}}", false), &host, "y").await;
        let calls = host.calls.lock().unwrap();
        let sent = |i: usize| -> Vec<(Role, String)> {
            calls[i]
                .messages
                .iter()
                .map(|m| (m.role, m.text.clone()))
                .collect()
        };
        // Last, as a user message, filled from memory.
        assert_eq!(
            sent(0),
            [
                (Role::System, "S".into()),
                (Role::User, "tidy up".into()),
                (Role::User, "Your next step: tidy up".into()),
            ]
        );
        // Empty once filled: nothing extra.
        assert_eq!(sent(1).len(), 2);
        // Text input: not used.
        assert!(sent(2).iter().all(|(_, t)| !t.contains("next step")));
    }

    #[tokio::test]
    async fn pane_output_sends_one_line_per_input_and_never_replies() {
        let l = lp(
            vec![
                b("in", "input", json!({})),
                b("sp", "regex", json!({ "pattern": "[^|]+" })),
                b("each", "for_each", json!({})),
                b("pane", "pane", json!({})),
                b("out", "output", json!({})),
            ],
            vec![
                ln("in", "out", "sp", "in"),
                ln("sp", "out", "each", "in"),
                ln("each", "out", "pane", "in"),
                ln("in", "out", "out", "in"),
            ],
        );
        let (o, ev) = go(&l, &Fake::with(&[]), "Reading\n  notes.txt|  |Done").await;
        assert_eq!(o.status, RunStatus::Completed);
        assert_eq!(o.replies.len(), 1, "the pane never adds to the reply");
        let lines: Vec<&str> = ev
            .iter()
            .filter_map(|e| match e {
                Event::Pane { block, text } => {
                    assert_eq!(block, "pane");
                    Some(text.as_str())
                }
                _ => None,
            })
            .collect();
        // Folded to one line each; a blank input shows nothing.
        assert_eq!(lines, vec!["Reading notes.txt", "Done"]);
        let long = pane_line(&"word ".repeat(400));
        assert_eq!(long.chars().count(), PANE_LINE_CHARS);
        assert!(long.ends_with('…'));
    }

    #[tokio::test]
    async fn a_value_that_keeps_doubling_stops_the_run() {
        // Combine fed its own output: the text doubles every step.
        let mut l = lp(
            vec![
                b("in", "input", json!({})),
                b(
                    "c",
                    "combine",
                    json!({ "inputs": 1, "format": "_INPUT1__INPUT1_" }),
                ),
            ],
            vec![ln("in", "out", "c", "in1"), ln("c", "out", "c", "in1")],
        );
        l.limits.max_steps = 1000;
        let (o, _) = go(&l, &Fake::default(), "hello").await;
        match o.status {
            RunStatus::LimitReached { message } => assert!(message.contains("64 MB"), "{message}"),
            other => panic!("{other:?}"),
        }
        assert!(
            o.steps < 40,
            "stopped early, not at the step limit: {}",
            o.steps
        );
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

        // (was tool_call_limit)
        {
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

        // (was waiting_runs_can_be_stopped)
        {
            let l = lp(
                vec![
                    b("in", "input", json!({})),
                    b("ask", "ask_user", json!({ "prompt": "Which one?" })),
                    b("out", "output", json!({})),
                ],
                vec![ln("in", "out", "ask", "in"), ln("ask", "out", "out", "in")],
            );
            let host = Fake {
                never_answer: true,
                ..Default::default()
            };
            let cancel = AtomicBool::new(false);
            let stopper = async {
                tokio::time::sleep(Duration::from_millis(150)).await;
                cancel.store(true, Ordering::Relaxed);
            };
            let inputs = RunInputs::default();
            let (o, _) = tokio::join!(run(&l, &host, &inputs, &cancel, |_| {}), stopper);
            assert_eq!(o.status, RunStatus::Stopped);
        }
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
        assert!(
            ev.iter().any(
                |e| matches!(e, Event::Notice { text, .. } if text.contains("is off in Chummy"))
            )
        );
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

    #[tokio::test]
    async fn read_exchange_newest_includes_messages_that_only_call_tools() {
        // An earlier assistant message, then a model turn that only calls a
        // tool: Last message skips the call-only message, Newest doesn't.
        let l = lp(
            vec![
                b("in", "input", json!({})),
                b("start", "exchange", json!({})),
                b(
                    "add",
                    "augment",
                    json!({ "role": "assistant", "format": "Earlier words" }),
                ),
                b("m", "llm", json!({ "tools": ["echo"] })),
                b("read", "read_exchange", json!({ "role": "assistant" })),
                b(
                    "both",
                    "combine",
                    json!({ "inputs": 2, "format": "[_INPUT1_|_INPUT2_]" }),
                ),
                b("o", "output", json!({})),
            ],
            vec![
                ln("in", "out", "start", "in"),
                ln("start", "out", "add", "exchange"),
                ln("in", "out", "add", "in"),
                ln("add", "out", "m", "in"),
                ln("m", "tool", "read", "in"),
                ln("read", "newest", "both", "in1"),
                ln("read", "last", "both", "in2"),
                ln("both", "out", "o", "in"),
            ],
        );
        let host = Fake::script(vec![calls("echo", json!({ "text": "x" }))]);
        let (o, _) = go(&l, &host, "hello").await;
        assert_eq!(o.replies, vec!["[|Earlier words\n\nhello]"], "{o:?}");
    }

    #[test]
    fn tool_results_fit_in_half_the_context() {
        assert_eq!(result_cap(50_000, 8_192), 16_384);
        assert_eq!(result_cap(50_000, 128_000), 50_000);
        assert_eq!(result_cap(50_000, 2_048), 12_000);
        assert_eq!(result_cap(500, 128_000), 500);
    }

    #[tokio::test]
    async fn context_check_measures_against_the_models_compaction_point() {
        // Default limits: 8,192 tokens, full at 80% (6,552). The system
        // message counts, filled in as it would be sent.
        let l = lp(
            vec![
                b("in", "input", json!({})),
                b("start", "exchange", json!({ "system": "Notes: {{notes}}" })),
                b("check", "context_check", json!({})),
                b("r", "forward", json!({})),
                b("f", "forward", json!({})),
                b("room", "combine", json!({ "inputs": 1, "format": "room" })),
                b("full", "combine", json!({ "inputs": 1, "format": "full" })),
                b("o", "output", json!({})),
            ],
            vec![
                ln("in", "out", "start", "in"),
                ln("start", "out", "check", "in"),
                ln("check", "room", "r", "in"),
                ln("check", "full", "f", "in"),
                ln("r", "out", "room", "in1"),
                ln("f", "out", "full", "in1"),
                ln("room", "out", "o", "in"),
                ln("full", "out", "o", "in"),
            ],
        );
        let host = Fake::with(&[]);
        let (o, _) = go(&l, &host, "short").await;
        assert_eq!(o.replies, ["room"], "{o:?}");
        // ~6,600 tokens of message: past 80%.
        let long = "word ".repeat(5300);
        let (o, ev) = go(&l, &host, &long).await;
        assert_eq!(o.replies, ["full"]);
        assert!(ev.iter().any(
            |e| matches!(e, Event::Notice { text, .. } if text.contains("of the model's 8192"))
        ));
        // A share of the context instead: 90% (7,372 tokens) has room.
        let mut l = l;
        l.blocks[2].params = json!({ "percent": 90 });
        assert_eq!(go(&l, &host, &long).await.0.replies, ["room"]);
        assert!(
            BlockKind::parse("context_check", &json!({ "percent": 0 })).is_ok()
                && !analyze(&{
                    let mut x = l.clone();
                    x.blocks[2].params = json!({ "percent": 0 });
                    x
                })
                .ok
        );
    }

    #[tokio::test]
    async fn runs_report_turn_usage() {
        // Message → model (calls echo) → Run tool → model answers.
        let l = lp(
            vec![
                b("in", "input", json!({})),
                b("m", "llm", json!({ "tools": ["echo"] })),
                b("run", "run_tool", json!({})),
                b("o", "output", json!({})),
            ],
            vec![
                ln("in", "out", "m", "in"),
                ln("m", "tool", "run", "in"),
                ln("run", "out", "m", "in"),
                ln("m", "out", "o", "in"),
            ],
        );
        let host = Fake::script(vec![
            calls("echo", json!({ "text": "hi" })),
            text("12345678"),
        ]);
        let (o, _) = go(&l, &host, "abcd").await;
        assert_eq!(o.status, RunStatus::Completed, "{o:?}");
        let u = &o.usage;
        // message (4 + 1) + call (4 + 4 + echo{"text":"hi"} ≈ 5) + result (4 + 3) + answer (4 + 2)
        assert!(u.tokens_added > 20 && u.tokens_added < 40, "{u:?}");
        assert_eq!(u.context_window, crate::DEFAULT_CONTEXT_WINDOW);
        // The last call's context holds the message, the call, the result and the answer.
        assert!(u.context_tokens >= u.tokens_added - 1, "{u:?}");
        assert_eq!(u.model, None);
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
            notes: vec![],
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

    #[tokio::test]
    async fn steps_log_inputs_and_outputs_inside_functions() {
        let shout = func(
            "shout",
            vec![b(
                "c",
                "combine",
                json!({ "inputs": 1, "format": "_INPUT1_!" }),
            )],
            vec![],
            &[("in1", "c", "in1")],
            &[("out1", "c", "out")],
        );
        let mut l = lp(
            vec![
                b("in", "input", json!({})),
                b("f", "function", json!({ "function": "shout" })),
                b("o", "output", json!({})),
            ],
            vec![ln("in", "out", "f", "in1"), ln("f", "out1", "o", "in")],
        );
        l.functions = vec![shout];
        let (o, ev) = go(&l, &Fake::default(), "hey").await;
        assert_eq!(o.replies, ["hey!"]);
        let steps: Vec<&StepLog> = ev
            .iter()
            .filter_map(|e| match e {
                Event::Step(s) => Some(s.as_ref()),
                _ => None,
            })
            .collect();
        // The inner block finishes before its function does, but starts after.
        let names: Vec<(&str, u32)> = steps.iter().map(|s| (s.block.as_str(), s.n)).collect();
        assert_eq!(names, [("in", 1), ("f/c", 3), ("f", 2), ("o", 4)]);
        let f = steps[2];
        assert_eq!(f.kind, "function");
        assert_eq!(f.function.as_deref(), Some("shout"));
        assert_eq!(f.inputs[0].port, "in1");
        assert_eq!(f.inputs[0].value.text, "hey");
        assert_eq!(f.outputs[0].port, "out1");
        assert_eq!(f.outputs[0].value.text, "hey!");
        assert_eq!(steps[1].outputs[0].value.text, "hey!");
        assert!(steps.iter().all(|s| s.status == StepStatus::Done));
    }

    #[tokio::test]
    async fn failures_and_model_calls_are_logged() {
        let l = lp(
            vec![
                b("in", "input", json!({})),
                b("m", "llm", json!({ "system": "Be brief." })),
                b("o", "output", json!({})),
            ],
            vec![ln("in", "out", "m", "in"), ln("m", "out", "o", "in")],
        );
        let (_, ev) = go(&l, &Fake::with(&["Sure."]), "Hi").await;
        let call = ev
            .iter()
            .find_map(|e| match e {
                Event::ModelCall(c) => Some(c),
                _ => None,
            })
            .unwrap();
        assert_eq!(call.block, "m");
        assert_eq!(call.answer.text, "Sure.");
        assert!(call.sent.text.contains("Be brief."), "{}", call.sent.text);
        assert!(call.sent.text.contains("Hi"));

        // A failure that stops the run is a failed step with the error.
        let l = lp(
            vec![
                b("in", "input", json!({})),
                b("r", "read_file", json!({})),
                b("o", "output", json!({})),
            ],
            vec![ln("in", "out", "r", "path"), ln("r", "out", "o", "in")],
        );
        let (_, ev) = go(&l, &Fake::default(), "missing.txt").await;
        let step = ev
            .iter()
            .find_map(|e| match e {
                Event::Step(s) if s.block == "r" => Some(s),
                _ => None,
            })
            .unwrap();
        assert_eq!(step.status, StepStatus::Failed);
        assert!(step.error.is_some());
    }

    #[tokio::test]
    async fn plan_outline_tracks_the_whole_plan() {
        let l = lp(
            vec![
                b("in", "input", json!({})),
                b("start", "outline", json!({ "action": "start" })),
                b("steps", "fixed", json!({ "text": "Guests\nFood" })),
                b("split", "regex", json!({ "pattern": "\\w+" })),
                b("add", "outline", json!({ "action": "add_steps" })),
                b("done", "fixed", json!({ "text": "Task: Guests" })),
                b(
                    "gate",
                    "combine",
                    json!({ "inputs": 2, "format": "_INPUT1_" }),
                ),
                b(
                    "mark",
                    "outline",
                    json!({ "action": "mark", "status": "done" }),
                ),
                b("o", "output", json!({})),
                b("o2", "output", json!({})),
            ],
            vec![
                ln("in", "out", "start", "in"),
                ln("in", "out", "add", "task"),
                ln("steps", "out", "split", "in"),
                ln("split", "out", "add", "steps"),
                ln("done", "out", "gate", "in1"),
                ln("add", "out", "gate", "in2"),
                ln("gate", "out", "mark", "task"),
                ln("mark", "out", "o", "in"),
                ln("start", "out", "o2", "in"),
            ],
        );
        let a = analyze(&l);
        assert!(a.ok, "{:?}", a.issues);
        let (o, _) = go(&l, &Fake::default(), "Party").await;
        assert_eq!(o.status, RunStatus::Completed, "{o:?}");
        assert_eq!(
            o.replies.last().unwrap(),
            "▾ 1. Party\n    ✓ 1.1. Guests\n    ○ 1.2. Food"
        );
    }

    #[tokio::test]
    async fn block_tool_switches_inherit_from_the_agent() {
        struct Agent {
            fake: Fake,
            agent_echo: bool,
        }
        impl Host for Agent {
            async fn llm(
                &self,
                call: LlmCall,
                t: &mut (dyn FnMut(&str) + Send),
            ) -> Result<LlmAnswer, LlmFailure> {
                self.fake.llm(call, t).await
            }
            fn tool_names(&self) -> Vec<String> {
                self.fake.tool_names()
            }
            fn tool_info(&self, name: &str) -> Option<ToolInfo> {
                self.fake.tool_info(name)
            }
            fn agent_allows(&self, _: &str) -> bool {
                self.agent_echo
            }
            async fn run_tool(&self, n: &str, a: Json, s: Switch) -> Result<ToolOutput, String> {
                self.fake.run_tool(n, a, s).await
            }
            fn read_file(&self, p: &str) -> Result<String, String> {
                self.fake.read_file(p)
            }
            fn write_file(&self, p: &str, c: &str, a: bool) -> Result<(), String> {
                self.fake.write_file(p, c, a)
            }
            fn list_files(&self, p: &str) -> Result<Vec<String>, String> {
                self.fake.list_files(p)
            }
            async fn ask(&self, a: Ask) -> Result<Answer, String> {
                self.fake.ask(a).await
            }
        }
        let offered = |params: Json, agent_echo: bool| async move {
            let off = params["tools"] == "off";
            let l = lp(
                vec![
                    b("in", "input", json!({})),
                    b("m", "llm", params),
                    b("o", "output", json!({})),
                    b("run", "run_tool", json!({})),
                ],
                {
                    let mut links = vec![ln("in", "out", "m", "in"), ln("m", "out", "o", "in")];
                    // With every tool off, the block has no Tool output to wire.
                    if !off {
                        links.push(ln("m", "tool", "run", "in"));
                        links.push(ln("run", "out", "m", "in"));
                    }
                    links
                },
            );
            let host = Agent {
                fake: Fake::with(&["ok"]),
                agent_echo,
            };
            let (_, ev) = go(&l, &host, "x").await;
            let names: Vec<String> = host.fake.calls.lock().unwrap()[0]
                .tools
                .iter()
                .map(|t| t.name.clone())
                .collect();
            let notice = ev.iter().any(
                |e| matches!(e, Event::Notice { text, .. } if text.contains("off for this agent")),
            );
            (names, notice)
        };
        // Inherit (the default): what the agent allows.
        assert_eq!(offered(json!({}), true).await.0, ["echo"]);
        assert!(offered(json!({}), false).await.0.is_empty());
        // Off: never.
        assert!(offered(json!({ "tools": "off" }), true).await.0.is_empty());
        // A block can't turn on what the agent has off: an older loop's "on"
        // reads as "same as the agent".
        let (on, notice) = offered(json!({ "tools": "on" }), false).await;
        assert!(on.is_empty(), "{on:?}");
        assert!(!notice);
        assert_eq!(offered(json!({ "tools": "on" }), true).await.0, ["echo"]);
        let each = json!({ "tools": { "each": { "echo": "on" } } });
        assert!(offered(each, false).await.0.is_empty());
        // Per tool.
        let each = json!({ "tools": { "all": "off", "each": { "echo": "inherit" } } });
        assert_eq!(offered(each, true).await.0, ["echo"]);
        // No Run tool after the block: it just answers, without tools.
        let l = lp(
            vec![
                b("in", "input", json!({})),
                b("m", "llm", json!({})),
                b("o", "output", json!({})),
            ],
            vec![ln("in", "out", "m", "in"), ln("m", "out", "o", "in")],
        );
        let host = Fake::with(&["ok"]);
        go(&l, &host, "x").await;
        assert!(host.calls.lock().unwrap()[0].tools.is_empty());
        // A tool the app doesn't allow is never offered, whatever the block says.
        assert!(
            offered(json!({ "tools": { "each": { "nope": "on" } } }), true)
                .await
                .0
                == ["echo"]
        );
    }

    #[test]
    fn older_loops_keep_blocks_without_tools_tool_free() {
        let json = r#"{"format":"chummy.loop","version":2,"name":"x","blocks":[
            {"id":"m","type":"llm","params":{}},
            {"id":"t","type":"llm","params":{"tools":["echo"],"network":true}}]}"#;
        let l = LoopFile::from_json(json).unwrap();
        let p = |i: usize| match BlockKind::parse(&l.blocks[i].kind, &l.blocks[i].params).unwrap() {
            BlockKind::Llm(p) => p,
            _ => unreachable!(),
        };
        assert_eq!(p(0).tools, ToolSwitches::off());
        assert_eq!(p(0).network, Switch::Off);
        assert_eq!(p(1).tools.switch_for("echo"), Switch::Inherit);
        assert_eq!(p(1).tools.switch_for("other"), Switch::Off);
        assert_eq!(p(1).network, Switch::Inherit);
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

    #[tokio::test]
    async fn tool_call_ids_are_made_unique() {
        // Two calls with empty ids, then the same id again on the next turn.
        let twin = |id: &str| ToolCall {
            id: id.into(),
            name: "echo".into(),
            arguments: json!({ "text": "a" }),
            network: Switch::Off,
        };
        let host = Fake::script(vec![
            LlmAnswer {
                calls: vec![twin(""), twin("")],
                ..Default::default()
            },
            LlmAnswer {
                calls: vec![twin("same")],
                ..Default::default()
            },
            LlmAnswer {
                calls: vec![twin("same")],
                ..Default::default()
            },
            text("done"),
        ]);
        let (o, _) = go(&tool_loop(), &host, "x").await;
        assert_eq!(o.replies, vec!["done"], "{o:?}");
        assert_eq!(o.tool_calls, 4);
        let last = host.calls.lock().unwrap().last().unwrap().clone();
        let ids: Vec<String> = last
            .messages
            .iter()
            .flat_map(|m| m.calls.iter().map(|c| c.id.clone()))
            .collect();
        let unique: std::collections::HashSet<&String> = ids.iter().collect();
        assert_eq!(ids.len(), 4);
        assert_eq!(unique.len(), 4, "{ids:?}");
        assert!(ids.iter().all(|i| !i.is_empty()));
        // Every tool result answers one of them.
        let answered: Vec<String> = last
            .messages
            .iter()
            .filter_map(|m| m.call_id.clone())
            .collect();
        assert_eq!(answered.len(), 4);
        assert!(answered.iter().all(|a| ids.contains(a)));
    }

    #[tokio::test]
    async fn reasoning_opened_in_the_prompt_is_split_off() {
        let l = lp(
            vec![
                b("in", "input", json!({})),
                b("m", "llm", json!({})),
                b("o", "output", json!({})),
                b("r", "output", json!({})),
            ],
            vec![
                ln("in", "out", "m", "in"),
                ln("m", "out", "o", "in"),
                ln("m", "reasoning", "r", "in"),
            ],
        );
        let host = Fake::with(&["first I think</think>\n\nThe answer."]);
        let (o, _) = go(&l, &host, "x").await;
        assert_eq!(o.replies, vec!["The answer.", "first I think"]);
    }

    #[tokio::test]
    async fn a_function_output_can_fire_many_times_and_runs_stop_inside() {
        let each = func(
            "each",
            vec![
                b("rx", "regex", json!({ "pattern": "\\w+" })),
                b("fe", "for_each", json!({})),
            ],
            vec![ln("rx", "out", "fe", "in")],
            &[("in1", "rx", "in")],
            &[("out1", "fe", "out")],
        );
        let mut l = lp(
            vec![
                b("in", "input", json!({})),
                b("f", "function", json!({ "function": "each" })),
                b("o", "output", json!({})),
            ],
            vec![ln("in", "out", "f", "in1"), ln("f", "out1", "o", "in")],
        );
        l.functions = vec![each];
        let (o, _) = go(&l, &Fake::default(), "one two three").await;
        assert_eq!(o.replies, vec!["one", "two", "three"]);

        // Stopping inside a function stops the whole run.
        let cancel = AtomicBool::new(true);
        let inputs = RunInputs {
            message: "a b".into(),
            ..Default::default()
        };
        let o = run(&l, &Fake::default(), &inputs, &cancel, |_| {}).await;
        assert_eq!(o.status, RunStatus::Stopped);
    }

    #[test]
    fn huge_loops_are_refused() {
        let blocks: Vec<Block> = (0..crate::validate::MAX_BLOCKS + 1)
            .map(|i| b(&format!("b{i}"), "fixed", json!({ "text": "x" })))
            .collect();
        let a = analyze(&lp(blocks, vec![]));
        assert!(a.issues.iter().any(|i| i.message.contains("too large")));
    }

    #[tokio::test]
    async fn ask_me_waits_for_the_answer() {
        let l = lp(
            vec![
                b("in", "input", json!({})),
                b(
                    "ask",
                    "ask_user",
                    json!({ "prompt": "Which file for _INPUT_?" }),
                ),
                b("out", "output", json!({})),
            ],
            vec![ln("in", "out", "ask", "in"), ln("ask", "out", "out", "in")],
        );
        let host = Fake::default();
        host.answers_for_user
            .lock()
            .unwrap()
            .push_back(Answer::Text("notes.md".into()));
        let (o, events) = go(&l, &host, "the summary").await;
        assert_eq!(o.replies, ["notes.md"]);
        let asks = host.asks.lock().unwrap().clone();
        assert_eq!(asks[0].text, "Which file for the summary?");
        assert_eq!(asks[0].kind, AskKind::Question);
        assert!(events.iter().any(|e| matches!(e, Event::Asked { .. })));
        assert!(events.iter().any(|e| matches!(e, Event::Answered { .. })));

        // Nobody to ask: the run stops with the host's reason.
        let host = Fake::default();
        let (o, _) = go(&l, &host, "x").await;
        assert!(
            matches!(o.status, RunStatus::Failed { ref error, .. } if error.contains("nobody to ask")),
            "{o:?}"
        );
    }

    #[tokio::test(start_paused = true)]
    async fn ask_me_times_out_to_the_default_or_stops() {
        let mk = |default: &str| {
            lp(
                vec![
                    b("in", "input", json!({})),
                    b(
                        "ask",
                        "ask_user",
                        json!({ "prompt": "_INPUT_", "timeout_minutes": 5, "default_answer": default }),
                    ),
                    b("out", "output", json!({})),
                ],
                vec![ln("in", "out", "ask", "in"), ln("ask", "out", "out", "in")],
            )
        };
        let host = Fake {
            never_answer: true,
            ..Default::default()
        };
        let (o, _) = go(&mk("skip it"), &host, "Anything?").await;
        assert_eq!(o.replies, ["skip it"]);
        let (o, _) = go(&mk(""), &host, "Anything?").await;
        assert!(
            matches!(o.status, RunStatus::Failed { ref error, .. } if error.contains("within 5 minutes")),
            "{o:?}"
        );
    }

    #[tokio::test]
    async fn stop_with_error_ends_the_run() {
        let l = lp(
            vec![
                b("in", "input", json!({})),
                b("say", "output", json!({})),
                b("stop", "stop", json!({})),
                b("later", "output", json!({})),
            ],
            vec![
                ln("in", "out", "say", "in"),
                ln("in", "out", "stop", "in"),
                ln("in", "out", "later", "in"),
            ],
        );
        let host = Fake::default();
        let (out, _) = go(&l, &host, "it can't be done").await;
        assert_eq!(
            out.replies,
            ["it can't be done"],
            "blocks queued after it don't run"
        );
        assert!(
            matches!(&out.status, RunStatus::Failed { error, .. } if error.contains("it can't be done")),
            "{:?}",
            out.status
        );
    }

    #[tokio::test]
    async fn blank_questions_are_never_asked() {
        let l = lp(
            vec![
                b("in", "input", json!({})),
                b("ask", "ask_user", json!({})),
                b("out", "output", json!({})),
                b("err", "output", json!({})),
            ],
            vec![
                ln("in", "out", "ask", "in"),
                ln("ask", "out", "out", "in"),
                ln("ask", "error", "err", "in"),
            ],
        );
        let host = Fake::default();
        let (out, _) = go(&l, &host, "  \n ").await;
        assert!(host.asks.lock().unwrap().is_empty());
        assert!(
            out.replies.iter().any(|t| t.contains("no question")),
            "{:?}",
            out.replies
        );
    }

    #[tokio::test]
    async fn tool_calls_can_need_the_users_ok() {
        // The block asks: allowed, then refused with a reason.
        let mut l = tool_loop();
        l.blocks[2].params = json!({ "confirm": true });
        let host = Fake::script(vec![
            calls("echo", json!({ "text": "one" })),
            calls("echo", json!({ "text": "two" })),
            text("done"),
        ]);
        host.answers_for_user
            .lock()
            .unwrap()
            .extend([Answer::Allow, Answer::Deny("not that one".into())]);
        let (o, _) = go(&l, &host, "x").await;
        assert_eq!(o.replies, ["done"]);
        assert_eq!(
            host.tool_runs.lock().unwrap().len(),
            1,
            "only the allowed call ran"
        );
        let asks = host.asks.lock().unwrap().clone();
        assert!(matches!(
            &asks[0].kind,
            AskKind::ToolApproval { tool, arguments } if tool == "echo" && arguments["text"] == "one"
        ));
        assert!(asks[0].text.contains("text: one"));
        let last = host.calls.lock().unwrap().last().unwrap().messages.clone();
        let refusal = last.iter().rev().find(|m| m.role == Role::Tool).unwrap();
        assert!(refusal.text.contains("didn't allow") && refusal.text.contains("not that one"));

        // The host can require it even when the block doesn't ask.
        let l = tool_loop();
        let host = Fake {
            confirm_tools: vec!["echo".into()],
            ..Fake::script(vec![calls("echo", json!({ "text": "a" })), text("ok")])
        };
        host.answers_for_user
            .lock()
            .unwrap()
            .push_back(Answer::Allow);
        let (o, _) = go(&l, &host, "x").await;
        assert_eq!(
            (o.replies.as_slice(), host.asks.lock().unwrap().len()),
            (&["ok".to_string()][..], 1)
        );

        // Without confirmation nothing is asked.
        let host = Fake::script(vec![calls("echo", json!({ "text": "a" })), text("ok")]);
        go(&tool_loop(), &host, "x").await;
        assert!(host.asks.lock().unwrap().is_empty());
    }

    #[test]
    fn approvals_show_everything_plainly() {
        let long = format!("https://good.example/{}&secret=1", "a".repeat(2100));
        let d = describe_call("fetch_page", &json!({ "url": long }));
        assert!(d.contains("more characters not shown"), "{d}");
        let tricky = describe_call("t", &json!({ "path": "evil\u{202e}txt.exe" }));
        assert!(tricky.contains("<U+202E>") && !tricky.contains('\u{202e}'));
        assert_eq!(describe_call("t", &json!({})), "Run the tool “t”?");
    }

    #[tokio::test]
    async fn a_withdrawn_question_during_a_stop_is_a_stop() {
        let l = lp(
            vec![
                b("in", "input", json!({})),
                b("ask", "ask_user", json!({ "prompt": "Which one?" })),
                b("out", "output", json!({})),
            ],
            vec![ln("in", "out", "ask", "in"), ln("ask", "out", "out", "in")],
        );
        let cancel = std::sync::Arc::new(AtomicBool::new(false));
        let host = Fake {
            stop_on_ask: Some(cancel.clone()),
            ..Default::default()
        };
        let o = run(&l, &host, &RunInputs::default(), &cancel, |_| {}).await;
        assert_eq!(o.status, RunStatus::Stopped);
        assert_eq!(host.asks.lock().unwrap().len(), 1);
    }

    #[tokio::test]
    async fn prompted_tools_are_described_and_read_from_text() {
        let mut l = tool_loop();
        l.blocks[1].params = json!({ "prompt": "Do: _INPUT_", "tools": ["echo"], "tool_mode": "prompted", "network": "on" });
        let host = Fake::script(vec![
            text(
                "Sure.\n<tool_call>\n{\"name\": \"echo\", \"arguments\": {\"text\": \"hi\"}}\n</tool_call>",
            ),
            text("done"),
        ]);
        let (o, _) = go(&l, &host, "x").await;
        assert_eq!(o.replies, ["done"], "{o:?}");
        let calls = host.calls.lock().unwrap();
        // No tools through the API; they're in the system message instead.
        assert!(calls[0].tools.is_empty());
        assert!(
            calls[0].messages[0].role == Role::System
                && calls[0].messages[0].text.contains("<tools>")
        );
        // The second call sees the call and result as plain text.
        let second = &calls[1].messages;
        assert!(
            second
                .iter()
                .all(|m| m.role != Role::Tool && m.calls.is_empty())
        );
        assert!(
            second
                .iter()
                .any(|m| m.text.contains("<tool_response>") && m.text.contains("echoed hi"))
        );
        assert_eq!(
            *host.online_runs.lock().unwrap(),
            [Switch::Inherit],
            "the block's internet switch rides with the call (an old \"on\" reads as inherit)"
        );
    }

    #[tokio::test]
    async fn tools_off_lets_the_loop_handle_them() {
        // model (tools listed in the prompt, not offered) → Find tool call →
        // Run tool → Read exchange → Reply; plain answers go out of No call.
        let l = lp(
            vec![
                b("in", "input", json!({})),
                b(
                    "m",
                    "llm",
                    json!({ "prompt": "Tools:\n_TOOLS_\n\n_INPUT_", "tools": ["echo"], "tool_mode": "off" }),
                ),
                b("ex", "exchange", json!({})),
                b("find", "find_call", json!({ "tools": ["echo"] })),
                b("run", "run_tool", json!({})),
                b("read", "read_exchange", json!({ "role": "tool" })),
                b("out", "output", json!({})),
            ],
            vec![
                ln("in", "out", "m", "in"),
                ln("in", "out", "ex", "in"),
                ln("ex", "out", "find", "exchange"),
                ln("m", "out", "find", "in"),
                ln("find", "call", "run", "in"),
                ln("run", "out", "read", "in"),
                ln("read", "last", "out", "in"),
                ln("find", "none", "out", "in"),
            ],
        );
        let host = Fake::with(&["{\"name\": \"echo\", \"arguments\": {\"text\": \"yo\"}}"]);
        let (o, _) = go(&l, &host, "x").await;
        assert_eq!(o.replies, ["echoed yo"], "{o:?}");
        let first = host.calls.lock().unwrap()[0].clone();
        assert!(first.tools.is_empty());
        assert!(
            first.messages[0]
                .text
                .contains("- echo(text): Echoes text.")
        );
        assert!(
            !BlockKind::parse("llm", &json!({ "tools": ["echo"], "tool_mode": "off" }))
                .unwrap()
                .outputs()
                .iter()
                .any(|p| p.name == "tool")
        );
        let host = Fake::with(&["no call here"]);
        assert_eq!(go(&l, &host, "x").await.0.replies, ["no call here"]);
        // A tool that isn't ticked in Find tool call is never run.
        let mut l = l;
        l.blocks[3].params = json!({ "tools": [] });
        let call = "{\"name\": \"echo\", \"arguments\": {\"text\": \"yo\"}}";
        let host = Fake::with(&[call]);
        assert_eq!(go(&l, &host, "x").await.0.replies, [call]);
    }

    #[tokio::test]
    async fn failed_outputs_catch_errors_when_linked() {
        // Read file fails (no such file); its Failed output feeds the reply.
        let l = lp(
            vec![
                b("in", "input", json!({})),
                b("read", "read_file", json!({})),
                b("out", "output", json!({})),
            ],
            vec![
                ln("in", "out", "read", "path"),
                ln("read", "error", "out", "in"),
            ],
        );
        let (o, _) = go(&l, &Fake::default(), "missing.txt").await;
        assert_eq!(o.status, RunStatus::Completed);
        assert_eq!(o.replies, ["missing.txt: not found"]);
        // Unlinked, the run stops as before.
        let mut l2 = l.clone();
        l2.links.pop();
        let (o, _) = go(&l2, &Fake::default(), "missing.txt").await;
        assert!(matches!(o.status, RunStatus::Failed { .. }));
    }

    #[tokio::test]
    async fn memory_is_saved_and_read_in_prompts() {
        let l = lp(
            vec![
                b("in", "input", json!({})),
                b("save", "store", json!({ "key": "notes", "max_chars": 100 })),
                b(
                    "m",
                    "llm",
                    json!({ "system": "Notes: {{notes}}", "prompt": "Q: _INPUT_" }),
                ),
                b("get", "recall", json!({ "key": "notes" })),
                b(
                    "both",
                    "combine",
                    json!({ "inputs": 2, "format": "_INPUT1_ / {{notes}} / _INPUT2_" }),
                ),
                b("out", "output", json!({})),
            ],
            vec![
                ln("in", "out", "save", "in"),
                ln("save", "out", "m", "in"),
                ln("m", "out", "get", "in"),
                ln("m", "out", "both", "in1"),
                ln("get", "out", "both", "in2"),
                ln("both", "out", "out", "in"),
            ],
        );
        let host = Fake::with(&["answer"]);
        let (o, _) = go(&l, &host, "remember {{notes}} this").await;
        let sent = host.calls.lock().unwrap()[0].messages.clone();
        assert_eq!(sent[0].text, "Notes: remember {{notes}} this");
        assert_eq!(sent[1].text, "Q: remember {{notes}} this");
        assert_eq!(
            o.replies,
            ["answer / remember {{notes}} this / remember {{notes}} this"]
        );
    }

    #[tokio::test]
    async fn daily_limits_ask_wait_or_stop() {
        let l = lp(
            vec![
                b("in", "input", json!({})),
                b("m", "llm", json!({ "on_limit": "ask" })),
                b("out", "output", json!({})),
            ],
            vec![ln("in", "out", "m", "in"), ln("m", "out", "out", "in")],
        );
        // Asked and allowed: more is granted and the call goes ahead.
        let host = Fake::with(&["ok"]);
        *host.exhausted.lock().unwrap() = true;
        host.answers_for_user
            .lock()
            .unwrap()
            .push_back(Answer::Allow);
        let (o, _) = go(&l, &host, "x").await;
        assert_eq!(o.replies, ["ok"]);
        assert_eq!(*host.granted.lock().unwrap(), [25]);
        assert!(matches!(
            host.asks.lock().unwrap()[0].kind,
            AskKind::Budget { limit: 100, .. }
        ));
        // Refused: the run stops at a limit.
        let host = Fake::with(&["ok"]);
        *host.exhausted.lock().unwrap() = true;
        host.answers_for_user
            .lock()
            .unwrap()
            .push_back(Answer::Deny(String::new()));
        let (o, _) = go(&l, &host, "x").await;
        assert!(matches!(o.status, RunStatus::LimitReached { .. }), "{o:?}");
        // Stop: no question at all.
        let mut stop = l.clone();
        stop.blocks[1].params = json!({ "on_limit": "stop" });
        let host = Fake::with(&["ok"]);
        *host.exhausted.lock().unwrap() = true;
        let (o, _) = go(&stop, &host, "x").await;
        assert!(
            matches!(o.status, RunStatus::LimitReached { ref message } if message.contains("Settings → Models"))
        );
        assert!(host.asks.lock().unwrap().is_empty());
    }

    #[tokio::test(start_paused = true)]
    async fn daily_limits_can_wait() {
        let l = lp(
            vec![
                b("in", "input", json!({})),
                b("m", "llm", json!({ "on_limit": "wait" })),
                b("out", "output", json!({})),
            ],
            vec![ln("in", "out", "m", "in"), ln("m", "out", "out", "in")],
        );
        let host = std::sync::Arc::new(Fake::with(&["later"]));
        *host.exhausted.lock().unwrap() = true;
        let h = host.clone();
        let refresh = async move {
            tokio::time::sleep(Duration::from_secs(150)).await;
            *h.exhausted.lock().unwrap() = false;
        };
        let cancel = AtomicBool::new(false);
        let inputs = RunInputs::default();
        let (o, _) = tokio::join!(run(&l, &*host, &inputs, &cancel, |_| {}), refresh);
        assert_eq!(o.replies, ["later"]);
    }

    #[tokio::test]
    async fn checking_calls_before_they_run() {
        // model → Tool call → Read exchange (calls) → Branch on a check:
        // allowed calls run; others are declined with the reason.
        let l = lp(
            vec![
                b("in", "input", json!({})),
                b("m", "llm", json!({ "tools": ["echo"] })),
                b("read", "read_exchange", json!({})),
                b("check", "fixed", json!({ "text": "" })),
                b("br", "branch", json!({ "pattern": "text: ok" })),
                b("run", "run_tool", json!({})),
                b("why", "fixed", json!({ "text": "the check said no" })),
                b("no", "decline_calls", json!({})),
                b("out", "output", json!({})),
            ],
            vec![
                ln("in", "out", "m", "in"),
                ln("m", "tool", "read", "in"),
                ln("m", "tool", "br", "value"),
                ln("read", "calls", "br", "test"),
                ln("br", "yes", "run", "in"),
                ln("br", "no", "no", "in"),
                ln("why", "out", "no", "reason"),
                ln("run", "out", "m", "in"),
                ln("no", "out", "m", "in"),
                ln("m", "out", "out", "in"),
            ],
        );
        let host = Fake::script(vec![
            calls("echo", json!({ "text": "bad" })),
            text("gave up"),
        ]);
        let (o, _) = go(&l, &host, "x").await;
        assert_eq!(o.replies, ["gave up"]);
        assert!(host.tool_runs.lock().unwrap().is_empty());
        let last = host.calls.lock().unwrap()[1].messages.clone();
        assert!(
            last.iter()
                .any(|m| m.role == Role::Tool && m.text == "Not run: the check said no")
        );

        let host = Fake::script(vec![calls("echo", json!({ "text": "ok" })), text("done")]);
        let (o, _) = go(&l, &host, "x").await;
        assert_eq!(
            (o.replies.as_slice(), host.tool_runs.lock().unwrap().len()),
            (&["done".to_string()][..], 1)
        );
    }

    /// Runs `l` able to pause, by `host.pause`, or carries on `resume`.
    async fn go_pausable(
        l: &LoopFile,
        host: &Fake,
        msg: &str,
        resume: Option<RunState>,
    ) -> (RunOutcome, Vec<Event>) {
        let mut ev = vec![];
        let inputs = RunInputs {
            message: msg.into(),
            ..Default::default()
        };
        let cancel = AtomicBool::new(false);
        let out = run_with(
            l,
            host,
            &inputs,
            resume,
            &cancel,
            Some(&host.pause),
            Some(&host.hurry),
            |e| ev.push(e),
        )
        .await;
        (out, ev)
    }

    /// A paused run's state as it would be saved and read back.
    fn saved(o: &RunOutcome) -> RunState {
        let state = o.paused.as_ref().expect("the run paused");
        serde_json::from_str(&serde_json::to_string(state).unwrap()).unwrap()
    }

    /// One reply, then a function with two model calls and memory between
    /// them, then the function's answer.
    fn two_step_loop() -> LoopFile {
        let work = func(
            "work",
            vec![
                b("m1", "llm", json!({ "prompt": "one: _INPUT_" })),
                b("st", "store", json!({ "key": "note" })),
                b("m2", "llm", json!({ "prompt": "two: _INPUT_ ({{note}})" })),
            ],
            vec![ln("m1", "out", "st", "in"), ln("st", "out", "m2", "in")],
            &[("x", "m1", "in")],
            &[("y", "m2", "out")],
        );
        let mut l = lp(
            vec![
                b("in", "input", json!({})),
                b("r0", "llm", json!({ "prompt": "zero: _INPUT_" })),
                b("o0", "output", json!({})),
                b("f", "function", json!({ "function": "work" })),
                b("o", "output", json!({})),
            ],
            vec![
                ln("in", "out", "r0", "in"),
                ln("r0", "out", "o0", "in"),
                ln("in", "out", "f", "x"),
                ln("f", "y", "o", "in"),
            ],
        );
        l.functions = vec![work];
        l
    }

    #[tokio::test]
    async fn a_paused_run_carries_on_where_it_stopped() {
        let l = two_step_loop();
        let whole = Fake::default();
        let (all, _) = go(&l, &whole, "hi").await;
        assert_eq!(all.status, RunStatus::Completed);

        // Pause after the second model call: inside the function, between
        // its first model block and Save to memory.
        let first = Fake {
            pause_after_call: 2,
            ..Default::default()
        };
        let (o, _) = go_pausable(&l, &first, "hi", None).await;
        assert_eq!(o.status, RunStatus::Paused { reason: None }, "{o:?}");
        let state = saved(&o);
        assert_eq!(state.frames.len(), 2, "the loop and the function call");

        let second = Fake::default();
        let (rest, _) = go_pausable(&l, &second, "hi", Some(state)).await;
        assert_eq!(rest.status, RunStatus::Completed, "{rest:?}");
        // Exactly the same run: nothing asked twice, memory kept, the same
        // replies and counts.
        let mut prompts = first.prompts.lock().unwrap().clone();
        prompts.extend(second.prompts.lock().unwrap().iter().cloned());
        assert_eq!(prompts, *whole.prompts.lock().unwrap());
        assert_eq!(rest.replies, all.replies);
        assert_eq!((rest.steps, rest.llm_calls), (all.steps, all.llm_calls));
    }

    #[tokio::test]
    async fn a_run_that_cannot_pause_never_does() {
        let l = two_step_loop();
        let host = Fake {
            pause_after_call: 1,
            ..Default::default()
        };
        let (o, _) = go(&l, &host, "hi").await;
        assert_eq!(o.status, RunStatus::Completed);
        assert!(o.paused.is_none());
    }

    #[tokio::test]
    async fn waiting_on_the_person_is_cut_short_to_pause() {
        let l = lp(
            vec![
                b("in", "input", json!({})),
                b(
                    "ask",
                    "ask_user",
                    json!({ "prompt": "Which file for _INPUT_?" }),
                ),
                b("out", "output", json!({})),
            ],
            vec![ln("in", "out", "ask", "in"), ln("ask", "out", "out", "in")],
        );
        let host = Fake {
            pause_at_ask: 1,
            never_answer: true,
            ..Default::default()
        };
        let (o, ev) = go_pausable(&l, &host, "the summary", None).await;
        assert_eq!(o.status, RunStatus::Paused { reason: None }, "{o:?}");
        assert!(ev.iter().any(|e| matches!(e, Event::Answered { .. })));
        // Asked again on resuming.
        let host = Fake::default();
        host.answers_for_user
            .lock()
            .unwrap()
            .push_back(Answer::Text("notes.md".into()));
        let (o, _) = go_pausable(&l, &host, "the summary", Some(saved(&o))).await;
        assert_eq!(o.replies, ["notes.md"]);
        assert_eq!(
            host.asks.lock().unwrap()[0].text,
            "Which file for the summary?"
        );
    }

    #[tokio::test]
    async fn a_block_that_ran_a_tool_finishes_before_pausing() {
        let two_calls = LlmAnswer {
            calls: vec![
                ToolCall {
                    id: "a".into(),
                    name: "echo".into(),
                    arguments: json!({ "text": "one" }),
                    network: Switch::Off,
                },
                ToolCall {
                    id: "b".into(),
                    name: "echo".into(),
                    arguments: json!({ "text": "two" }),
                    network: Switch::Off,
                },
            ],
            ..Default::default()
        };
        let host = Fake {
            confirm_tools: vec!["echo".into()],
            // Pause while the second call waits for its approval: the
            // first already ran, so the block can't run again.
            pause_at_ask: 2,
            ..Fake::script(vec![two_calls, text("done")])
        };
        host.answers_for_user
            .lock()
            .unwrap()
            .extend([Answer::Allow, Answer::Allow]);
        let (o, _) = go_pausable(&tool_loop(), &host, "go", None).await;
        assert!(matches!(o.status, RunStatus::Paused { .. }), "{o:?}");
        assert_eq!(host.tool_runs.lock().unwrap().len(), 2);

        // Resuming doesn't run either tool again.
        let next = Fake::with(&["done"]);
        let (o, _) = go_pausable(&tool_loop(), &next, "go", Some(saved(&o))).await;
        assert_eq!(o.status, RunStatus::Completed, "{o:?}");
        assert!(next.tool_runs.lock().unwrap().is_empty());
        assert_eq!(o.tool_calls, 2);
    }

    #[tokio::test]
    async fn an_unreachable_model_pauses_and_is_asked_again() {
        let l = two_step_loop();
        let host = Fake {
            unreachable_next: Mutex::new(1),
            ..Default::default()
        };
        let (o, ev) = go_pausable(&l, &host, "hi", None).await;
        assert!(
            matches!(o.status, RunStatus::Paused { reason: Some(ref r) } if r.contains("couldn't reach")),
            "{o:?}"
        );
        assert_eq!(o.llm_calls, 0, "the call that failed isn't counted");
        assert!(
            ev.iter()
                .any(|e| matches!(e, Event::Notice { text, .. } if text.starts_with("Paused")))
        );

        let next = Fake::default();
        let (o, _) = go_pausable(&l, &next, "hi", Some(saved(&o))).await;
        assert_eq!(o.status, RunStatus::Completed, "{o:?}");
        assert_eq!(next.prompts.lock().unwrap()[0], "zero: hi");
        assert_eq!(o.llm_calls, 3);

        // A run that can't pause fails, as before.
        let host = Fake {
            unreachable_next: Mutex::new(1),
            ..Default::default()
        };
        let (o, _) = go(&l, &host, "hi").await;
        assert!(matches!(o.status, RunStatus::Failed { .. }), "{o:?}");
    }

    #[tokio::test]
    async fn a_hurried_pause_drops_the_model_call_and_asks_again() {
        let l = two_step_loop();
        let host = Fake {
            hang_on_call: 2,
            ..Default::default()
        };
        let (o, _) = go_pausable(&l, &host, "hi", None).await;
        assert_eq!(o.status, RunStatus::Paused { reason: None }, "{o:?}");
        assert_eq!(o.llm_calls, 1, "the dropped call isn't counted");
        let dropped = host.prompts.lock().unwrap()[1].clone();

        let next = Fake::default();
        let (o, _) = go_pausable(&l, &next, "hi", Some(saved(&o))).await;
        assert_eq!(o.status, RunStatus::Completed, "{o:?}");
        assert_eq!(next.prompts.lock().unwrap()[0], dropped, "asked again");
        assert_eq!(o.llm_calls, 3);

        // Without the hurry flag, a pause waits for the call to finish.
        let host = Fake {
            pause_after_call: 1,
            ..Default::default()
        };
        let (o, _) = go_pausable(&l, &host, "hi", None).await;
        assert_eq!(o.status, RunStatus::Paused { reason: None }, "{o:?}");
        assert_eq!(o.llm_calls, 1);
    }

    #[tokio::test]
    async fn a_paused_state_must_fit_its_loop() {
        let l = two_step_loop();
        let host = Fake {
            pause_after_call: 2,
            ..Default::default()
        };
        let (o, _) = go_pausable(&l, &host, "hi", None).await;
        let state = saved(&o);

        // Another loop: refused, nothing runs.
        let mut other = l.clone();
        other
            .blocks
            .push(b("extra", "fixed", json!({ "text": "x" })));
        let next = Fake::default();
        let (r, _) = go_pausable(&other, &next, "hi", Some(state.clone())).await;
        assert!(
            matches!(r.status, RunStatus::Failed { ref error, .. } if error.contains("doesn't fit")),
            "{r:?}"
        );
        assert!(next.prompts.lock().unwrap().is_empty());

        // Another version of the state, or one that's been tampered with.
        let mut v = serde_json::to_value(&state).unwrap();
        v["version"] = json!(99);
        let old: RunState = serde_json::from_value(v).unwrap();
        let (r, _) = go_pausable(&l, &Fake::default(), "hi", Some(old)).await;
        assert!(matches!(r.status, RunStatus::Failed { .. }));
        let mut v = serde_json::to_value(&state).unwrap();
        v["frames"][0]["ready"] = json!(["nope"]);
        let bad: RunState = serde_json::from_value(v).unwrap();
        let (r, _) = go_pausable(&l, &Fake::default(), "hi", Some(bad)).await;
        assert!(matches!(r.status, RunStatus::Failed { .. }));
        let mut v = serde_json::to_value(&state).unwrap();
        v["frames"][0]["ready"] = json!(["o", "o", "o"]);
        let bad: RunState = serde_json::from_value(v).unwrap();
        let (r, _) = go_pausable(&l, &Fake::default(), "hi", Some(bad)).await;
        assert!(matches!(r.status, RunStatus::Failed { .. }));
    }
}
