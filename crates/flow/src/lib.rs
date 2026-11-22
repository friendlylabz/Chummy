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
//! See `docs/loop-format.md` for the file format.

mod blocks;
mod engine;
mod exchange;
mod format;
mod templates;
mod text;
mod validate;

pub use blocks::{
    BadCall, BlockInfo, BlockKind, Category, ChoiceOption, OnToolError, ParamKind, ParamSpec,
    PortSpec, RoleFilter, ToolChoice, ValueKind, catalog,
};
pub use engine::{
    Answer, Ask, AskKind, BlockState, ChatTurn, Event, Host, LlmAnswer, LlmCall, MalformedCall,
    RunInputs, RunLimits, RunOutcome, RunStatus, ToolOutput, Value, run,
};
pub use exchange::{ExMessage, Exchange, Role, ToolCall, ToolInfo, check_arguments};
pub use format::{
    Block, FORMAT, FunctionDef, FunctionInput, FunctionOutput, Link, LoopFile, PortRef, Position,
    VERSION,
};
pub use templates::{Template, templates};
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
