//! The block catalog: every block type, its parameters and its ports.

use serde::{Deserialize, Serialize};
use serde_json::{Value as Json, json};

use crate::exchange::Role;

/// What a port carries.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ValueKind {
    Text,
    /// A list of texts.
    List,
    /// A conversation with the model (see [`crate::Exchange`]).
    Exchange,
    /// Anything: passes along whatever it receives.
    Any,
}

impl ValueKind {
    /// Whether a value from `self` may flow into a port expecting `to`.
    pub fn fits(self, to: ValueKind) -> bool {
        self == to || self == ValueKind::Any || to == ValueKind::Any
    }
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct PortSpec {
    pub name: String,
    pub label: String,
    pub kind: ValueKind,
}

/// The name of the output that carries a block's error.
pub const FAILED: &str = "error";

fn failed() -> PortSpec {
    port(FAILED, "Failed", ValueKind::Text)
}

fn port(name: impl Into<String>, label: impl Into<String>, kind: ValueKind) -> PortSpec {
    PortSpec {
        name: name.into(),
        label: label.into(),
        kind,
    }
}

fn d_max_tokens() -> u32 {
    1024
}
fn d_two() -> u32 {
    2
}
fn d_ten() -> u32 {
    10
}
fn d_chunk() -> u32 {
    2000
}
fn d_deviation() -> u32 {
    50
}
fn d_newline() -> String {
    "\n".into()
}
fn d_result_chars() -> u32 {
    4000
}
fn d_roles() -> Vec<Role> {
    vec![Role::User]
}
fn d_user() -> Role {
    Role::User
}
fn d_input() -> String {
    "_INPUT_".into()
}

/// Whether a model block must call a tool.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ToolChoice {
    /// The model decides.
    #[default]
    Auto,
    /// The model must call one of the block's tools.
    Required,
}

/// How a model block offers its tools to the model.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ToolMode {
    /// Through the model server's own tool calling, in the model's own
    /// format. Best when the server supports it.
    #[default]
    Native,
    /// Chummy describes the tools in the system message and reads calls out
    /// of the answer (`<tool_call>{…}</tool_call>`). For servers or models
    /// without tool calling.
    Prompted,
    /// Not offered automatically; the loop handles tools itself (e.g. with
    /// `_TOOLS_` in a prompt and a Find tool call block).
    Off,
}

/// What a model block does when its model has used its daily tokens.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum OnLimit {
    /// Ask the user whether to allow more today.
    #[default]
    Ask,
    /// Wait until the allowance refreshes (at midnight).
    Wait,
    /// Stop the run.
    Stop,
}

/// What a model block does when the model's tool call is wrong (unknown
/// tool, bad arguments, unreadable JSON, or no call when one is required).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BadCall {
    /// Tell the model what was wrong and ask again (up to `retries` times),
    /// then stop the run.
    #[default]
    Retry,
    /// Stop the run.
    Fail,
    /// Use the model's text as its answer and ignore the bad call.
    Answer,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum OnToolError {
    /// Give the error to the model as the tool's result, so it can react.
    #[default]
    TellModel,
    /// Stop the run.
    Fail,
}

/// Which messages a Read exchange block looks at.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RoleFilter {
    Any,
    User,
    #[default]
    Assistant,
    Tool,
    System,
}

