//! Generation settings beyond temperature and answer length: reasoning
//! effort, sampling, penalties, seed and stop sequences. Set per model
//! connection; an agent can override any of them (see [`ModelOverrides`]).
//!
//! Every setting is optional: unset means "don't send it", so the server's
//! own default applies. Settings a kind of server doesn't accept are never
//! sent (see [`support`]), so a setting left over from another kind can't
//! make requests fail.

use serde::{Deserialize, Deserializer, Serialize};

use crate::ProviderKind;
use crate::config::ProviderConfig;

/// How much a reasoning model thinks before it answers.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ReasoningEffort {
    /// Don't think (where the model allows it).
    None,
    Minimal,
    Low,
    Medium,
    High,
}

impl ReasoningEffort {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::None => "none",
            Self::Minimal => "minimal",
            Self::Low => "low",
            Self::Medium => "medium",
            Self::High => "high",
        }
    }

    /// Gemini's thinking budget, in tokens, for this effort.
    pub(crate) fn gemini_budget(self) -> u32 {
        match self {
            Self::None => 0,
            Self::Minimal => 512,
            Self::Low => 1024,
            Self::Medium => 8192,
            Self::High => 24576,
        }
    }
}

/// Optional generation settings (`None` / empty: not sent).
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Generation {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reasoning_effort: Option<ReasoningEffort>,
    /// Nucleus sampling: only the most likely tokens adding up to this share.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub top_p: Option<f32>,
    /// Only the this-many most likely tokens.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub top_k: Option<u32>,
    /// Drop tokens less likely than this share of the most likely one.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub min_p: Option<f32>,
    /// Push away from tokens already used at all (−2 to 2).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub presence_penalty: Option<f32>,
    /// Push away from tokens by how often they were used (−2 to 2).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub frequency_penalty: Option<f32>,
    /// Multiplicative repetition penalty (1 = none), as llama.cpp and vLLM use.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub repeat_penalty: Option<f32>,
    /// Fixed random seed, for repeatable answers where supported.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub seed: Option<u64>,
    /// Stop the answer when one of these appears (at most 4).
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub stop: Vec<String>,
}

/// One generation setting, for [`support`] and the UI.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Param {
    ReasoningEffort,
    TopP,
    TopK,
    MinP,
    PresencePenalty,
    FrequencyPenalty,
    RepeatPenalty,
    Seed,
    Stop,
}

impl Param {
    pub const ALL: [Param; 9] = [
        Param::ReasoningEffort,
        Param::TopP,
        Param::TopK,
        Param::MinP,
        Param::PresencePenalty,
        Param::FrequencyPenalty,
        Param::RepeatPenalty,
        Param::Seed,
        Param::Stop,
    ];
}

/// Whether a kind of server takes a setting: `Err` says why not (shown
/// in the app next to the greyed-out setting).
pub fn support(kind: ProviderKind, param: Param) -> Result<(), &'static str> {
    use Param::*;
    use ProviderKind::*;
    match (kind, param) {
        // Sent as-is; servers ignore what they don't know.
        (OpenAiCompatible | Ollama, _) => Ok(()),
        (Local, _) => Err("The built-in engine doesn't use this yet."),
        (OpenAi, TopK | MinP | RepeatPenalty) => Err("OpenAI doesn't accept this."),
        (OpenAi, _) => Ok(()),
        (Anthropic, ReasoningEffort) => Err(
            "Not yet for Anthropic: its thinking has to be passed back between tool calls, which Chummy doesn't do yet.",
        ),
        (Anthropic, TopP | TopK | Stop) => Ok(()),
        (Anthropic, _) => Err("Anthropic doesn't accept this."),
        (Gemini, MinP | RepeatPenalty) => Err("Gemini doesn't accept this."),
        (Gemini, _) => Ok(()),
    }
}

