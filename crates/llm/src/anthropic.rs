//! Anthropic Messages API wire format.

use std::collections::BTreeMap;

use serde_json::{Value, json};

use crate::types::{ChatEvent, ChatRequest, Role, StopReason, Usage, finish_tool_call};

pub(crate) const VERSION: &str = "2023-06-01";
const DEFAULT_MAX_TOKENS: u32 = 4096;

pub(crate) fn body(model: &str, req: &ChatRequest) -> Value {
    let system: Vec<&str> = req
        .messages
        .iter()
        .filter(|m| m.role == Role::System)
        .map(|m| m.content.as_str())
        .collect();

    // Anthropic wants alternating user/assistant turns of content blocks; tool
    // results are user-turn blocks. Consecutive same-role messages are merged.
    let mut turns: Vec<(String, Vec<Value>)> = Vec::new();
    for m in req.messages.iter().filter(|m| m.role != Role::System) {
        let (role, blocks) = match m.role {
            Role::User => ("user", vec![json!({ "type": "text", "text": m.content })]),
            Role::Tool => (
                "user",
                vec![json!({
                    "type": "tool_result",
                    "tool_use_id": m.tool_call_id.clone().unwrap_or_default(),
                    "content": m.content,
                })],
            ),
            Role::Assistant => {
                let mut b = Vec::new();
                if !m.content.is_empty() {
                    b.push(json!({ "type": "text", "text": m.content }));
                }
                for c in &m.tool_calls {
                    b.push(json!({ "type": "tool_use", "id": c.id, "name": c.name, "input": c.arguments }));
                }
                ("assistant", b)
            }
            Role::System => unreachable!(),
        };
        match turns.last_mut() {
            Some((r, existing)) if r == role => existing.extend(blocks),
            _ => turns.push((role.to_string(), blocks)),
        }
    }

    let mut b = json!({
        "model": model,
        "max_tokens": req.max_tokens.unwrap_or(DEFAULT_MAX_TOKENS),
        "messages": turns.into_iter().map(|(role, content)| json!({ "role": role, "content": content })).collect::<Vec<_>>(),
        "stream": true,
    });
    if !system.is_empty() {
        b["system"] = json!(system.join("\n\n"));
    }
    if !req.tools.is_empty() {
        b["tools"] = req
            .tools
            .iter()
            .map(|t| json!({ "name": t.name, "description": t.description, "input_schema": t.parameters }))
            .collect();
    }
    if let Some(t) = req.temperature {
        b["temperature"] = json!(t);
    }
    b
}

enum Block {
    Text,
    Thinking,
    Tool {
        id: String,
        name: String,
        json: String,
    },
}

#[derive(Default)]
pub(crate) struct Decoder {
    blocks: BTreeMap<u64, Block>,
    usage: Usage,
    stop: Option<StopReason>,
    done: bool,
}

impl Decoder {
    pub fn data(&mut self, data: &str) -> Result<Vec<ChatEvent>, String> {
        let v: Value = serde_json::from_str(data).map_err(|e| format!("bad event: {e}"))?;
        let mut out = Vec::new();
        match v["type"].as_str().unwrap_or_default() {
            "message_start" => {
                self.usage.input_tokens =
                    v["message"]["usage"]["input_tokens"].as_u64().unwrap_or(0) as u32;
            }
            "content_block_start" => {
                let idx = v["index"].as_u64().unwrap_or(0);
                let cb = &v["content_block"];
                let block = match cb["type"].as_str().unwrap_or_default() {
                    "tool_use" => Block::Tool {
                        id: cb["id"].as_str().unwrap_or_default().into(),
                        name: cb["name"].as_str().unwrap_or_default().into(),
                        json: String::new(),
                    },
                    "thinking" => Block::Thinking,
                    _ => Block::Text,
                };
                if let Block::Text = block
                    && let Some(t) = cb["text"].as_str().filter(|t| !t.is_empty())
                {
                    out.push(ChatEvent::Text { text: t.into() });
                }
                self.blocks.insert(idx, block);
            }
            "content_block_delta" => {
                let idx = v["index"].as_u64().unwrap_or(0);
                let d = &v["delta"];
                match (
                    d["type"].as_str().unwrap_or_default(),
                    self.blocks.get_mut(&idx),
                ) {
                    ("text_delta", _) => out.push(ChatEvent::Text {
                        text: d["text"].as_str().unwrap_or_default().into(),
                    }),
                    ("thinking_delta", _) => out.push(ChatEvent::Reasoning {
                        text: d["thinking"].as_str().unwrap_or_default().into(),
                    }),
                    ("input_json_delta", Some(Block::Tool { json, .. })) => {
                        json.push_str(d["partial_json"].as_str().unwrap_or_default())
                    }
                    _ => {}
                }
            }
            "content_block_stop" => {
                let idx = v["index"].as_u64().unwrap_or(0);
                if let Some(Block::Tool { id, name, json }) = self.blocks.remove(&idx) {
                    out.push(finish_tool_call(id, name, &json));
                }
            }
            "message_delta" => {
                if let Some(sr) = v["delta"]["stop_reason"].as_str() {
                    self.stop = Some(match sr {
                        "end_turn" | "stop_sequence" => StopReason::EndTurn,
                        "tool_use" => StopReason::ToolUse,
                        "max_tokens" => StopReason::MaxTokens,
                        other => StopReason::Other(other.into()),
                    });
                }
                if let Some(n) = v["usage"]["output_tokens"].as_u64() {
                    self.usage.output_tokens = n as u32;
                }
            }
            "message_stop" => out.extend(self.finish()),
            "error" => {
                return Err(v["error"]["message"]
                    .as_str()
                    .unwrap_or("server error")
                    .into());
            }
            _ => {} // ping and future event types
        }
        Ok(out)
    }