impl RoleFilter {
    pub fn matches(self, r: Role) -> bool {
        match self {
            RoleFilter::Any => true,
            RoleFilter::User => r == Role::User,
            RoleFilter::Assistant => r == Role::Assistant,
            RoleFilter::Tool => r == Role::Tool,
            RoleFilter::System => r == Role::System,
        }
    }
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Empty {}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct HistoryParams {
    /// How many recent messages (yours and the agent's) to include.
    #[serde(default = "d_ten")]
    pub max_messages: u32,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FixedParams {
    #[serde(default)]
    pub text: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LlmParams {
    /// The request. `_INPUT_` is replaced by the block's input; if it's
    /// missing, the input is added after the prompt.
    #[serde(default)]
    pub prompt: String,
    /// Optional system message.
    #[serde(default)]
    pub system: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub temperature: Option<f32>,
    #[serde(default = "d_max_tokens")]
    pub max_tokens: u32,
    /// Model connection id. `None` uses the agent's model.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    /// Tools the model may call. With none, the block has no Tool output.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub tools: Vec<String>,
    #[serde(default)]
    pub tool_choice: ToolChoice,
    #[serde(default)]
    pub on_bad_call: BadCall,
    /// How many times to ask again after a bad tool call (0–5).
    #[serde(default = "d_two")]
    pub retries: u32,
    #[serde(default)]
    pub tool_mode: ToolMode,
    /// This block's tools may use the internet (if the app and the agent
    /// also allow it).
    #[serde(default)]
    pub network: bool,
    #[serde(default)]
    pub on_limit: OnLimit,
}

fn d_memory_chars() -> u32 {
    4000
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct StoreParams {
    /// Memory name: letters, digits and `_`. `{{name}}` in prompts reads it.
    #[serde(default)]
    pub key: String,
    /// Longer text is cut (from the start, keeping the newest part).
    #[serde(default = "d_memory_chars")]
    pub max_chars: u32,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RecallParams {
    #[serde(default)]
    pub key: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum OutlineAction {
    /// Start the outline with one task (the goal).
    #[default]
    Start,
    /// Add a list of steps under a task.
    AddSteps,
    /// Set a task's status.
    Mark,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct OutlineParams {
    /// Memory name the rendered outline is saved under (`{{plan}}`).
    #[serde(default = "d_outline_key")]
    pub key: String,
    #[serde(default)]
    pub action: OutlineAction,
    /// For Mark.
    #[serde(default = "d_outline_status")]
    pub status: crate::outline::TaskStatus,
    /// For Add steps: most steps added (0 = all).
    #[serde(default = "d_outline_limit")]
    pub limit: u32,
}

fn d_outline_key() -> String {
    "plan".into()
}
fn d_outline_status() -> crate::outline::TaskStatus {
    crate::outline::TaskStatus::Done
}
fn d_outline_limit() -> u32 {
    5
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FindCallParams {
    /// Tools the text may call.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub tools: Vec<String>,
    /// These tools may use the internet (if the app and the agent also allow it).
    #[serde(default)]
    pub network: bool,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ForEachParams {
    /// Send at most this many items; 0 sends them all.
    #[serde(default)]
    pub limit: u32,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RunToolParams {
    /// Longer results are cut, to keep the model's context small.
    #[serde(default = "d_result_chars")]
    pub max_result_chars: u32,
    #[serde(default)]
    pub on_error: OnToolError,
    /// Ask the user before each call. (Chummy may also ask for some tools
    /// whatever this says; see [`crate::Host::must_confirm`].)
    #[serde(default)]
    pub confirm: bool,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AskUserParams {
    /// The question. `_INPUT_` is replaced by the block's input; if it's
    /// missing, the input is added after it.
    #[serde(default = "d_input")]
    pub prompt: String,
    /// Give up after this many minutes; 0 waits as long as it takes.
    #[serde(default)]
    pub timeout_minutes: u32,
    /// Used when the wait times out. Empty: a timeout stops the run.
    #[serde(default)]
    pub default_answer: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct StartExchangeParams {
    /// Optional system message at the start.
    #[serde(default)]
    pub system: String,
    /// Roles for the input's items, in order, repeating. Text input uses
    /// the first.
    #[serde(default = "d_roles")]
    pub roles: Vec<Role>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AugmentParams {
    #[serde(default = "d_user")]
    pub role: Role,
    /// `_INPUT_` is replaced by the added text.
    #[serde(default = "d_input")]
    pub format: String,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ReadExchangeParams {
    #[serde(default)]
    pub role: RoleFilter,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FunctionParams {
    /// Id of a function in the loop's `functions`.
    #[serde(default)]
    pub function: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CombineParams {
    /// Number of inputs, 1–10.
    #[serde(default = "d_two")]
    pub inputs: u32,
    /// `_INPUT1_`, `_INPUT2_`, … are replaced by the inputs.
    #[serde(default)]
    pub format: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SplitParams {
    #[serde(default = "d_chunk")]
    pub chunk_size: u32,
    #[serde(default)]
    pub overlap: u32,
    /// How far from the ideal cut to look for a newline, period, comma or space.
    #[serde(default = "d_deviation")]
    pub deviation: u32,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SeparateParams {
    /// Number of outputs, 1–10.
    #[serde(default = "d_two")]
    pub outputs: u32,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct JoinParams {
    #[serde(default = "d_newline")]
    pub separator: String,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PatternParams {
    /// A regular expression. Use `(?i)` at the start to ignore case.
    #[serde(default)]
    pub pattern: String,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct WriteFileParams {
    /// Add to the end of the file instead of replacing it.
    #[serde(default)]
    pub append: bool,
}

/// A block type with its parameters.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", content = "params", rename_all = "snake_case")]
pub enum BlockKind {
    Input(Empty),
    History(HistoryParams),
    Fixed(FixedParams),
    Output(Empty),
    Llm(LlmParams),
    Combine(CombineParams),
    Forward(Empty),
    Remember(Empty),
    Split(SplitParams),
    ForEach(ForEachParams),
    Separate(SeparateParams),
    Join(JoinParams),
    Regex(PatternParams),
    Capture(PatternParams),
    Branch(PatternParams),
    ReadFile(Empty),
    WriteFile(WriteFileParams),
    ListFiles(Empty),
    RunTool(RunToolParams),
    AskUser(AskUserParams),
    DeclineCalls(Empty),
    FindCall(FindCallParams),
    Store(StoreParams),
    Recall(RecallParams),
    Outline(OutlineParams),
    Exchange(StartExchangeParams),
    Augment(AugmentParams),
    ReadExchange(ReadExchangeParams),
    /// A block made of other blocks. Its ports come from the function, so
    /// [`BlockKind::inputs`] and [`BlockKind::outputs`] are empty here; see
    /// [`crate::analyze`].
    Function(FunctionParams),
}

impl BlockKind {
    /// Parses a block's `type` and `params`.
    pub fn parse(kind: &str, params: &Json) -> Result<Self, String> {
        if !catalog().iter().any(|b| b.kind == kind) {
            return Err(format!("unknown block type “{kind}”"));
        }
        let params = if params.is_null() {
            json!({})
        } else {
            params.clone()
        };
        serde_json::from_value(json!({ "type": kind, "params": params }))
            .map_err(|e| format!("bad parameters for {kind}: {e}"))
    }

    pub fn inputs(&self) -> Vec<PortSpec> {
        use BlockKind::*;
        use ValueKind::*;
        match self {
            Input(_) | History(_) | Fixed(_) | Function(_) => vec![],
            Output(_) | Split(_) | Regex(_) | Capture(_) => vec![port("in", "In", Text)],
            Llm(_) | AskUser(_) => vec![port("in", "In", Any)],
            DeclineCalls(_) => vec![
                port("in", "Tool call", ValueKind::Exchange),
                port("reason", "Reason", Text),
            ],
            FindCall(_) => vec![
                port("exchange", "Exchange", ValueKind::Exchange),
                port("in", "Text", Text),
            ],
            Store(_) => vec![port("in", "In", Text)],
            Recall(_) => vec![port("in", "Trigger", Any)],
            Outline(p) => match p.action {
                OutlineAction::Start => vec![port("in", "Goal", Text)],
                OutlineAction::AddSteps => {
                    vec![port("task", "Task", Text), port("steps", "Steps", List)]
                }
                OutlineAction::Mark => vec![port("task", "Task", Text)],
            },
            RunTool(_) => vec![port("in", "Tool call", ValueKind::Exchange)],
            BlockKind::Exchange(_) => vec![port("in", "In", Any)],
            Augment(_) => vec![
                port("exchange", "Exchange", ValueKind::Exchange),
                port("in", "Add", Any),
            ],
            ReadExchange(_) => vec![port("in", "Exchange", ValueKind::Exchange)],
            Forward(_) | Remember(_) => vec![port("in", "In", Any)],
            ForEach(_) | Separate(_) | Join(_) => vec![port("in", "List", List)],
            Combine(p) => (1..=p.inputs.clamp(1, 10))
                .map(|i| port(format!("in{i}"), format!("In {i}"), Text))
                .collect(),
            Branch(_) => vec![port("value", "Value", Any), port("test", "Test", Text)],
            ReadFile(_) | ListFiles(_) => vec![port("path", "Path", Text)],
            WriteFile(_) => vec![port("path", "Path", Text), port("content", "Content", Text)],
        }
    }

    pub fn outputs(&self) -> Vec<PortSpec> {
        use BlockKind::*;
        use ValueKind::*;
        match self {
            Output(_) | Function(_) => vec![],
            Llm(p) => {
                let mut v = vec![port("out", "Answer", Text)];
                if !p.tools.is_empty() && p.tool_mode != ToolMode::Off {
                    v.push(port("tool", "Tool call", ValueKind::Exchange));
                }
                v.push(port("reasoning", "Reasoning", Text));
                v.push(failed());
                v
            }
            RunTool(_) => vec![port("out", "Results", ValueKind::Exchange), failed()],
            AskUser(_) => vec![port("out", "Answer", Text), failed()],
            DeclineCalls(_) => vec![port("out", "Exchange", ValueKind::Exchange)],
            FindCall(_) => vec![
                port("call", "Tool call", ValueKind::Exchange),
                port("none", "No call", Text),
            ],
            Store(_) | Recall(_) => vec![port("out", "Out", Text)],
            Outline(_) => vec![port("out", "Outline", Text)],
            BlockKind::Exchange(_) | Augment(_) => {
                vec![port("out", "Exchange", ValueKind::Exchange)]
            }
            ReadExchange(_) => vec![
                port("last", "Last message", Text),
                port("all", "Transcript", Text),
                port("calls", "Tool calls", Text),
            ],
            Input(_) | History(_) | Fixed(_) | Combine(_) | ForEach(_) | Join(_) => {
                vec![port("out", "Out", Text)]
            }
            ReadFile(_) => vec![port("out", "Out", Text), failed()],
            WriteFile(_) => vec![port("out", "Path", Text), failed()],
            Forward(_) | Remember(_) => vec![port("out", "Out", Any)],
            Split(_) | Regex(_) => vec![port("out", "List", List)],
            ListFiles(_) => vec![port("out", "List", List), failed()],
            Separate(p) => (1..=p.outputs.clamp(1, 10))
                .map(|i| port(format!("out{i}"), format!("Item {i}"), Text))
                .collect(),
            Capture(p) => {
                let groups = regex::Regex::new(&p.pattern)
                    .map(|r| r.captures_len() - 1)
                    .unwrap_or(0);
                (1..=groups.min(10))
                    .map(|i| port(format!("group{i}"), format!("Group {i}"), List))
                    .collect()
            }
            Branch(_) => vec![port("yes", "Matches", Any), port("no", "Doesn't", Any)],
        }
    }

    /// Whether this block has a Failed output, which carries the error
    /// message instead of stopping the run (when something is linked to it).
    pub fn can_fail_softly(&self) -> bool {
        self.outputs().iter().any(|p| p.name == FAILED)
    }

    pub fn type_name(&self) -> &'static str {
        use BlockKind::*;
        match self {
            Input(_) => "input",
            History(_) => "history",
            Fixed(_) => "fixed",
            Output(_) => "output",
            Llm(_) => "llm",
            Combine(_) => "combine",
            Forward(_) => "forward",
            Remember(_) => "remember",
            Split(_) => "split",
            ForEach(_) => "for_each",
            Separate(_) => "separate",
            Join(_) => "join",
            Regex(_) => "regex",
            Capture(_) => "capture",
            Branch(_) => "branch",
            ReadFile(_) => "read_file",
            WriteFile(_) => "write_file",
            ListFiles(_) => "list_files",
            RunTool(_) => "run_tool",
            AskUser(_) => "ask_user",
            DeclineCalls(_) => "decline_calls",
            FindCall(_) => "find_call",
            Store(_) => "store",
            Recall(_) => "recall",
            Outline(_) => "outline",
            Exchange(_) => "exchange",
            Augment(_) => "augment",
            ReadExchange(_) => "read_exchange",
            Function(_) => "function",
        }
    }
}

// ------------------------------------------------------------------ catalog

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Category {
    Start,
    Think,
    /// Blocks that wait for the person using Chummy.
    You,
    /// Remembering text for the rest of the run.
    Memory,
    Exchange,
    Text,
    Flow,
    Workspace,
    Finish,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum ParamKind {
    Text,
    Multiline,
    Number {
        min: f64,
        max: f64,
        step: f64,
    },
    Integer {
        min: i64,
        max: i64,
    },
    Bool,
    /// A model connection id, or empty for the agent's model.
    Model,
    /// One of a fixed set of values.
    Choice {
        options: Vec<ChoiceOption>,
    },
    /// A list of message roles.
    Roles,
    /// A list of installed tool names.
    Tools,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct ChoiceOption {
    pub value: &'static str,
    pub label: &'static str,
}

fn choice(options: &[(&'static str, &'static str)]) -> ParamKind {
    ParamKind::Choice {
        options: options
            .iter()
            .map(|(value, label)| ChoiceOption { value, label })
            .collect(),
    }
}

const ROLE_OPTIONS: &[(&str, &str)] = &[
    ("user", "User"),
    ("assistant", "Assistant"),
    ("system", "System"),
    ("tool", "Tool"),
];

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct ParamSpec {
    pub key: &'static str,
    pub label: &'static str,
    #[serde(flatten)]
    pub kind: ParamKind,
    pub help: &'static str,
    /// May be left empty (unset).
    pub optional: bool,
}

/// What the editor shows about a block type.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct BlockInfo {
    pub kind: &'static str,
    pub label: &'static str,
    pub category: Category,
    pub description: &'static str,
    pub params: Vec<ParamSpec>,
    pub default_params: Json,
    /// Shown in the editor's palette.
    pub palette: bool,
}

fn p(key: &'static str, label: &'static str, kind: ParamKind, help: &'static str) -> ParamSpec {
    ParamSpec {
        key,
        label,
        kind,
        help,
        optional: false,
    }
}

pub fn catalog() -> Vec<BlockInfo> {
    use Category::*;
    use ParamKind as K;
    let info =
        |kind, label, category, description, params: Vec<ParamSpec>, default_params| BlockInfo {
            kind,
            label,
            category,
            description,
            params,
            default_params,
            palette: true,
        };
    vec![
        info(
            "input",
            "Message",
            Start,
            "Starts the loop with the message you send in chat.",
            vec![],
            json!({}),
        ),
        info(
            "history",
            "Conversation",
            Start,
            "Starts the loop with the recent conversation as text (“User: … / Assistant: …”), not including the new message.",
            vec![p(
                "max_messages",
                "Messages to include",
                K::Integer { min: 1, max: 100 },
                "Most recent first.",
            )],
            json!({ "max_messages": 10 }),
        ),
        info(
            "fixed",
            "Fixed text",
            Start,
            "Starts the loop with text you write here.",
            vec![p("text", "Text", K::Multiline, "")],
            json!({ "text": "" }),
        ),
        info(
            "llm",
            "Ask the model",
            Think,
            "Asks the model. Give it text (with your prompt around it) or an exchange (which it continues). Its answer goes out of Answer; if it calls one of its tools, the exchange goes out of Tool call instead and Answer stays silent. Reasoning carries any thinking the model shows. If the call fails, the error goes out of Failed when something is linked to it (otherwise the run stops).",
            vec![
                p(
                    "prompt",
                    "Prompt",
                    K::Multiline,
                    "Use _INPUT_ for the block's input. Short, specific prompts work best with small models. Not used when the input is an exchange.",
                ),
                ParamSpec {
                    optional: true,
                    ..p(
                        "system",
                        "System message",
                        K::Multiline,
                        "Optional instructions sent as the system message.",
                    )
                },
                ParamSpec {
                    optional: true,
                    ..p(
                        "model",
                        "Model",
                        K::Model,
                        "Leave on the agent's model unless this step needs a different one.",
                    )
                },
                ParamSpec {
                    optional: true,
                    ..p(
                        "temperature",
                        "Temperature",
                        K::Number {
                            min: 0.0,
                            max: 2.0,
                            step: 0.05,
                        },
                        "Leave empty for the model's default.",
                    )
                },
                p(
                    "max_tokens",
                    "Max answer length (tokens)",
                    K::Integer { min: 1, max: 32768 },
                    "",
                ),
                ParamSpec {
                    optional: true,
                    ..p(
                        "tools",
                        "Tools",
                        K::Tools,
                        "Tools this step may call. Fewer tools means a shorter prompt and fewer mistakes.",
                    )
                },
                p(
                    "tool_choice",
                    "Tool use",
                    choice(&[("auto", "Model decides"), ("required", "Must call a tool")]),
                    "",
                ),
                p(
                    "on_bad_call",
                    "If a tool call is wrong",
                    choice(&[
                        ("retry", "Explain and ask again"),
                        ("fail", "Stop the run"),
                        ("answer", "Use the text as the answer"),
                    ]),
                    "Wrong means an unknown tool, bad arguments, or no call when one is required.",
                ),
                p(
                    "retries",
                    "Times to ask again",
                    K::Integer { min: 0, max: 5 },
                    "",
                ),
                p(
                    "tool_mode",
                    "How tools are offered",
                    choice(&[
                        ("native", "Native (the model server's tool calling)"),
                        ("prompted", "Prompted (described in the system message)"),
                        ("off", "Off (the loop handles tools)"),
                    ]),
                    "Native suits most servers. Prompted works with models or servers that can't call tools. Off leaves tools to the loop: use _TOOLS_ in a prompt and a Find tool call block.",
                ),
                p(
                    "network",
                    "Tools may use the internet",
                    K::Bool,
                    "Also needs Internet access in Settings → Permissions, the tool's own internet switch, and the agent's.",
                ),
                p(
                    "on_limit",
                    "When the model's daily tokens run out",
                    choice(&[
                        ("ask", "Ask me for more"),
                        ("wait", "Wait until tomorrow"),
                        ("stop", "Stop the run"),
                    ]),
                    "Each model connection has a daily token limit (Settings → Models).",
                ),
            ],
            json!({ "prompt": "_INPUT_", "max_tokens": 1024, "tool_mode": "native", "network": false, "on_limit": "ask" }),
        ),
        info(
            "run_tool",
            "Run tool",
            Think,
            "Runs the tool calls at the end of an exchange, in the agent's sandbox, and adds their results. Link it back to the model block so the model can use them.",
            vec![
                p(
                    "max_result_chars",
                    "Longest result (characters)",
                    K::Integer {
                        min: 100,
                        max: 100000,
                    },
                    "Longer results are cut, to keep the conversation short.",
                ),
                p(
                    "on_error",
                    "If a tool fails",
                    choice(&[("tell_model", "Tell the model"), ("fail", "Stop the run")]),
                    "",
                ),
                p(
                    "confirm",
                    "Ask me before each call",
                    K::Bool,
                    "Each call waits until you allow it in chat. Chummy always asks for tools that use the internet unless you turned that off in Settings → Tools.",
                ),
            ],
            json!({ "max_result_chars": 4000, "on_error": "tell_model", "confirm": false }),
        ),
        info(
            "ask_user",
            "Ask me",
            You,
            "Asks you a question in chat and waits for your answer, which goes out of Answer. Scheduled jobs wait too, and Chummy lets you know.",
            vec![
                p(
                    "prompt",
                    "Question",
                    K::Multiline,
                    "Use _INPUT_ for the block's input.",
                ),
                p(
                    "timeout_minutes",
                    "Give up after (minutes, 0 = never)",
                    K::Integer { min: 0, max: 10080 },
                    "",
                ),
                ParamSpec {
                    optional: true,
                    ..p(
                        "default_answer",
                        "Answer if nobody replies in time",
                        K::Text,
                        "Leave empty to stop the run instead.",
                    )
                },
            ],
            json!({ "prompt": "_INPUT_", "timeout_minutes": 0, "default_answer": "" }),
        ),
        info(
            "decline_calls",
            "Decline tool calls",
            Think,
            "Answers the pending tool calls at the end of an exchange with “Not run” and the Reason, without running them. Use it when a check says a call is wrong; link it back to the model block.",
            vec![],
            json!({}),
        ),
        info(
            "find_call",
            "Find tool call",
            Think,
            "For loops that handle tools themselves: finds a tool call in the Text (<tool_call>{\"name\": …, \"arguments\": {…}}</tool_call>, or the same JSON on its own) and adds it to the exchange for Run tool. Text without a call goes out of No call.",
            vec![
                ParamSpec {
                    optional: true,
                    ..p("tools", "Tools", K::Tools, "Tools the text may call.")
                },
                p(
                    "network",
                    "Tools may use the internet",
                    K::Bool,
                    "Also needs Internet access in Settings → Permissions, the tool's own internet switch, and the agent's.",
                ),
            ],
            json!({ "network": false }),
        ),
        info(
            "store",
            "Save to memory",
            Memory,
            "Saves its input under a name for the rest of the run, and passes it on. Any prompt can include it as {{name}}.",
            vec![
                p("key", "Name", K::Text, "Letters, digits and _, e.g. notes."),
                p(
                    "max_chars",
                    "Longest (characters)",
                    K::Integer {
                        min: 100,
                        max: 100000,
                    },
                    "Longer text keeps its end.",
                ),
            ],
            json!({ "key": "notes", "max_chars": 4000 }),
        ),
        info(
            "recall",
            "Read memory",
            Memory,
            "When triggered, sends out the text saved under a name (empty if nothing is saved yet).",
            vec![p("key", "Name", K::Text, "")],
            json!({ "key": "notes" }),
        ),
        info(
            "outline",
            "Plan outline",
            Memory,
            "Keeps the whole plan as a numbered, indented outline for the rest of the run: start it with the goal, add steps under a task, and mark tasks done or failed. Sends out the whole outline each time, and saves it to memory ({{plan}}).",
            vec![
                p(
                    "action",
                    "Action",
                    choice(&[
                        ("start", "Start with the goal"),
                        ("add_steps", "Add steps under a task"),
                        ("mark", "Mark a task"),
                    ]),
                    "Tasks are found by their text (a leading “Task:” is ignored).",
                ),
                p(
                    "status",
                    "Mark as",
                    choice(&[
                        ("done", "Done ✓"),
                        ("failed", "Failed ✗"),
                        ("working", "Working ▸"),
                        ("pending", "To do ○"),
                    ]),
                    "For Mark a task.",
                ),
                p(
                    "limit",
                    "Most steps",
                    K::Integer { min: 0, max: 100 },
                    "For Add steps. 0 adds all; match For each's limit.",
                ),
                p(
                    "key",
                    "Memory name",
                    K::Text,
                    "Read anywhere as {{plan}} (or this name).",
                ),
            ],
            json!({ "action": "start", "status": "done", "limit": 5, "key": "plan" }),
        ),
        info(
            "exchange",
            "Start exchange",
            Exchange,
            "Starts a conversation with the model from text (one message) or a list (one message per item, with roles in turn).",
            vec![
                ParamSpec {
                    optional: true,
                    ..p("system", "System message", K::Multiline, "")
                },
                p(
                    "roles",
                    "Roles",
                    K::Roles,
                    "For a list: the role of item 1, item 2, …, repeating.",
                ),
            ],
            json!({ "system": "", "roles": ["user"] }),
        ),
        info(
            "augment",
            "Add to exchange",
            Exchange,
            "Waits for an exchange and some text, then adds the text as a new message. A list adds one message per item.",
            vec![
                p("role", "Role", choice(ROLE_OPTIONS), ""),
                p(
                    "format",
                    "Message",
                    K::Multiline,
                    "_INPUT_ is replaced by the added text.",
                ),
            ],
            json!({ "role": "user", "format": "_INPUT_" }),
        ),
        info(
            "read_exchange",
            "Read exchange",
            Exchange,
            "Gets text out of an exchange: the last message (from a chosen role), the whole transcript, and the tool calls waiting at its end (for checking them before they run).",
            vec![p(
                "role",
                "Last message from",
                choice(&[
                    ("assistant", "Assistant"),
                    ("user", "User"),
                    ("tool", "Tool"),
                    ("system", "System"),
                    ("any", "Anyone"),
                ]),
                "",
            )],
            json!({ "role": "assistant" }),
        ),
        info(
            "combine",
            "Combine",
            Text,
            "Waits for all of its inputs, then fills them into a template: _INPUT1_, _INPUT2_, …",
            vec![
                p(
                    "inputs",
                    "Number of inputs",
                    K::Integer { min: 1, max: 10 },
                    "",
                ),
                p(
                    "format",
                    "Template",
                    K::Multiline,
                    "Use _INPUT1_, _INPUT2_, … for the inputs.",
                ),
            ],
            json!({ "inputs": 2, "format": "_INPUT1_\n\n_INPUT2_" }),
        ),
        info(
            "split",
            "Split into chunks",
            Text,
            "Cuts long text into overlapping chunks, breaking at newlines, periods, commas or spaces where it can. Outputs a list.",
            vec![
                p(
                    "chunk_size",
                    "Chunk size (characters)",
                    K::Integer {
                        min: 100,
                        max: 100000,
                    },
                    "",
                ),
                p(
                    "overlap",
                    "Overlap (characters)",
                    K::Integer { min: 0, max: 10000 },
                    "",
                ),
                p(
                    "deviation",
                    "Search distance for a clean break",
                    K::Integer { min: 0, max: 1000 },
                    "",
                ),
            ],
            json!({ "chunk_size": 2000, "overlap": 0, "deviation": 50 }),
        ),
        info(
            "join",
            "Join list",
            Text,
            "Turns a list back into one text.",
            vec![p(
                "separator",
                "Separator",
                K::Text,
                "Put between items. \\n is a new line.",
            )],
            json!({ "separator": "\n" }),
        ),
        info(
            "regex",
            "Find matches",
            Text,
            "Outputs a list of every match of a regular expression.",
            vec![p(
                "pattern",
                "Regular expression",
                K::Text,
                "Start with (?i) to ignore case.",
            )],
            json!({ "pattern": "" }),
        ),
        info(
            "capture",
            "Capture groups",
            Text,
            "Matches a regular expression and gives each capture group its own output: a list of that group's text in every match.",
            vec![p(
                "pattern",
                "Regular expression",
                K::Text,
                "Each ( ) group becomes an output.",
            )],
            json!({ "pattern": "" }),
        ),
        info(
            "branch",
            "Branch",
            Flow,
            "Waits for a Value and a Test. If the Test text matches the pattern, the Value goes out of Matches; otherwise out of Doesn't. Use it to end a loop or retry.",
            vec![p(
                "pattern",
                "Pattern (regular expression)",
                K::Text,
                "e.g. (?i)^\\s*yes",
            )],
            json!({ "pattern": "(?i)^\\s*yes" }),
        ),
        info(
            "forward",
            "Pass through",
            Flow,
            "Passes its input on unchanged. Handy for tidying links or joining paths.",
            vec![],
            json!({}),
        ),
        info(
            "remember",
            "Remember first",
            Flow,
            "Keeps the first input it gets and sends that same value every time it's triggered again.",
            vec![],
            json!({}),
        ),
        info(
            "for_each",
            "For each",
            Flow,
            "Sends each item of a list on separately, one after another.",
            vec![p(
                "limit",
                "At most (0 = all)",
                K::Integer { min: 0, max: 1000 },
                "",
            )],
            json!({ "limit": 0 }),
        ),
        info(
            "separate",
            "Pick items",
            Flow,
            "Sends item 1 of a list out of output 1, item 2 out of output 2, and so on.",
            vec![p(
                "outputs",
                "Number of outputs",
                K::Integer { min: 1, max: 10 },
                "",
            )],
            json!({ "outputs": 2 }),
        ),
        info(
            "read_file",
            "Read file",
            Workspace,
            "Reads a text file from the agent's workspace. The input is a path inside the workspace.",
            vec![],
            json!({}),
        ),
        info(
            "write_file",
            "Write file",
            Workspace,
            "Writes Content to a file in the agent's workspace, creating folders as needed. Outputs the path.",
            vec![p(
                "append",
                "Add to the end instead of replacing",
                K::Bool,
                "",
            )],
            json!({ "append": false }),
        ),
        info(
            "list_files",
            "List files",
            Workspace,
            "Lists a folder in the agent's workspace. Use . for the workspace itself. Folders end with /.",
            vec![],
            json!({}),
        ),
        info(
            "output",
            "Reply",
            Finish,
            "Sends its input to the chat as the agent's reply. If it runs more than once, each part is added to the reply.",
            vec![],
            json!({}),
        ),
        BlockInfo {
            palette: false,
            ..info(
                "function",
                "Function",
                Flow,
                "A block made of other blocks. It waits for all its inputs, runs its blocks, and sends out what reaches its outputs.",
                vec![],
                json!({ "function": "" }),
            )
        },
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_catalog_entry_parses_with_its_defaults() {
        for b in catalog() {
            let kind =
                BlockKind::parse(b.kind, &b.default_params).unwrap_or_else(|e| panic!("{e}"));
            assert_eq!(kind.type_name(), b.kind);
            let roundtrip = serde_json::to_value(&kind).unwrap();
            assert_eq!(roundtrip["type"], b.kind);
        }
    }

    #[test]
    fn params_are_strict_and_defaulted() {
        let k = BlockKind::parse("llm", &json!({ "prompt": "hi" })).unwrap();
        assert!(matches!(
            k,
            BlockKind::Llm(LlmParams {
                max_tokens: 1024,
                ..
            })
        ));
        assert!(BlockKind::parse("llm", &json!({ "promt": "typo" })).is_err());
        assert!(BlockKind::parse("nope", &json!({})).is_err());
    }

    #[test]
    fn ports_follow_params() {
        let c = BlockKind::parse("combine", &json!({ "inputs": 3 })).unwrap();
        assert_eq!(
            c.inputs()
                .iter()
                .map(|p| p.name.as_str())
                .collect::<Vec<_>>(),
            ["in1", "in2", "in3"]
        );
        let cap = BlockKind::parse("capture", &json!({ "pattern": "(\\w+)=(\\d+)" })).unwrap();
        assert_eq!(cap.outputs().len(), 2);
        assert!(ValueKind::Any.fits(ValueKind::List) && !ValueKind::Text.fits(ValueKind::List));
    }
}
