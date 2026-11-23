//! Tools without the server's help: Chummy describes the tools in the system
//! message and reads calls out of the model's answer.
//!
//! The format is the widely trained "Hermes" style, which most small open
//! models (Qwen, Hermes, many fine-tunes) know:
//!
//! ```text
//! <tool_call>
//! {"name": "calculator", "arguments": {"expression": "2 + 2"}}
//! </tool_call>
//! ```
//!
//! Results go back as `<tool_response>` blocks in a user message, so any
//! chat server works, even one with no idea of tools.

use serde_json::{Value as Json, json};

use crate::exchange::{ExMessage, Role, ToolInfo};

/// The system-message section describing `tools`.
pub fn tool_section(tools: &[ToolInfo]) -> String {
    let list = tools
        .iter()
        .map(|t| {
            json!({ "name": t.name, "description": t.description, "parameters": t.parameters })
                .to_string()
        })
        .collect::<Vec<_>>()
        .join("\n");
    format!(
        "# Tools\n\n\
         You can call these tools. To call one, reply with exactly this, and nothing after it:\n\
         <tool_call>\n\
         {{\"name\": \"<tool name>\", \"arguments\": {{<the arguments as JSON>}}}}\n\
         </tool_call>\n\
         To call several, write one <tool_call> block for each. The results come back in \
         <tool_response> blocks. If you don't need a tool, just answer.\n\n\
         <tools>\n{list}\n</tools>"
    )
}

/// Tool descriptions for `_TOOLS_` in a prompt: one line each.
pub fn tool_list(tools: &[ToolInfo]) -> String {
    tools
        .iter()
        .map(|t| {
            let args = t.parameters["properties"]
                .as_object()
                .map(|p| p.keys().cloned().collect::<Vec<_>>().join(", "))
                .unwrap_or_default();
            format!("- {}({args}): {}", t.name, t.description)
        })
        .collect::<Vec<_>>()
        .join("\n")
}

/// Rewrites an exchange for a model that doesn't know tool messages: calls
/// become `<tool_call>` text, and results become `<tool_response>` user
/// messages (consecutive results share one message).
pub fn plain_history(messages: &[ExMessage]) -> Vec<ExMessage> {
    let mut out: Vec<ExMessage> = Vec::new();
    for m in messages {
        match m.role {
            Role::Assistant if !m.calls.is_empty() => {
                let mut text = m.text.trim().to_string();
                for c in &m.calls {
                    if !text.is_empty() {
                        text.push('\n');
                    }
                    text.push_str(&format!(
                        "<tool_call>\n{}\n</tool_call>",
                        json!({ "name": c.name, "arguments": c.arguments })
                    ));
                }
                out.push(ExMessage::new(Role::Assistant, text));
            }
            Role::Tool => {
                let block = format!(
                    "<tool_response>\n{}\n</tool_response>",
                    json!({ "name": m.name.clone().unwrap_or_default(), "content": m.text })
                );
                match out.last_mut() {
                    Some(prev)
                        if prev.role == Role::User && prev.text.starts_with("<tool_response>") =>
                    {
                        prev.text.push('\n');
                        prev.text.push_str(&block);
                    }
                    _ => out.push(ExMessage::new(Role::User, block)),
                }
            }
            _ => out.push(ExMessage {
                calls: vec![],
                call_id: None,
                ..m.clone()
            }),
        }
    }
    out
}

/// A call found in text.
#[derive(Debug, Clone, PartialEq)]
pub struct FoundCall {
    pub name: String,
    pub arguments: Json,
}

/// A `<tool_call>` that couldn't be read.
#[derive(Debug, Clone, PartialEq)]
pub struct BadCall {
    pub name: String,
    pub raw: String,
    pub error: String,
}

/// Strips a Markdown code fence around JSON, if there is one.
fn unfence(s: &str) -> &str {
    let t = s.trim();
    let Some(rest) = t.strip_prefix("```") else {
        return t;
    };
    let rest = rest.trim_start_matches(|c: char| c.is_ascii_alphanumeric());
    rest.strip_suffix("```").unwrap_or(rest).trim()
}

fn read_call(raw: &str) -> Result<FoundCall, BadCall> {
    let bad = |name: &str, error: String| BadCall {
        name: name.to_string(),
        raw: raw.trim().to_string(),
        error,
    };
    let v: Json = serde_json::from_str(unfence(raw)).map_err(|e| bad("", e.to_string()))?;
    let name = v["name"].as_str().unwrap_or_default().trim().to_string();
    if name.is_empty() {
        return Err(bad("", "it has no \"name\"".into()));
    }
    let args = match v.get("arguments").or_else(|| v.get("parameters")) {
        None | Some(Json::Null) => json!({}),
        Some(Json::String(s)) => serde_json::from_str(s)
            .map_err(|e| bad(&name, format!("the arguments aren't valid JSON ({e})")))?,
        Some(other) => other.clone(),
    };
    if !args.is_object() {
        return Err(bad(&name, "the arguments must be a JSON object".into()));
    }
    Ok(FoundCall {
        name,
        arguments: args,
    })
}