    pub fn finish(&mut self) -> Vec<ChatEvent> {
        if self.done {
            return vec![];
        }
        self.done = true;
        vec![ChatEvent::Done {
            stop: self.stop.take().unwrap_or(StopReason::EndTurn),
            usage: Some(self.usage),
        }]
    }
}

pub(crate) fn parse_models(v: &Value) -> Vec<String> {
    v["data"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|m| m["id"].as_str().map(str::to_string))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::{Message, ToolCall};

    #[test]
    fn decodes_text_and_tool_use() {
        let mut d = Decoder::default();
        let mut ev = vec![];
        for line in [
            r#"{"type":"message_start","message":{"usage":{"input_tokens":12}}}"#,
            r#"{"type":"content_block_start","index":0,"content_block":{"type":"text","text":""}}"#,
            r#"{"type":"content_block_delta","index":0,"delta":{"type":"text_delta","text":"Let me check."}}"#,
            r#"{"type":"content_block_stop","index":0}"#,
            r#"{"type":"content_block_start","index":1,"content_block":{"type":"tool_use","id":"toolu_1","name":"get_weather","input":{}}}"#,
            r#"{"type":"content_block_delta","index":1,"delta":{"type":"input_json_delta","partial_json":"{\"city\":"}}"#,
            r#"{"type":"content_block_delta","index":1,"delta":{"type":"input_json_delta","partial_json":" \"Paris\"}"}}"#,
            r#"{"type":"content_block_stop","index":1}"#,
            r#"{"type":"message_delta","delta":{"stop_reason":"tool_use"},"usage":{"output_tokens":30}}"#,
            r#"{"type":"message_stop"}"#,
        ] {
            ev.extend(d.data(line).unwrap());
        }
        assert_eq!(
            ev[0],
            ChatEvent::Text {
                text: "Let me check.".into()
            }
        );
        assert_eq!(
            ev[1],
            ChatEvent::ToolCall {
                call: ToolCall {
                    id: "toolu_1".into(),
                    name: "get_weather".into(),
                    arguments: json!({"city":"Paris"})
                }
            }
        );
        assert_eq!(
            ev[2],
            ChatEvent::Done {
                stop: StopReason::ToolUse,
                usage: Some(Usage {
                    input_tokens: 12,
                    output_tokens: 30
                })
            }
        );
    }

    #[test]
    fn merges_tool_results_into_user_turns() {
        let call = ToolCall {
            id: "t1".into(),
            name: "f".into(),
            arguments: json!({}),
        };
        let mut asst = Message::assistant("ok");
        asst.tool_calls = vec![call.clone()];
        let req = ChatRequest {
            messages: vec![
                Message::system("be brief"),
                Message::user("hi"),
                asst,
                Message::tool_result(&call, "r1"),
                Message::user("and?"),
            ],
            ..Default::default()
        };
        let b = body("claude", &req);
        assert_eq!(b["system"], "be brief");
        let msgs = b["messages"].as_array().unwrap();
        assert_eq!(msgs.len(), 3);
        assert_eq!(msgs[1]["content"][1]["type"], "tool_use");
        assert_eq!(msgs[2]["role"], "user");
        assert_eq!(msgs[2]["content"][0]["type"], "tool_result");
        assert_eq!(msgs[2]["content"][1]["text"], "and?");
    }
}