impl Generation {
    /// Only the settings `kind` accepts.
    pub fn for_kind(&self, kind: ProviderKind) -> Generation {
        let ok = |p| support(kind, p).is_ok();
        Generation {
            reasoning_effort: self.reasoning_effort.filter(|_| ok(Param::ReasoningEffort)),
            top_p: self.top_p.filter(|_| ok(Param::TopP)),
            top_k: self.top_k.filter(|_| ok(Param::TopK)),
            min_p: self.min_p.filter(|_| ok(Param::MinP)),
            presence_penalty: self.presence_penalty.filter(|_| ok(Param::PresencePenalty)),
            frequency_penalty: self
                .frequency_penalty
                .filter(|_| ok(Param::FrequencyPenalty)),
            repeat_penalty: self.repeat_penalty.filter(|_| ok(Param::RepeatPenalty)),
            seed: self.seed.filter(|_| ok(Param::Seed)),
            stop: if ok(Param::Stop) {
                self.stop.clone()
            } else {
                vec![]
            },
        }
    }

    /// Each setting from `self`, else from `fallback`.
    pub fn or(&self, fallback: &Generation) -> Generation {
        Generation {
            reasoning_effort: self.reasoning_effort.or(fallback.reasoning_effort),
            top_p: self.top_p.or(fallback.top_p),
            top_k: self.top_k.or(fallback.top_k),
            min_p: self.min_p.or(fallback.min_p),
            presence_penalty: self.presence_penalty.or(fallback.presence_penalty),
            frequency_penalty: self.frequency_penalty.or(fallback.frequency_penalty),
            repeat_penalty: self.repeat_penalty.or(fallback.repeat_penalty),
            seed: self.seed.or(fallback.seed),
            stop: if self.stop.is_empty() {
                fallback.stop.clone()
            } else {
                self.stop.clone()
            },
        }
    }

    /// What's wrong with these settings, in plain words (empty: fine).
    pub fn problems(&self) -> Vec<String> {
        let mut out = Vec::new();
        let range = |out: &mut Vec<String>, v: Option<f32>, lo: f32, hi: f32, name: &str| {
            if let Some(x) = v
                && !(x.is_finite() && (lo..=hi).contains(&x))
            {
                out.push(format!("{name} must be between {lo} and {hi}."));
            }
        };
        range(&mut out, self.top_p, 0.0, 1.0, "Top P");
        range(&mut out, self.min_p, 0.0, 1.0, "Min P");
        range(
            &mut out,
            self.presence_penalty,
            -2.0,
            2.0,
            "Presence penalty",
        );
        range(
            &mut out,
            self.frequency_penalty,
            -2.0,
            2.0,
            "Frequency penalty",
        );
        range(&mut out, self.repeat_penalty, 0.0, 10.0, "Repeat penalty");
        if let Some(k) = self.top_k
            && !(1..=1000).contains(&k)
        {
            out.push("Top K must be between 1 and 1000.".into());
        }
        if let Some(s) = self.seed
            && s > MAX_SEED
        {
            out.push(format!("Seed must be at most {MAX_SEED}."));
        }
        if self.stop.len() > MAX_STOP {
            out.push(format!("At most {MAX_STOP} stop sequences."));
        }
        if self
            .stop
            .iter()
            .any(|s| s.is_empty() || s.chars().count() > 100)
        {
            out.push("Stop sequences must be 1 to 100 characters.".into());
        }
        out
    }
}

/// A setting as JSON, rounded to 6 decimals: `f32` → `f64` would otherwise
/// send 0.1 as 0.10000000149011612.
pub(crate) fn num(x: f32) -> serde_json::Value {
    serde_json::json!((f64::from(x) * 1e6).round() / 1e6)
}

/// Most stop sequences (OpenAI's limit, the lowest of the three APIs).
pub const MAX_STOP: usize = 4;
/// Largest seed: the biggest whole number JSON numbers carry exactly.
pub const MAX_SEED: u64 = (1 << 53) - 1;

