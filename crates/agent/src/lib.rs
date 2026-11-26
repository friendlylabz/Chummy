//! Chummy's agents, shared by the app and chummyd: what an agent is
//! ([`Agent`]), the configuration it runs under ([`AgentConfig`]), and the
//! loop library ([`Library`]).

mod agent;
mod config;
pub mod library;

pub use agent::{Agent, Permissions, ToolPref, Workspace};
pub use config::AgentConfig;
pub use library::{Library, LoopSummary, valid_id};
