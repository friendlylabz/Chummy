//! A model block's own generation settings: reasoning effort, sampling,
//! penalties, seed and stop sequences. (Temperature and the longest answer
//! are separate block params, as they always were.)
//!
//! Each setting works like an agent's model settings: missing means
//! "inherit" (the agent's, else the model connection's), `null` means
//! "don't send it" (the server's default), and a value means "use this".
//! The runner turns these into `chummy_llm::ModelOverrides` for the call.

use serde::{Deserialize, Deserializer, Serialize};

/// How much a reasoning model thinks before it answers.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Effort {
    None,
    Minimal,
    Low,
    Medium,
    High,
}

impl Effort {
    pub fn as_str(self) -> &'static str {
        match self {
            Effort::None => "none",
            Effort::Minimal => "minimal",
            Effort::Low => "low",
            Effort::Medium => "medium",
            Effort::High => "high",
        }
    }
}

/// Most stop sequences a block may set (the model connection's limit too).
pub const MAX_STOP: usize = 4;

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct GenerationOverrides {
    #[serde(
        skip_serializing_if = "Option::is_none",
        deserialize_with = "set_or_null"
    )]
    pub reasoning_effort: Option<Option<Effort>>,
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

/// A present field (`null` included) is `Some`; a missing one stays `None`.
fn set_or_null<'de, D, T>(d: D) -> Result<Option<Option<T>>, D::Error>
where
    D: Deserializer<'de>,
    T: Deserialize<'de>,
{
    Option::<T>::deserialize(d).map(Some)
}

impl GenerationOverrides {
    pub fn is_empty(&self) -> bool {
        *self == Self::default()
    }

    /// What's wrong with these settings, in plain words (empty: fine).
    /// The same ranges as a model connection's settings.
    pub fn problems(&self) -> Vec<String> {
        let mut out = Vec::new();
        let mut range = |name: &str, v: Option<Option<f32>>, lo: f32, hi: f32| {
            if let Some(Some(x)) = v
                && !(x.is_finite() && (lo..=hi).contains(&x))
            {
                out.push(format!("{name} must be between {lo} and {hi}."));
            }
        };
        range("Top P", self.top_p, 0.0, 1.0);
        range("Min P", self.min_p, 0.0, 1.0);
        range("Presence penalty", self.presence_penalty, -2.0, 2.0);
        range("Frequency penalty", self.frequency_penalty, -2.0, 2.0);
        range("Repeat penalty", self.repeat_penalty, 0.0, 10.0);
        if let Some(Some(k)) = self.top_k
            && !(1..=1000).contains(&k)
        {
            out.push("Top K must be between 1 and 1000.".into());
        }
        if let Some(Some(s)) = self.seed
            && s > (1u64 << 53) - 1
        {
            out.push("The seed is too large.".into());
        }
        if let Some(stop) = &self.stop {
            if stop.len() > MAX_STOP {
                out.push(format!("At most {MAX_STOP} stop sequences."));
            }
            if stop.iter().any(|s| s.is_empty() || s.chars().count() > 100) {
                out.push("Each stop sequence must be 1 to 100 characters.".into());
            }
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn missing_inherits_null_turns_off_a_value_sets() {
        let g: GenerationOverrides =
            serde_json::from_str(r#"{"reasoning_effort":"low","top_p":null,"stop":[]}"#).unwrap();
        assert_eq!(g.reasoning_effort, Some(Some(Effort::Low)));
        assert_eq!(g.top_p, Some(None));
        assert_eq!(g.seed, None);
        assert_eq!(g.stop, Some(vec![]));
        let back = serde_json::to_value(&g).unwrap();
        assert!(back["top_p"].is_null() && back.get("seed").is_none());
        assert_eq!(
            serde_json::from_value::<GenerationOverrides>(back).unwrap(),
            g
        );
        assert!(GenerationOverrides::default().is_empty());
        assert!(serde_json::from_str::<GenerationOverrides>(r#"{"temprature":1}"#).is_err());

        // (was problems_are_explained)
        {
            let g = GenerationOverrides {
                top_p: Some(Some(1.5)),
                top_k: Some(Some(0)),
                stop: Some(vec!["".into(); 5]),
                ..Default::default()
            };
            assert_eq!(g.problems().len(), 4);
            assert!(GenerationOverrides::default().problems().is_empty());
        }
    }
}
