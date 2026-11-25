//! Provider configuration and presets.

use serde::{Deserialize, Serialize};

use crate::generation::Generation;

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
    /// Most tokens (sent and received) per day; `None` means no limit.
    /// Missing in older files means the default limit.
    #[serde(default = "default_daily_tokens")]
    pub daily_tokens: Option<u64>,
    /// Longest answer, in tokens, unless a model block sets its own.
    #[serde(default = "default_max_tokens")]
    pub max_tokens: u32,
    /// The context length the server reports for this model, in tokens
    /// (`None`: it doesn't say). Found when the connection is saved, tested
    /// or asked; the settings below can't go past it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub context_max: Option<u32>,
    /// Context length the user set, in tokens (`None`: the reported one, or
    /// [`DEFAULT_CONTEXT`]). Capped at `context_max`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub context_tokens: Option<u32>,
    /// When loops should make room (summarise), in tokens (`None`:
    /// [`DEFAULT_COMPACT_PERCENT`] of the context). Capped at the context.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub compact_at: Option<u32>,
    /// "Use model within app": Chummy may use this model for its own small
    /// jobs outside loops (chat titles and the like). Off by default: the
    /// model is the user's, for their loops.
    #[serde(default, skip_serializing_if = "is_false")]
    pub use_in_app: bool,
    /// Reasoning effort, sampling, penalties, seed and stop sequences
    /// (all optional; unset means the server's default).
    #[serde(default, skip_serializing_if = "is_default_generation")]
    pub generation: Generation,
}

fn is_default_generation(g: &Generation) -> bool {
    *g == Generation::default()
}

fn is_false(b: &bool) -> bool {
    !*b
}

/// Context length assumed when neither the server nor the user says.
pub const DEFAULT_CONTEXT: u32 = 8192;
/// Loops make room at this share of the context unless set otherwise.
pub const DEFAULT_COMPACT_PERCENT: u32 = 80;
/// The smallest context or compaction point accepted.
pub const MIN_CONTEXT: u32 = 512;

/// The answer length new connections start with.
pub const DEFAULT_MAX_TOKENS: u32 = 1024;
/// The shortest and longest "Longest answer" accepted, in tokens.
pub const MIN_ANSWER_TOKENS: u32 = 16;
pub const MAX_ANSWER_TOKENS: u32 = 32_768;

fn default_max_tokens() -> u32 {
    DEFAULT_MAX_TOKENS
}

/// The daily token limit new connections start with.
pub const DEFAULT_DAILY_TOKENS: u64 = 1_000_000;

fn default_daily_tokens() -> Option<u64> {
    Some(DEFAULT_DAILY_TOKENS)
}

impl ProviderConfig {
    /// Keychain entry name for this provider's API key.
    pub fn secret_key(&self) -> String {
        format!("provider:{}", self.id)
    }

    /// The context length in use, in tokens: the user's setting (never more
    /// than the server reports), else the reported one, else [`DEFAULT_CONTEXT`].
    pub fn context_window(&self) -> u32 {
        let reported = self.context_max.filter(|n| *n > 0);
        let n = match (self.context_tokens.filter(|n| *n > 0), reported) {
            (Some(mine), Some(max)) => mine.min(max),
            (Some(mine), None) => mine,
            (None, Some(max)) => max,
            (None, None) => DEFAULT_CONTEXT,
        };
        n.max(MIN_CONTEXT.min(reported.unwrap_or(MIN_CONTEXT)))
    }

