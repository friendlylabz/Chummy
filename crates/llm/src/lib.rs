//! # chummy-llm
//!
//! LLM backends, configured entirely in the app.
//!
//! One OpenAI-compatible adapter covers OpenAI, Ollama, llama.cpp, LM Studio,
//! vLLM and OpenRouter. Anthropic and Gemini get native adapters. Every adapter
//! streams and normalizes tool calls into one internal format, so the loop engine
//! never sees provider differences.
//!
//! Status: pass 1 defines configuration types. Adapters arrive in pass 2.

use serde::{Deserialize, Serialize};

/// Which wire protocol a provider speaks.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ProviderKind {
    /// Any server implementing the OpenAI Chat Completions API.
    OpenAiCompatible,
    OpenAi,
    Ollama,
    Anthropic,
    Gemini,
}

impl ProviderKind {
    /// Sensible default base URL, if the provider has one.
    pub fn default_base_url(self) -> Option<&'static str> {
        match self {
            ProviderKind::OpenAiCompatible => None,
            ProviderKind::OpenAi => Some("https://api.openai.com/v1"),
            ProviderKind::Ollama => Some("http://localhost:11434/v1"),
            ProviderKind::Anthropic => Some("https://api.anthropic.com"),
            ProviderKind::Gemini => Some("https://generativelanguage.googleapis.com"),
        }
    }
}

/// A configured model endpoint. API keys are never stored here; they live in
/// the OS keychain and are looked up by `id`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProviderConfig {
    /// Stable identifier, also used as the keychain entry name.
    pub id: String,
    /// Display name shown in the UI.
    pub name: String,
    pub kind: ProviderKind,
    pub base_url: String,
    /// Model name sent to the endpoint, e.g. `llama3.2:3b`.
    pub model: String,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn provider_config_round_trips_as_json() {
        let cfg = ProviderConfig {
            id: "local".into(),
            name: "Local Ollama".into(),
            kind: ProviderKind::Ollama,
            base_url: ProviderKind::Ollama.default_base_url().unwrap().into(),
            model: "llama3.2:3b".into(),
        };
        let json = serde_json::to_string(&cfg).unwrap();
        assert!(json.contains("\"kind\":\"ollama\""));
        let back: ProviderConfig = serde_json::from_str(&json).unwrap();
        assert_eq!(cfg, back);
    }
}
