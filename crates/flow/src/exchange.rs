//! Exchanges: a conversation with the model, passed between blocks.
//!
//! An exchange is a list of messages (system, user, assistant, tool). A
//! model block given an exchange continues it; when the model asks for a
//! tool, the exchange (with the model's request at the end) goes out of the
//! block's Tool output, and a Run tool block adds the results so the model
//! can pick up where it left off.

use serde::{Deserialize, Serialize};
use serde_json::Value as Json;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Role {
    System,
    User,
    Assistant,
    Tool,
}

impl Role {
    pub fn label(self) -> &'static str {
        match self {
            Role::System => "System",
            Role::User => "User",
            Role::Assistant => "Assistant",
            Role::Tool => "Tool",
        }
    }
}

/// A tool call the model asked for.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ToolCall {
    pub id: String,
    pub name: String,
    /// Always a JSON object.
    pub arguments: Json,
    /// The block that made this call lets its tools use the internet. (The
    /// app and the agent must allow it too.)
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub online: bool,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ExMessage {
    pub role: Role,
    #[serde(default)]
    pub text: String,
    /// Assistant messages: the tools it asked for.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub calls: Vec<ToolCall>,
    /// Tool messages: which call this answers, and the tool's name.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub call_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
}

impl ExMessage {
    pub fn new(role: Role, text: impl Into<String>) -> Self {
        Self {
            role,
            text: text.into(),
            calls: vec![],
            call_id: None,
            name: None,
        }
    }
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct Exchange {
    pub messages: Vec<ExMessage>,
}

impl Exchange {
    /// The tool calls still waiting for results: those in the last
    /// assistant message that no later tool message answers.
    pub fn pending_calls(&self) -> Vec<ToolCall> {
        let Some(i) = self
            .messages
            .iter()
            .rposition(|m| m.role == Role::Assistant)
        else {
            return vec![];
        };
        let answered: Vec<&str> = self.messages[i + 1..]
            .iter()
            .filter_map(|m| m.call_id.as_deref())
            .collect();
        self.messages[i]
            .calls
            .iter()
            .filter(|c| !answered.contains(&c.id.as_str()))
            .cloned()
            .collect()
    }

    /// "User: …\n\nAssistant: …", for text blocks and previews.
    pub fn transcript(&self) -> String {
        self.messages
            .iter()
            .map(|m| {
                let mut s = match (&m.role, &m.name) {
                    (Role::Tool, Some(n)) => format!("Tool ({n}): {}", m.text),
                    (r, _) => format!("{}: {}", r.label(), m.text),
                };
                for c in &m.calls {
                    if !s.ends_with(": ") {
                        s.push('\n');
                    }
                    s.push_str(&format!("[calls {} {}]", c.name, c.arguments));
                }
                s.trim_end().to_string()
            })
            .collect::<Vec<_>>()
            .join("\n\n")
    }
}

/// A tool as offered to the model.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ToolInfo {
    pub name: String,
    pub description: String,
    /// JSON Schema for the arguments object.
    pub parameters: Json,
}

/// Checks a call's arguments against the tool's schema, fixing what's safe
/// to fix: numbers and true/false written as text (small models often do
/// this). Returns the cleaned arguments, or what's wrong in words the model
/// can act on.
pub fn check_arguments(schema: &Json, args: &Json) -> Result<Json, String> {
    let Some(obj) = args.as_object() else {
        return Err("the arguments must be a JSON object".into());
    };
    let props = schema
        .get("properties")
        .and_then(|p| p.as_object())
        .cloned()
        .unwrap_or_default();
    let mut out = serde_json::Map::new();
    let strict = schema.get("additionalProperties") == Some(&Json::Bool(false));
    let required: Vec<&str> = schema
        .get("required")
        .and_then(|r| r.as_array())
        .map(|r| r.iter().filter_map(|x| x.as_str()).collect())
        .unwrap_or_default();
    for (k, v) in obj {
        // `null` for an optional argument means "use the default".
        if v.is_null() && !required.contains(&k.as_str()) {
            continue;
        }
        match props.get(k) {
            Some(spec) => {
                out.insert(k.clone(), check_value(k, spec, v)?);
            }
            None if strict => {
                let known: Vec<&str> = props.keys().map(String::as_str).collect();
                return Err(format!(
                    "there's no argument “{k}” (it takes: {})",
                    if known.is_empty() {
                        "nothing".into()
                    } else {
                        known.join(", ")
                    }
                ));
            }
            None => {
                out.insert(k.clone(), v.clone());
            }
        }
    }
    if let Some(req) = schema.get("required").and_then(|r| r.as_array()) {
        for r in req.iter().filter_map(|r| r.as_str()) {
            if !out.contains_key(r) {
                return Err(format!("the argument “{r}” is missing"));
            }
        }
    }
    Ok(Json::Object(out))
}