    /// When loops should make room, in tokens: the user's setting (never
    /// more than the context), else [`DEFAULT_COMPACT_PERCENT`] of it.
    pub fn compact_threshold(&self) -> u32 {
        let window = self.context_window();
        match self.compact_at.filter(|n| *n > 0) {
            Some(n) => n.min(window),
            None => (u64::from(window) * u64::from(DEFAULT_COMPACT_PERCENT) / 100) as u32,
        }
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
    #[test]
    fn context_and_compaction_points() {
        let mut p = ProviderConfig {
            id: "x".into(),
            name: "x".into(),
            kind: ProviderKind::OpenAiCompatible,
            base_url: "http://localhost:1/v1".into(),
            model: "m".into(),
            local: None,
            temperature: None,
            daily_tokens: None,
            max_tokens: 1024,
            context_max: None,
            context_tokens: None,
            compact_at: None,
            use_in_app: false,
            generation: Default::default(),
        };
        // Nothing known: the default, made room for at 80%.
        assert_eq!(p.context_window(), DEFAULT_CONTEXT);
        assert_eq!(p.compact_threshold(), 6553);
        // The server says.
        p.context_max = Some(32768);
        assert_eq!((p.context_window(), p.compact_threshold()), (32768, 26214));
        // The user can lower it but not go past what the server reports.
        p.context_tokens = Some(16384);
        assert_eq!(p.context_window(), 16384);
        p.context_tokens = Some(65536);
        assert_eq!(p.context_window(), 32768);
        // The compaction point, capped at the context.
        p.compact_at = Some(20000);
        assert_eq!(p.compact_threshold(), 20000);
        p.compact_at = Some(99999);
        assert_eq!(p.compact_threshold(), 32768);
        // Without a report, the user's context is used as is.
        p.context_max = None;
        p.compact_at = None;
        assert_eq!((p.context_window(), p.compact_threshold()), (65536, 52428));
    }

    use super::*;

    #[test]
    fn presets_and_saved_connections() {
        let ps = presets();
        let mut ids: Vec<_> = ps.iter().map(|p| p.id).collect();
        ids.sort();
        ids.dedup();
        assert_eq!(ids.len(), ps.len());

        // (was provider_config_round_trips_as_json)
        {
            let cfg = ProviderConfig {
                id: "local".into(),
                name: "Local Ollama".into(),
                kind: ProviderKind::Ollama,
                base_url: ProviderKind::Ollama.default_base_url().unwrap().into(),
                model: "llama3.2:3b".into(),
                local: None,
                temperature: None,
                daily_tokens: Some(crate::DEFAULT_DAILY_TOKENS),
                max_tokens: crate::DEFAULT_MAX_TOKENS,
                context_max: None,
                context_tokens: None,
                compact_at: None,
                use_in_app: false,
                generation: Default::default(),
            };
            let json = serde_json::to_string(&cfg).unwrap();
            assert!(json.contains("\"kind\":\"ollama\""));
            let back: ProviderConfig = serde_json::from_str(&json).unwrap();
            assert_eq!(cfg, back);
            assert_eq!(cfg.secret_key(), "provider:local");
        }
    }
}

/// What's wrong with a model server's address, if anything, in words for
/// the user. Addresses with a user name or password in them are refused, so
/// keys only ever live in the keychain. Plain `http://` is allowed anywhere
/// (some servers sit behind a VPN with no usable certificate), but the app
/// asks first when it isn't this computer or the local network: see
/// [`unencrypted_remote`].
pub fn endpoint_problem(url: &str) -> Option<String> {
    let url = url.trim();
    if !(url.starts_with("http://") || url.starts_with("https://")) {
        return Some("The server address must start with http:// or https://.".into());
    }
    let Ok(parsed) = reqwest::Url::parse(url) else {
        return Some("That server address isn't valid.".into());
    };
    if !parsed.username().is_empty() || parsed.password().is_some() {
        return Some("Don't put a user name or key in the address; use the API key field.".into());
    }
    if parsed.host().is_none() {
        return Some("The server address needs a host name.".into());
    }
    None
}

/// Whether `url` is plain `http://` to somewhere other than this computer
/// or the local network, so API keys and conversations would cross the
/// network unencrypted (unless something else, like a VPN, encrypts them).
pub fn unencrypted_remote(url: &str) -> bool {
    let Ok(parsed) = reqwest::Url::parse(url.trim()) else {
        return false;
    };
    if parsed.scheme() != "http" {
        return false;
    }
    let Some(host) = parsed.host() else {
        return false;
    };
    let local = match host {
        url::Host::Ipv4(ip) => ip.is_loopback() || ip.is_private() || ip.is_link_local(),
        url::Host::Ipv6(ip) => {
            ip.is_loopback() || ip.is_unique_local() || ip.is_unicast_link_local()
        }
        url::Host::Domain(d) => {
            let d = d.trim_end_matches('.').to_ascii_lowercase();
            d == "localhost"
                || d.ends_with(".localhost")
                || d.ends_with(".local")
                || d.ends_with(".lan")
                || d.ends_with(".home.arpa")
                || d.ends_with(".internal")
                || !d.contains('.')
        }
    };
    !local
}

#[cfg(test)]
mod endpoint_tests {
    use super::{endpoint_problem, unencrypted_remote};

    #[test]
    fn addresses_are_checked() {
        for ok in [
            "https://api.openai.com/v1",
            "http://localhost:11434",
            "http://203.0.113.9/v1",
            "http://llm.example.vpn:8000/v1",
        ] {
            assert_eq!(endpoint_problem(ok), None, "{ok}");
        }
        for bad in [
            "https://user:key@api.example.com",
            "http://user@example.com",
            "ftp://x",
            "localhost:8080",
            "http://",
        ] {
            assert!(endpoint_problem(bad).is_some(), "{bad}");
        }
    }

    #[test]
    fn plain_http_off_the_local_network_is_flagged() {
        for local in [
            "https://api.openai.com/v1",
            "https://203.0.113.9/v1",
            "http://localhost:11434",
            "http://127.0.0.1:8080/v1",
            "http://192.168.1.20:8000/v1",
            "http://10.0.0.5/v1",
            "http://[::1]:8080",
            "http://gpu-box:8000/v1",
            "http://studio.local:1234/v1",
        ] {
            assert!(!unencrypted_remote(local), "{local}");
        }
        for remote in [
            "http://api.example.com/v1",
            "http://203.0.113.9/v1",
            "http://100.64.1.2:8000/v1",
        ] {
            assert!(unencrypted_remote(remote), "{remote}");
        }
    }
}
