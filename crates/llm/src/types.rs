//! Provider-neutral chat types.

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Role {
    System,
    User,
    Assistant,
    /// The result of a tool call, answering [`Message::tool_call_id`].
    Tool,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Message {
    pub role: Role,
    #[serde(default)]
    pub content: String,
    /// Tool calls the assistant made in this turn.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub tool_calls: Vec<ToolCall>,
    /// For `Role::Tool`: which call this answers.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tool_call_id: Option<String>,
    /// For `Role::Tool`: the tool's name (Gemini needs it).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
}

impl Message {
    fn new(role: Role, content: impl Into<String>) -> Self {
        Self {
            role,
            content: content.into(),
            tool_calls: vec![],
            tool_call_id: None,
            name: None,
        }
    }
    pub fn system(content: impl Into<String>) -> Self {
        Self::new(Role::System, content)
    }
    pub fn user(content: impl Into<String>) -> Self {
        Self::new(Role::User, content)
    }
    pub fn assistant(content: impl Into<String>) -> Self {
        Self::new(Role::Assistant, content)
    }
    pub fn tool_result(call: &ToolCall, content: impl Into<String>) -> Self {
        Self {
            tool_call_id: Some(call.id.clone()),
            name: Some(call.name.clone()),
            ..Self::new(Role::Tool, content)
        }
    }
}

/// A complete, parsed tool call.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ToolCall {
    pub id: String,
    pub name: String,
    /// Always a JSON object.
    pub arguments: serde_json::Value,
}

/// A tool offered to the model.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ToolSpec {
    pub name: String,
    pub description: String,
    /// JSON Schema for the arguments object.
    pub parameters: serde_json::Value,
}

/// Whether the model must call a tool.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ToolChoice {
    #[default]
    Auto,
    /// Call one of the offered tools.
    Required,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct ChatRequest {
    pub messages: Vec<Message>,
    #[serde(default)]
    pub tools: Vec<ToolSpec>,
    /// Ignored when `tools` is empty.
    #[serde(default)]
    pub tool_choice: ToolChoice,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub temperature: Option<f32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_tokens: Option<u32>,
    /// Reasoning effort, sampling and the rest; unset ones come from the
    /// connection, and only what its kind of server accepts is sent.
    #[serde(default)]
    pub generation: crate::Generation,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum StopReason {
    EndTurn,
    ToolUse,
    MaxTokens,
    Other(String),
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Usage {
    pub input_tokens: u32,
    pub output_tokens: u32,
}

/// What a provider produces while answering, in order.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ChatEvent {
    Text {
        text: String,
    },
    /// Visible "thinking" from reasoning models, when the provider exposes it.
    Reasoning {
        text: String,
    },
    ToolCall {
        call: ToolCall,
    },
    /// The model tried to call a tool but its arguments weren't valid JSON.
    MalformedToolCall {
        id: String,
        name: String,
        raw: String,
        error: String,
    },
    Done {
        stop: StopReason,
        usage: Option<Usage>,
    },
}

/// Everything from one streamed answer, collected.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct ChatResponse {
    pub text: String,
    pub reasoning: String,
    pub tool_calls: Vec<ToolCall>,
    pub malformed: Vec<(String, String)>,
    pub stop: Option<StopReason>,
    pub usage: Option<Usage>,
}

impl ChatResponse {
    pub(crate) fn absorb(&mut self, ev: &ChatEvent) {
        match ev {
            ChatEvent::Text { text } => self.text.push_str(text),
            ChatEvent::Reasoning { text } => self.reasoning.push_str(text),
            ChatEvent::ToolCall { call } => self.tool_calls.push(call.clone()),
            ChatEvent::MalformedToolCall { name, raw, .. } => {
                self.malformed.push((name.clone(), raw.clone()))
            }
            ChatEvent::Done { stop, usage } => {
                self.stop = Some(stop.clone());
                self.usage = *usage;
            }
        }
    }
}

/// Parses accumulated tool-call argument text into an event. Empty arguments
/// mean "no arguments".
pub(crate) fn finish_tool_call(id: String, name: String, raw: &str) -> ChatEvent {
    let name = name.trim().to_string();
    if name.is_empty() {
        return ChatEvent::MalformedToolCall {
            id,
            name,
            raw: raw.into(),
            error: "the call has no tool name".into(),
        };
    }
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return ChatEvent::ToolCall {
            call: ToolCall {
                id,
                name,
                arguments: serde_json::json!({}),
            },
        };
    }
    match serde_json::from_str::<serde_json::Value>(trimmed) {
        Ok(v @ serde_json::Value::Object(_)) => ChatEvent::ToolCall {
            call: ToolCall {
                id,
                name,
                arguments: v,
            },
        },
        // Some models encode the arguments twice: a JSON string of JSON.
        Ok(serde_json::Value::String(inner))
            if serde_json::from_str::<serde_json::Value>(&inner).is_ok_and(|v| v.is_object()) =>
        {
            ChatEvent::ToolCall {
                call: ToolCall {
                    id,
                    name,
                    arguments: serde_json::from_str(&inner).unwrap_or_default(),
                },
            }
        }
        Ok(_) => ChatEvent::MalformedToolCall {
            id,
            name,
            raw: raw.into(),
            error: "arguments must be a JSON object".into(),
        },
        Err(e) => ChatEvent::MalformedToolCall {
            id,
            name,
            raw: raw.into(),
            error: e.to_string(),
        },
    }
}

/// Short random-ish id for providers that don't assign tool-call ids.
pub(crate) fn make_call_id(n: usize) -> String {
    let t = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.subsec_nanos())
        .unwrap_or(0);
    format!("call_{t:08x}{n}")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tool_call_arguments() {
        let ok = |raw: &str| match finish_tool_call("1".into(), "t".into(), raw) {
            ChatEvent::ToolCall { call } => Some(call.arguments),
            _ => None,
        };
        assert_eq!(ok("").unwrap(), serde_json::json!({}));
        assert_eq!(ok(r#"{"a":1}"#).unwrap(), serde_json::json!({ "a": 1 }));
        // Double-encoded arguments are unwrapped.
        assert_eq!(ok(r#""{\"a\":1}""#).unwrap(), serde_json::json!({ "a": 1 }));
        assert!(ok("[1]").is_none());
        assert!(ok(r#""just text""#).is_none());
        assert!(ok("{broken").is_none());
        assert!(matches!(
            finish_tool_call("1".into(), "  ".into(), "{}"),
            ChatEvent::MalformedToolCall { .. }
        ));
    }
}
