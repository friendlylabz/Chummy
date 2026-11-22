//! The block catalog: every block type, its parameters and its ports.

use serde::{Deserialize, Serialize};
use serde_json::{Value as Json, json};

/// What a port carries.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ValueKind {
    Text,
    /// A list of texts.
    List,
    /// Text or list: passes along whatever it receives.
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
    ForEach(Empty),
    Separate(SeparateParams),
    Join(JoinParams),
    Regex(PatternParams),
    Capture(PatternParams),
    Branch(PatternParams),
    ReadFile(Empty),
    WriteFile(WriteFileParams),
    ListFiles(Empty),
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
            Input(_) | History(_) | Fixed(_) => vec![],
            Output(_) | Llm(_) | Split(_) | Regex(_) | Capture(_) => vec![port("in", "In", Text)],
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
            Output(_) => vec![],
            Input(_) | History(_) | Fixed(_) | Llm(_) | Combine(_) | ForEach(_) | Join(_)
            | ReadFile(_) => vec![port("out", "Out", Text)],
            WriteFile(_) => vec![port("out", "Path", Text)],
            Forward(_) | Remember(_) => vec![port("out", "Out", Any)],
            Split(_) | Regex(_) | ListFiles(_) => vec![port("out", "List", List)],
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
        }
    }
}

// ------------------------------------------------------------------ catalog

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Category {
    Start,
    Think,
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
}

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
            "Sends a prompt to the model and passes on its answer. Write _INPUT_ where the input should go; without it, the input is added after the prompt.",
            vec![
                p(
                    "prompt",
                    "Prompt",
                    K::Multiline,
                    "Use _INPUT_ for the block's input. Short, specific prompts work best with small models.",
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
            ],
            json!({ "prompt": "_INPUT_", "max_tokens": 1024 }),
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
            vec![],
            json!({}),
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
