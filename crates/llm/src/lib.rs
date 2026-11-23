//! # chummy-llm
//!
//! One pipeline from any model endpoint to the loop engine.
//!
//! ```text
//! ProviderConfig ──► Client ──► adapter (OpenAI-compatible | Anthropic | Gemini)
//!                                  │  HTTP + server-sent events
//!                                  ▼
//!                     ChatEvent stream: Text · Reasoning · ToolCall · MalformedToolCall · Done
//! ```
//!
//! Every adapter turns its provider's wire format into the same [`ChatEvent`]s,
//! so nothing downstream knows which provider is in use. Tool-call arguments
//! are always fully assembled and parsed before they are emitted. Arguments a
//! model got wrong become [`ChatEvent::MalformedToolCall`] (with the raw text)
//! instead of an error, so a verification step can ask the model to fix them.
//!
//! The OpenAI-compatible adapter covers OpenAI, Ollama, LM Studio, llama.cpp,
//! vLLM, OpenRouter, Groq, Mistral, DeepSeek, xAI and Chummy's own built-in
//! engine.
//!
//! Model connections are made by the app on the user's behalf and are separate
//! from an agent's network permission, which stays off unless granted.

mod anthropic;
mod client;
mod config;
mod context;
mod error;
mod gemini;
mod openai;
mod probe;
mod sse;
mod types;

pub use client::Client;
pub use config::{
    DEFAULT_COMPACT_PERCENT, DEFAULT_CONTEXT, DEFAULT_DAILY_TOKENS, DEFAULT_MAX_TOKENS, LocalModel,
    MIN_CONTEXT, Precision, Preset, ProviderConfig, ProviderKind, endpoint_problem, presets,
    unencrypted_remote,
};
pub use error::LlmError;
pub use probe::{ProbeReport, ProbeStep, StepStatus, probe};
pub use types::{
    ChatEvent, ChatRequest, ChatResponse, Message, Role, StopReason, ToolCall, ToolChoice,
    ToolSpec, Usage,
};