fn check_value(name: &str, spec: &Json, v: &Json) -> Result<Json, String> {
    if let Some(options) = spec.get("enum").and_then(|e| e.as_array()) {
        if !options.contains(v) {
            return Err(format!(
                "“{name}” must be one of {}",
                options
                    .iter()
                    .map(|o| o.to_string())
                    .collect::<Vec<_>>()
                    .join(", ")
            ));
        }
        return Ok(v.clone());
    }
    let want = spec.get("type").and_then(|t| t.as_str()).unwrap_or("");
    let wrong = |what: &str| Err(format!("“{name}” must be {what}"));
    match (want, v) {
        ("string", Json::String(_)) => Ok(v.clone()),
        ("string", Json::Number(n)) => Ok(Json::String(n.to_string())),
        ("string", _) => wrong("text"),
        ("integer", Json::Number(n)) if n.is_i64() || n.is_u64() => Ok(v.clone()),
        ("integer", Json::Number(n)) => match n.as_f64() {
            Some(f) if f.fract() == 0.0 => Ok(Json::from(f as i64)),
            _ => wrong("a whole number"),
        },
        ("integer", Json::String(s)) => s
            .trim()
            .parse::<i64>()
            .map(Json::from)
            .or_else(|_| wrong("a whole number")),
        ("integer", _) => wrong("a whole number"),
        ("number", Json::Number(_)) => Ok(v.clone()),
        ("number", Json::String(s)) => s
            .trim()
            .parse::<f64>()
            .ok()
            .and_then(serde_json::Number::from_f64)
            .map(Json::Number)
            .ok_or(())
            .or_else(|_| wrong("a number")),
        ("number", _) => wrong("a number"),
        ("boolean", Json::Bool(_)) => Ok(v.clone()),
        ("boolean", Json::String(s)) => match s.trim().to_lowercase().as_str() {
            "true" | "yes" => Ok(Json::Bool(true)),
            "false" | "no" => Ok(Json::Bool(false)),
            _ => wrong("true or false"),
        },
        ("boolean", _) => wrong("true or false"),
        ("array", Json::Array(items)) => {
            let item_spec = spec.get("items").cloned().unwrap_or(Json::Null);
            items
                .iter()
                .map(|i| {
                    if item_spec.is_object() {
                        check_value(name, &item_spec, i)
                    } else {
                        Ok(i.clone())
                    }
                })
                .collect::<Result<Vec<_>, _>>()
                .map(Json::Array)
        }
        ("array", _) => wrong("a list"),
        _ => Ok(v.clone()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn schema() -> Json {
        json!({
            "type": "object",
            "properties": {
                "path": { "type": "string" },
                "max_chars": { "type": "integer" },
                "append": { "type": "boolean" },
                "mode": { "enum": ["a", "b"] }
            },
            "required": ["path"],
            "additionalProperties": false
        })
    }

    #[test]
    fn arguments_are_checked_and_gently_fixed() {
        let ok = check_arguments(
            &schema(),
            &json!({ "path": "x", "max_chars": "50", "append": "true" }),
        )
        .unwrap();
        assert_eq!(ok, json!({ "path": "x", "max_chars": 50, "append": true }));
        assert!(
            check_arguments(&schema(), &json!({}))
                .unwrap_err()
                .contains("“path” is missing")
        );
        assert!(
            check_arguments(&schema(), &json!({ "path": "x", "nope": 1 }))
                .unwrap_err()
                .contains("no argument “nope”")
        );
        assert!(check_arguments(&schema(), &json!({ "path": "x", "max_chars": "lots" })).is_err());
        assert!(check_arguments(&schema(), &json!({ "path": "x", "mode": "c" })).is_err());
        assert!(check_arguments(&schema(), &json!("x")).is_err());
        // null for an optional argument is dropped; for a required one it's an error.
        assert_eq!(
            check_arguments(&schema(), &json!({ "path": "x", "max_chars": null })).unwrap(),
            json!({ "path": "x" })
        );
        assert!(check_arguments(&schema(), &json!({ "path": null })).is_err());
    }

    #[test]
    fn pending_calls_and_transcript() {
        let call = ToolCall {
            id: "1".into(),
            name: "read_file".into(),
            arguments: json!({ "path": "a" }),
            online: false,
        };
        let mut ex = Exchange {
            messages: vec![ExMessage::new(Role::User, "hi")],
        };
        ex.messages.push(ExMessage {
            calls: vec![call.clone()],
            ..ExMessage::new(Role::Assistant, "")
        });
        assert_eq!(ex.pending_calls(), vec![call]);
        ex.messages.push(ExMessage {
            call_id: Some("1".into()),
            name: Some("read_file".into()),
            ..ExMessage::new(Role::Tool, "contents")
        });
        assert!(ex.pending_calls().is_empty());
        let t = ex.transcript();
        assert!(
            t.starts_with("User: hi\n\nAssistant: [calls read_file"),
            "{t}"
        );
        assert!(t.ends_with("Tool (read_file): contents"));
    }
}