/// An agent's changes to its model's settings. For each one: missing =
/// inherit the model's; `null` = don't send it (the server's default);
/// a value = use that. Temperature and answer length are here too.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct ModelOverrides {
    #[serde(
        skip_serializing_if = "Option::is_none",
        deserialize_with = "set_or_null"
    )]
    pub temperature: Option<Option<f32>>,
    /// Longest answer (always set on the model, so `null` isn't used).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub max_tokens: Option<u32>,
    #[serde(
        skip_serializing_if = "Option::is_none",
        deserialize_with = "set_or_null"
    )]
    pub reasoning_effort: Option<Option<ReasoningEffort>>,
    #[serde(
        skip_serializing_if = "Option::is_none",
        deserialize_with = "set_or_null"
    )]
    pub top_p: Option<Option<f32>>,
    #[serde(
        skip_serializing_if = "Option::is_none",
        deserialize_with = "set_or_null"
    )]
    pub top_k: Option<Option<u32>>,
    #[serde(
        skip_serializing_if = "Option::is_none",
        deserialize_with = "set_or_null"
    )]
    pub min_p: Option<Option<f32>>,
    #[serde(
        skip_serializing_if = "Option::is_none",
        deserialize_with = "set_or_null"
    )]
    pub presence_penalty: Option<Option<f32>>,
    #[serde(
        skip_serializing_if = "Option::is_none",
        deserialize_with = "set_or_null"
    )]
    pub frequency_penalty: Option<Option<f32>>,
    #[serde(
        skip_serializing_if = "Option::is_none",
        deserialize_with = "set_or_null"
    )]
    pub repeat_penalty: Option<Option<f32>>,
    #[serde(
        skip_serializing_if = "Option::is_none",
        deserialize_with = "set_or_null"
    )]
    pub seed: Option<Option<u64>>,
    /// An empty list means "send none".
    #[serde(skip_serializing_if = "Option::is_none")]
    pub stop: Option<Vec<String>>,
}

/// A present field (`null` included) is `Some`; a missing one stays `None`
/// through `#[serde(default)]`.
fn set_or_null<'de, D, T>(d: D) -> Result<Option<Option<T>>, D::Error>
where
    D: Deserializer<'de>,
    T: Deserialize<'de>,
{
    Option::<T>::deserialize(d).map(Some)
}

impl ModelOverrides {
    pub fn is_empty(&self) -> bool {
        *self == Self::default()
    }

    /// The model connection as this agent uses it.
    pub fn apply(&self, cfg: &ProviderConfig) -> ProviderConfig {
        let mut c = cfg.clone();
        let g = &mut c.generation;
        fn set<T: Copy>(slot: &mut Option<T>, o: Option<Option<T>>) {
            if let Some(v) = o {
                *slot = v;
            }
        }
        set(&mut c.temperature, self.temperature);
        if let Some(m) = self.max_tokens {
            c.max_tokens = m;
        }
        set(&mut g.reasoning_effort, self.reasoning_effort);
        set(&mut g.top_p, self.top_p);
        set(&mut g.top_k, self.top_k);
        set(&mut g.min_p, self.min_p);
        set(&mut g.presence_penalty, self.presence_penalty);
        set(&mut g.frequency_penalty, self.frequency_penalty);
        set(&mut g.repeat_penalty, self.repeat_penalty);
        set(&mut g.seed, self.seed);
        if let Some(s) = &self.stop {
            g.stop = s.clone();
        }
        c
    }

