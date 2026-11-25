//! # chummy-flow
//!
//! Agent loops: the file format, the block catalog, validation, and the
//! engine that runs them.
//!
//! A loop is a directed graph of **blocks**. Text flows along **links** from a
//! block's output ports to other blocks' input ports. A block runs once every
//! one of its input ports has a value waiting, taking one value from each, and
//! sends its results to everything linked to its outputs. A port with several
//! incoming links queues their values in arrival order, so a block in a cycle
//! runs again each time new input arrives.
//!
//! See `docs/dev/file-formats.md` (Loops) for the file format.

mod blocks;
mod config;
mod engine;
mod exchange;
mod format;
pub mod log;
pub mod outline;
pub mod prompted;
mod review;
pub use review::{
    AuthorText, BlockRef, Capability, Flag, FlagLevel, Flow, FlowInput, FlowStep, HiddenChar,
    LoopReview, ModelReview, Origin, review,
};
mod templates;
mod text;
pub use text::strip_thinking;
mod validate;

pub use blocks::{
    BadCall, BlockInfo, BlockKind, Category, ChoiceOption, FAILED, OnLimit, OnToolError, ParamKind,
    ParamSpec, PortSpec, RoleFilter, Switch, ToolChoice, ToolMode, ToolSwitches, ValueKind,
    catalog,
};
pub use config::{CONFIG_FORMAT, LoopConfig, Overrides, find_block};
pub use engine::{
    Answer, Ask, AskKind, BlockState, Budget, ChatTurn, ContextLimits, DEFAULT_CONTEXT_WINDOW,
    Event, Host, LlmAnswer, LlmCall, MalformedCall, RunInputs, RunLimits, RunOutcome, RunStatus,
    ToolOutput, TurnUsage, Value, run,
};
pub use exchange::{
    ExMessage, Exchange, Role, ToolCall, ToolInfo, check_arguments, estimate_messages,
};
pub use format::{
    Block, FORMAT, FunctionDef, FunctionInput, FunctionOutput, Link, LoopFile, Note, PortRef,
    Position, VERSION,
};
pub use log::{LoggedValue, ModelLog, PortLog, StepLog, StepStatus};
pub use templates::{ASSISTANT, TERMINAL, Template, builtin_prints, fingerprint, templates};
pub use validate::{Analysis, BlockPorts, FunctionPorts, Issue, Severity, analyze};

#[derive(Debug, thiserror::Error)]
pub enum FlowError {
    #[error("this isn't a valid loop file: {0}")]
    Parse(String),
    #[error("this isn't a Chummy loop file (format is “{0}”)")]
    WrongFormat(String),
    #[error("this loop was made with a newer version of Chummy (format version {0})")]
    UnsupportedVersion(u32),
}
