//! Google Gemini `generateContent` wire format.

use serde_json::{Map, Value, json};

use crate::types::{
    ChatEvent, ChatRequest, Role, StopReason, ToolChoice, Usage, finish_tool_call, make_call_id,
};

pub(crate) fn body(req: &ChatRequest) -> Value {
    let system: Vec<&str> = req
        .messages
        .iter()
        .filter(|m| m.role == Role::System)
        .map(|m| m.content.as_str())
        .collect();

    let mut contents: Vec<(String, Vec<Value>)> = Vec::new();
    for m in req.messages.iter().filter(|m| m.role != Role::System) {
        let (role, parts) = match m.role {
            Role::User if m.content.trim().is_empty() => ("user", vec![]),
            Role::User => ("user", vec![json!({ "text": m.content })]),
            Role::Tool => (
                "user",
                vec![json!({ "functionResponse": {
                    "name": m.name.clone().unwrap_or_default(),
                    "response": { "content": m.content },
                }})],
            ),
            Role::Assistant => {
                let mut p = Vec::new();
                if !m.content.trim().is_empty() {
                    p.push(json!({ "text": m.content }));
                }
                for c in &m.tool_calls {
                    p.push(json!({ "functionCall": { "name": c.name, "args": c.arguments } }));
                }
                ("model", p)
            }
            Role::System => unreachable!(),
        };
        match contents.last_mut() {
            Some((r, existing)) if r == role => existing.extend(parts),
            _ => contents.push((role.to_string(), parts)),
        }
    }

    // Empty parts and a conversation starting with the model are refused.
    for (_, parts) in contents.iter_mut() {
        if parts.is_empty() {
            parts.push(json!({ "text": "(empty)" }));
        }
    }
    if contents.first().is_none_or(|(r, _)| r != "user") {
        contents.insert(0, ("user".into(), vec![json!({ "text": "(start)" })]));
    }

    let mut b = json!({
        "contents": contents.into_iter().map(|(role, parts)| json!({ "role": role, "parts": parts })).collect::<Vec<_>>(),
    });
    if !system.is_empty() {
        b["systemInstruction"] = json!({ "parts": [{ "text": system.join("\n\n") }] });
    }
    if !req.tools.is_empty() {
        b["tools"] = json!([{ "functionDeclarations": req.tools.iter().map(|t| json!({
            "name": t.name, "description": t.description, "parameters": clean_schema(&t.parameters),
        })).collect::<Vec<_>>() }]);
        if req.tool_choice == ToolChoice::Required {
            b["toolConfig"] = json!({ "functionCallingConfig": { "mode": "ANY" } });
        }
    }
    let mut cfg = Map::new();
    if let Some(t) = req.temperature {
        cfg.insert("temperature".into(), crate::generation::num(t));
    }
    if let Some(m) = req.max_tokens {
        cfg.insert("maxOutputTokens".into(), json!(m));
    }
    let g = &req.generation;
    for (key, v) in [
        ("topP", g.top_p),
        ("presencePenalty", g.presence_penalty),
        ("frequencyPenalty", g.frequency_penalty),
    ] {
        if let Some(x) = v {
            cfg.insert(key.into(), crate::generation::num(x));
        }
    }
    if let Some(k) = g.top_k {
        cfg.insert("topK".into(), json!(k));
    }
    if let Some(s) = g.seed {
        cfg.insert("seed".into(), json!(s));
    }
    if !g.stop.is_empty() {
        cfg.insert("stopSequences".into(), json!(g.stop));
    }
    if let Some(e) = g.reasoning_effort {
        cfg.insert(
            "thinkingConfig".into(),
            json!({ "thinkingBudget": e.gemini_budget() }),
        );
    }
    if !cfg.is_empty() {
        b["generationConfig"] = Value::Object(cfg);
    }
    b
}

/// Gemini accepts an OpenAPI subset of JSON Schema; drop keys it rejects.
fn clean_schema(v: &Value) -> Value {
    match v {
        Value::Object(m) => Value::Object(
            m.iter()
                .filter(|(k, _)| {
                    !matches!(
                        k.as_str(),
                        "$schema" | "additionalProperties" | "$id" | "examples"
                    )
                })
                .map(|(k, v)| (k.clone(), clean_schema(v)))
                .collect(),
        ),
        Value::Array(a) => Value::Array(a.iter().map(clean_schema).collect()),
        other => other.clone(),
    }
}

#[derive(Default)]
pub(crate) struct Decoder {
    calls: usize,
    stop: Option<StopReason>,
    usage: Option<Usage>,
    done: bool,
}