    /// What's wrong with these overrides, in plain words (empty: fine).
    pub fn problems(&self) -> Vec<String> {
        let mut out = Vec::new();
        if let Some(Some(t)) = self.temperature
            && !(t.is_finite() && (0.0..=2.0).contains(&t))
        {
            out.push("Temperature must be between 0 and 2.".into());
        }
        if let Some(m) = self.max_tokens
            && !(crate::MIN_ANSWER_TOKENS..=crate::MAX_ANSWER_TOKENS).contains(&m)
        {
            out.push(format!(
                "Longest answer must be between {} and {} tokens.",
                crate::MIN_ANSWER_TOKENS,
                crate::MAX_ANSWER_TOKENS
            ));
        }
        let as_generation = Generation {
            reasoning_effort: self.reasoning_effort.flatten(),
            top_p: self.top_p.flatten(),
            top_k: self.top_k.flatten(),
            min_p: self.min_p.flatten(),
            presence_penalty: self.presence_penalty.flatten(),
            frequency_penalty: self.frequency_penalty.flatten(),
            repeat_penalty: self.repeat_penalty.flatten(),
            seed: self.seed.flatten(),
            stop: self.stop.clone().unwrap_or_default(),
        };
        out.extend(as_generation.problems());
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unsupported_settings_are_never_sent() {
        let g = Generation {
            reasoning_effort: Some(ReasoningEffort::High),
            top_k: Some(40),
            min_p: Some(0.05),
            top_p: Some(0.9),
            ..Default::default()
        };
        let openai = g.for_kind(ProviderKind::OpenAi);
        assert_eq!(openai.top_k, None);
        assert_eq!(openai.min_p, None);
        assert_eq!(openai.top_p, Some(0.9));
        assert_eq!(openai.reasoning_effort, Some(ReasoningEffort::High));
        let claude = g.for_kind(ProviderKind::Anthropic);
        assert_eq!(claude.reasoning_effort, None);
        assert_eq!(claude.top_k, Some(40));
        assert_eq!(g.for_kind(ProviderKind::Local), Generation::default());
        assert_eq!(g.for_kind(ProviderKind::OpenAiCompatible), g);
        for p in Param::ALL {
            assert!(support(ProviderKind::Ollama, p).is_ok());
        }
    }

    #[test]
    fn overrides_inherit_unset_or_replace() {
        let model = ProviderConfig {
            temperature: Some(0.7),
            max_tokens: 1024,
            generation: Generation {
                top_p: Some(0.9),
                seed: Some(7),
                stop: vec!["END".into()],
                ..Default::default()
            },
            ..serde_json::from_str(r#"{"id":"m","name":"M","kind":"open_ai","model":"x"}"#).unwrap()
        };
        // Missing: inherit; null: don't send; a value: use it.
        let o: ModelOverrides = serde_json::from_str(
            r#"{"temperature":0.1,"top_p":null,"reasoning_effort":"low","max_tokens":300}"#,
        )
        .unwrap();
        assert_eq!(o.temperature, Some(Some(0.1)));
        assert_eq!(o.top_p, Some(None));
        assert_eq!(o.seed, None);
        let used = o.apply(&model);
        assert_eq!(used.temperature, Some(0.1));
        assert_eq!(used.max_tokens, 300);
        assert_eq!(used.generation.top_p, None);
        assert_eq!(used.generation.seed, Some(7));
        assert_eq!(used.generation.stop, ["END"]);
        assert_eq!(used.generation.reasoning_effort, Some(ReasoningEffort::Low));
        // Round trip keeps null apart from missing.
        let v = serde_json::to_value(&o).unwrap();
        assert!(v["top_p"].is_null() && v.get("top_p").is_some());
        assert!(v.get("seed").is_none());
        assert_eq!(serde_json::from_value::<ModelOverrides>(v).unwrap(), o);
        assert!(ModelOverrides::default().apply(&model) == model);
        assert!(ModelOverrides::default().is_empty());
    }

    #[test]
    fn problems_are_explained() {
        let g = Generation {
            top_p: Some(1.5),
            top_k: Some(0),
            stop: vec!["".into(); 5],
            ..Default::default()
        };
        assert_eq!(g.problems().len(), 4);
        let o = ModelOverrides {
            temperature: Some(Some(3.0)),
            presence_penalty: Some(Some(-3.0)),
            ..Default::default()
        };
        assert_eq!(o.problems().len(), 2);
        assert!(Generation::default().problems().is_empty());
    }
}