/// Finds tool calls in a model's answer. Returns the text without them, the
/// calls, and the ones that couldn't be read. `<tool_call>` blocks are used
/// when present (an unclosed one at the end counts); otherwise an answer
/// that is nothing but a `{"name": …, "arguments": …}` object is a call.
pub fn find_calls(text: &str) -> (String, Vec<FoundCall>, Vec<BadCall>) {
    const OPEN: &str = "<tool_call>";
    const CLOSE: &str = "</tool_call>";
    let mut calls = Vec::new();
    let mut bad = Vec::new();
    if text.contains(OPEN) {
        let mut rest = text;
        let mut kept = String::new();
        while let Some(i) = rest.find(OPEN) {
            kept.push_str(&rest[..i]);
            let after = &rest[i + OPEN.len()..];
            let (inner, next) = match after.find(CLOSE) {
                Some(j) => (&after[..j], &after[j + CLOSE.len()..]),
                None => (after, ""),
            };
            match read_call(inner) {
                Ok(c) => calls.push(c),
                Err(b) => bad.push(b),
            }
            rest = next;
        }
        kept.push_str(rest);
        return (kept.trim().to_string(), calls, bad);
    }
    let t = unfence(text);
    if t.starts_with('{')
        && t.ends_with('}')
        && let Ok(v) = serde_json::from_str::<Json>(t)
        && v.get("name").is_some_and(Json::is_string)
        && (v.get("arguments").is_some() || v.get("parameters").is_some())
    {
        match read_call(t) {
            Ok(c) => calls.push(c),
            Err(b) => bad.push(b),
        }
        return (String::new(), calls, bad);
    }
    (text.trim().to_string(), calls, bad)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::exchange::ToolCall;

    #[test]
    fn finds_calls_in_tags_and_plain_json() {
        let (text, calls, bad) = find_calls(
            "Let me work it out.\n<tool_call>\n{\"name\": \"calculator\", \"arguments\": {\"expression\": \"2+2\"}}\n</tool_call>",
        );
        assert_eq!(text, "Let me work it out.");
        assert_eq!(calls[0].name, "calculator");
        assert_eq!(calls[0].arguments["expression"], "2+2");
        assert!(bad.is_empty());

        // Several calls, a fenced one, string arguments, an unclosed last one.
        let (_, calls, bad) = find_calls(
            "<tool_call>```json\n{\"name\":\"a\",\"arguments\":\"{\\\"x\\\":1}\"}\n```</tool_call>\
             <tool_call>{\"name\":\"b\",\"parameters\":{}}",
        );
        assert_eq!(calls.len(), 2, "{bad:?}");
        assert_eq!(calls[0].arguments["x"], 1);

        // Broken ones are reported, not dropped.
        let (_, calls, bad) =
            find_calls("<tool_call>{\"name\": \"a\", \"arguments\": {</tool_call>");
        assert!(calls.is_empty());
        assert_eq!(bad.len(), 1);
        let (_, _, bad) = find_calls("<tool_call>{\"arguments\": {}}</tool_call>");
        assert!(bad[0].error.contains("name"));

        // A bare JSON call, and ordinary JSON that isn't one.
        let (text, calls, _) = find_calls("{\"name\": \"current_time\", \"arguments\": {}}");
        assert_eq!((text.as_str(), calls.len()), ("", 1));
        let (text, calls, _) = find_calls("{\"name\": \"Bob\", \"age\": 3}");
        assert_eq!((calls.len(), text.contains("Bob")), (0, true));
        let (text, calls, _) = find_calls("Just an answer.");
        assert_eq!((text.as_str(), calls.len()), ("Just an answer.", 0));
    }

    #[test]
    fn history_becomes_plain_text() {
        let mut call = ExMessage::new(Role::Assistant, "Checking.");
        call.calls = vec![ToolCall {
            id: "c1".into(),
            name: "calculator".into(),
            arguments: json!({ "expression": "1+1" }),
            network: crate::blocks::Switch::Off,
        }];
        let mut r1 = ExMessage::new(Role::Tool, "2");
        r1.name = Some("calculator".into());
        r1.call_id = Some("c1".into());
        let r2 = ExMessage {
            name: Some("calculator".into()),
            ..ExMessage::new(Role::Tool, "3")
        };
        let out = plain_history(&[ExMessage::new(Role::User, "hi"), call, r1, r2]);
        assert_eq!(out.len(), 3);
        assert!(out[1].text.contains("<tool_call>") && out[1].calls.is_empty());
        assert_eq!(out[2].role, Role::User);
        assert_eq!(out[2].text.matches("<tool_response>").count(), 2);
        // The model's own format round-trips through the parser.
        let (_, calls, _) = find_calls(&out[1].text);
        assert_eq!(calls[0].arguments["expression"], "1+1");
    }

    #[test]
    fn describes_tools() {
        let t = ToolInfo {
            name: "calculator".into(),
            description: "Works out sums.".into(),
            parameters: json!({ "type": "object", "properties": { "expression": { "type": "string" } } }),
        };
        assert!(tool_section(std::slice::from_ref(&t)).contains("\"name\":\"calculator\""));
        assert_eq!(tool_list(&[t]), "- calculator(expression): Works out sums.");
    }
}