impl Decoder {
    pub fn data(&mut self, data: &str) -> Result<Vec<ChatEvent>, String> {
        let v: Value = serde_json::from_str(data).map_err(|e| format!("bad chunk: {e}"))?;
        if let Some(err) = v.get("error") {
            return Err(err["message"].as_str().unwrap_or("server error").into());
        }
        let mut out = Vec::new();
        if let Some(u) = v.get("usageMetadata") {
            self.usage = Some(Usage {
                input_tokens: u["promptTokenCount"].as_u64().unwrap_or(0) as u32,
                output_tokens: u["candidatesTokenCount"].as_u64().unwrap_or(0) as u32,
            });
        }
        let Some(cand) = v["candidates"].get(0) else {
            return Ok(out);
        };
        for part in cand["content"]["parts"].as_array().into_iter().flatten() {
            if let Some(t) = part["text"].as_str().filter(|t| !t.is_empty()) {
                if part["thought"].as_bool() == Some(true) {
                    out.push(ChatEvent::Reasoning { text: t.into() });
                } else {
                    out.push(ChatEvent::Text { text: t.into() });
                }
            }
            if let Some(fc) = part.get("functionCall") {
                let raw = fc.get("args").map(|a| a.to_string()).unwrap_or_default();
                out.push(finish_tool_call(
                    make_call_id(self.calls),
                    fc["name"].as_str().unwrap_or_default().into(),
                    &raw,
                ));
                self.calls += 1;
            }
        }
        if let Some(fr) = cand["finishReason"].as_str() {
            self.stop = Some(match fr {
                "STOP" => StopReason::EndTurn,
                "MAX_TOKENS" => StopReason::MaxTokens,
                other => StopReason::Other(other.to_lowercase()),
            });
        }
        Ok(out)
    }

    pub fn finish(&mut self) -> Vec<ChatEvent> {
        if self.done {
            return vec![];
        }
        self.done = true;
        let stop = if self.calls > 0 {
            StopReason::ToolUse
        } else {
            self.stop.take().unwrap_or(StopReason::EndTurn)
        };
        vec![ChatEvent::Done {
            stop,
            usage: self.usage,
        }]
    }
}

pub(crate) fn parse_models(v: &Value) -> Vec<String> {
    v["models"]
        .as_array()
        .into_iter()
        .flatten()
        .filter(|m| {
            m["supportedGenerationMethods"]
                .as_array()
                .is_some_and(|a| a.iter().any(|x| x == "generateContent"))
        })
        .filter_map(|m| {
            m["name"]
                .as_str()
                .map(|n| n.trim_start_matches("models/").to_string())
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn never_sends_empty_turns_or_starts_with_the_model() {
        let req = ChatRequest {
            messages: vec![
                Message::system("s"),
                Message::assistant("hello first"),
                Message::user(""),
                Message::assistant(""),
            ],
            ..Default::default()
        };
        let b = body(&req);
        let turns = b["contents"].as_array().unwrap();
        assert_eq!(turns[0]["role"], "user");
        for t in turns {
            let parts = t["parts"].as_array().unwrap();
            assert!(!parts.is_empty(), "{b}");
            assert!(parts.iter().all(|p| p["text"] != ""), "{b}");
        }
    }
    use crate::types::{Message, ToolCall, ToolSpec};

    #[test]
    fn decodes_text_and_function_calls() {
        let mut d = Decoder::default();
        let mut ev = d
            .data(r#"{"candidates":[{"content":{"role":"model","parts":[{"text":"Sure."},{"functionCall":{"name":"get_weather","args":{"city":"Paris"}}}]},"finishReason":"STOP"}],"usageMetadata":{"promptTokenCount":7,"candidatesTokenCount":3}}"#)
            .unwrap();
        ev.extend(d.finish());
        assert_eq!(
            ev[0],
            ChatEvent::Text {
                text: "Sure.".into()
            }
        );
        assert!(
            matches!(&ev[1], ChatEvent::ToolCall { call } if call.name == "get_weather" && call.arguments == json!({"city":"Paris"}))
        );
        assert!(matches!(
            &ev[2],
            ChatEvent::Done {
                stop: StopReason::ToolUse,
                usage: Some(Usage {
                    input_tokens: 7,
                    output_tokens: 3
                })
            }
        ));
    }

    #[test]
    fn sends_generation_settings_and_a_thinking_budget() {
        let req = ChatRequest {
            messages: vec![Message::user("q")],
            generation: crate::Generation {
                reasoning_effort: Some(crate::ReasoningEffort::None),
                top_k: Some(20),
                stop: vec!["X".into()],
                ..Default::default()
            },
            ..Default::default()
        };
        let c = &body(&req)["generationConfig"];
        assert_eq!(c["topK"], 20);
        assert_eq!(c["stopSequences"][0], "X");
        assert_eq!(c["thinkingConfig"]["thinkingBudget"], 0);
    }

    #[test]
    fn builds_body_with_clean_schema_and_function_responses() {
        let call = ToolCall {
            id: "c".into(),
            name: "f".into(),
            arguments: json!({"a":1}),
        };
        let mut asst = Message::assistant("");
        asst.tool_calls = vec![call.clone()];
        let req = ChatRequest {
            messages: vec![
                Message::system("sys"),
                Message::user("q"),
                asst,
                Message::tool_result(&call, "res"),
            ],
            tools: vec![ToolSpec {
                name: "f".into(),
                description: "d".into(),
                parameters: json!({"type":"object","additionalProperties":false,"properties":{"a":{"type":"integer"}}}),
            }],
            ..Default::default()
        };
        let b = body(&req);
        assert_eq!(b["systemInstruction"]["parts"][0]["text"], "sys");
        assert_eq!(b["contents"][1]["role"], "model");
        assert_eq!(
            b["contents"][2]["parts"][0]["functionResponse"]["name"],
            "f"
        );
        let params = &b["tools"][0]["functionDeclarations"][0]["parameters"];
        assert!(params.get("additionalProperties").is_none());
        assert_eq!(params["properties"]["a"]["type"], "integer");
    }
}
