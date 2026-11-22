//! Provider configuration and presets.

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
    /// Chummy's built-in engine (OpenAI-compatible, served locally).
    Local,
}

impl ProviderKind {
    pub fn default_base_url(self) -> Option<&'static str> {
        match self {
            ProviderKind::OpenAiCompatible | ProviderKind::Local => None,
            ProviderKind::OpenAi => Some("https://api.openai.com/v1"),
            ProviderKind::Ollama => Some("http://localhost:11434/v1"),
            ProviderKind::Anthropic => Some("https://api.anthropic.com"),
            ProviderKind::Gemini => Some("https://generativelanguage.googleapis.com"),
        }
    }

    pub(crate) fn speaks_openai(self) -> bool {
        matches!(
            self,
            ProviderKind::OpenAiCompatible
                | ProviderKind::OpenAi
                | ProviderKind::Ollama
                | ProviderKind::Local
        )
    }
}

/// Weight precision for the built-in engine.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Precision {
    /// bf16 or fp16, whichever the hardware supports.
    #[default]
    Half,
    /// 8-bit weights (bitsandbytes, NVIDIA GPUs only).
    Int8,
    /// 4-bit weights (bitsandbytes, NVIDIA GPUs only).
    Int4,
}

/// A model downloaded for the built-in engine.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LocalModel {
    /// Hugging Face repo id, e.g. `Qwen/Qwen2.5-1.5B-Instruct`.
    pub repo: String,
    /// Commit the files were downloaded from.
    pub revision: String,
    /// Folder name inside the Chummy folder's `models/`.
    pub dir_name: String,
    /// Tool-call format used by the model (`hermes`, `llama3_json`, …).
    pub parser: String,
    #[serde(default)]
    pub precision: Precision,
}

/// A configured model endpoint. API keys are never stored here; they live in
/// the OS keychain under [`ProviderConfig::secret_key`].
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ProviderConfig {
    /// Stable identifier.
    pub id: String,
    /// Display name.
    pub name: String,
    pub kind: ProviderKind,
    /// Base URL. Ignored for [`ProviderKind::Local`].
    #[serde(default)]
    pub base_url: String,
    /// Model name sent to the endpoint.
    pub model: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub local: Option<LocalModel>,
    /// Sampling temperature to use unless a loop step overrides it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub temperature: Option<f32>,
}

impl ProviderConfig {
    /// Keychain entry name for this provider's API key.
    pub fn secret_key(&self) -> String {
        format!("provider:{}", self.id)
    }
}

/// A starting point offered in the "Add model" screen.
#[derive(Debug, Clone, Serialize)]
pub struct Preset {
    pub id: &'static str,
    pub label: &'static str,
    pub kind: ProviderKind,
    pub base_url: &'static str,
    pub needs_key: bool,
    pub local: bool,
    pub hint: &'static str,
}

/// Presets shown in the UI, open-source and local options first.
pub fn presets() -> Vec<Preset> {
    use ProviderKind::*;
    let p = |id, label, kind, base_url, needs_key, local, hint| Preset {
        id,
        label,
        kind,
        base_url,
        needs_key,
        local,
        hint,
    };
    vec![
        p(
            "ollama",
            "Ollama",
            Ollama,
            "http://localhost:11434/v1",
            false,
            true,
            "Runs models on this computer. Start Ollama first.",
        ),
        p(
            "lmstudio",
            "LM Studio",
            OpenAiCompatible,
            "http://localhost:1234/v1",
            false,
            true,
            "Enable the local server in LM Studio.",
        ),
        p(
            "llamacpp",
            "llama.cpp server",
            OpenAiCompatible,
            "http://localhost:8080/v1",
            false,
            true,
            "Start llama-server with --jinja for tool calling.",
        ),
        p(
            "vllm",
            "vLLM",
            OpenAiCompatible,
            "http://localhost:8000/v1",
            false,
            true,
            "Start with --enable-auto-tool-choice and a --tool-call-parser.",
        ),
        p(
            "custom",
            "Other OpenAI-compatible server",
            OpenAiCompatible,
            "",
            false,
            true,
            "Any server with an OpenAI-style /v1/chat/completions endpoint.",
        ),
        p(
            "openai",
            "OpenAI",
            OpenAi,
            "https://api.openai.com/v1",
            true,
            false,
            "",
        ),
        p(
            "anthropic",
            "Anthropic (Claude)",
            Anthropic,
            "https://api.anthropic.com",
            true,
            false,
            "",
        ),
        p(
            "gemini",
            "Google Gemini",
            Gemini,
            "https://generativelanguage.googleapis.com",
            true,
            false,
            "",
        ),
        p(
            "openrouter",
            "OpenRouter",
            OpenAiCompatible,
            "https://openrouter.ai/api/v1",
            true,
            false,
            "Hundreds of open and frontier models behind one key.",
        ),
        p(
            "groq",
            "Groq",
            OpenAiCompatible,
            "https://api.groq.com/openai/v1",
            true,
            false,
            "",
        ),
        p(
            "mistral",
            "Mistral",
            OpenAiCompatible,
            "https://api.mistral.ai/v1",
            true,
            false,
            "",
        ),
        p(
            "deepseek",
            "DeepSeek",
            OpenAiCompatible,
            "https://api.deepseek.com/v1",
            true,
            false,
            "",
        ),
        p(
            "xai",
            "xAI (Grok)",
            OpenAiCompatible,
            "https://api.x.ai/v1",
            true,
            false,
            "",
        ),
        p(
            "together",
            "Together AI",
            OpenAiCompatible,
            "https://api.together.xyz/v1",
            true,
            false,
            "",
        ),
    ]
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
            local: None,
            temperature: None,
        };
        let json = serde_json::to_string(&cfg).unwrap();
        assert!(json.contains("\"kind\":\"ollama\""));
        let back: ProviderConfig = serde_json::from_str(&json).unwrap();
        assert_eq!(cfg, back);
        assert_eq!(cfg.secret_key(), "provider:local");
    }

    #[test]
    fn presets_have_unique_ids() {
        let ps = presets();
        let mut ids: Vec<_> = ps.iter().map(|p| p.id).collect();
        ids.sort();
        ids.dedup();
        assert_eq!(ids.len(), ps.len());
    }
}
