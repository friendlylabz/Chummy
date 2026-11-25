//! OpenAI Chat Completions wire format (also spoken by Ollama, vLLM, llama.cpp,
//! LM Studio, OpenRouter and the built-in engine).

use std::collections::BTreeMap;

use serde_json::{Value, json};

use crate::generation::num;

use crate::types::{
    ChatEvent, ChatRequest, Message, Role, StopReason, ToolChoice, Usage, finish_tool_call,
    make_call_id,
};

/// `official`: the request goes to OpenAI itself (usage in the stream, and
/// `max_completion_tokens`, which its reasoning models need).
pub(crate) fn body(model: &str, req: &ChatRequest, official: bool) -> Value {
    let mut b = json!({
        "model": model,
        "messages": req.messages.iter().map(message).collect::<Vec<_>>(),
        "stream": true,
    });
    if official {
        b["stream_options"] = json!({ "include_usage": true });
    }
    if !req.tools.is_empty() {
        b["tools"] = req
            .tools
            .iter()
            .map(|t| {
                json!({ "type": "function", "function": {
                    "name": t.name, "description": t.description, "parameters": t.parameters,
                }})
            })
            .collect();
        if req.tool_choice == ToolChoice::Required {
            b["tool_choice"] = json!("required");
        }
    }
    if let Some(t) = req.temperature {
        b["temperature"] = num(t);
    }
    if let Some(m) = req.max_tokens {
        b[if official {
            "max_completion_tokens"
        } else {
            "max_tokens"
        }] = json!(m);
    }
    let g = &req.generation;
    if let Some(e) = g.reasoning_effort {
        b["reasoning_effort"] = json!(e.as_str());
    }
    for (key, v) in [
        ("top_p", g.top_p),
        ("min_p", g.min_p),
        ("presence_penalty", g.presence_penalty),
        ("frequency_penalty", g.frequency_penalty),
        // llama.cpp and LM Studio say repeat_penalty, vLLM repetition_penalty.
        ("repeat_penalty", g.repeat_penalty),
        ("repetition_penalty", g.repeat_penalty),
    ] {
        if let Some(x) = v {
            b[key] = num(x);
        }
    }
    if let Some(k) = g.top_k {
        b["top_k"] = json!(k);
    }
    if let Some(s) = g.seed {
        b["seed"] = json!(s);
    }
    if !g.stop.is_empty() {
        b["stop"] = json!(g.stop);
    }
    b
}

fn message(m: &Message) -> Value {
    match m.role {
        Role::System => json!({ "role": "system", "content": m.content }),
        Role::User => json!({ "role": "user", "content": m.content }),
        Role::Tool => json!({
            "role": "tool",
            "tool_call_id": m.tool_call_id.clone().unwrap_or_default(),
            "content": m.content,
        }),
        Role::Assistant => {
            let mut v = json!({ "role": "assistant", "content": m.content });
            if !m.tool_calls.is_empty() {
                v["tool_calls"] = m
                    .tool_calls
                    .iter()
                    .map(|c| {
                        json!({ "id": c.id, "type": "function", "function": {
                            "name": c.name, "arguments": c.arguments.to_string(),
                        }})
                    })
                    .collect();
            }
            v
        }
    }
}

#[derive(Default)]
struct PartialCall {
    id: String,
    name: String,
    args: String,
}

/// Turns streamed `chat.completion.chunk` objects into [`ChatEvent`]s.
#[derive(Default)]
pub(crate) struct Decoder {
    calls: BTreeMap<u64, PartialCall>,
    stop: Option<StopReason>,
    usage: Option<Usage>,
    done: bool,
}

impl Decoder {
    pub fn data(&mut self, data: &str) -> Result<Vec<ChatEvent>, String> {
        if data.trim() == "[DONE]" {
            return Ok(self.finish());
        }
        let v: Value = serde_json::from_str(data).map_err(|e| format!("bad chunk: {e}"))?;
        if let Some(err) = v.get("error") {
            return Err(err["message"]
                .as_str()
                .unwrap_or("server error")
                .to_string());
        }
        let mut out = Vec::new();

        if let Some(u) = v.get("usage").filter(|u| !u.is_null()) {
            self.usage = Some(Usage {
                input_tokens: u["prompt_tokens"].as_u64().unwrap_or(0) as u32,
                output_tokens: u["completion_tokens"].as_u64().unwrap_or(0) as u32,
            });
        }

        let Some(choice) = v["choices"].get(0) else {
            return Ok(out);
        };
        let delta = &choice["delta"];

        // Servers name this differently; some send both with the same text.
        if let Some(r) = ["reasoning_content", "reasoning"]
            .iter()
            .find_map(|k| delta[*k].as_str().filter(|s| !s.is_empty()))
        {
            out.push(ChatEvent::Reasoning {
                text: r.to_string(),
            });
        }
        if let Some(t) = delta["content"].as_str().filter(|s| !s.is_empty()) {
            out.push(ChatEvent::Text {
                text: t.to_string(),
            });
        }
        if let Some(calls) = delta["tool_calls"].as_array() {
            for (i, c) in calls.iter().enumerate() {
                let id = c["id"].as_str().filter(|s| !s.is_empty());
                let idx = match c["index"].as_u64() {
                    Some(n) => n,
                    // Without an index, a new id means a new call.
                    None => {
                        let i = i as u64;
                        match (id, self.calls.get(&i)) {
                            (Some(id), Some(e)) if !e.id.is_empty() && e.id != id => {
                                self.calls.keys().max().map_or(0, |k| k + 1)
                            }
                            _ => i,
                        }
                    }
                };
                let entry = self.calls.entry(idx).or_default();
                if let Some(id) = id {
                    entry.id = id.to_string();
                }
                if let Some(n) = c["function"]["name"].as_str() {
                    // Some servers repeat the whole name in every chunk.
                    if entry.name != n {
                        entry.name.push_str(n);
                    }
                }
                if let Some(a) = c["function"]["arguments"].as_str() {
                    entry.args.push_str(a);
                } else if let Some(obj) = c["function"]["arguments"].as_object() {
                    // Some servers send arguments as an object instead of a string.
                    entry.args = Value::Object(obj.clone()).to_string();
                }
            }
        }
        if let Some(fr) = choice["finish_reason"].as_str() {
            self.stop = Some(match fr {
                "stop" => StopReason::EndTurn,
                "tool_calls" | "function_call" => StopReason::ToolUse,
                "length" => StopReason::MaxTokens,
                other => StopReason::Other(other.to_string()),
            });
        }
        Ok(out)
    }

    /// Emits completed tool calls and `Done`. Safe to call more than once.
    pub fn finish(&mut self) -> Vec<ChatEvent> {
        if self.done {
            return vec![];
        }
        self.done = true;
        let mut out = Vec::new();
        let had_calls = !self.calls.is_empty();
        for (n, (_, c)) in std::mem::take(&mut self.calls).into_iter().enumerate() {
            let id = if c.id.is_empty() {
                make_call_id(n)
            } else {
                c.id
            };
            out.push(finish_tool_call(id, c.name, &c.args));
        }
        let stop = match self.stop.take() {
            Some(StopReason::EndTurn) | None if had_calls => StopReason::ToolUse,
            Some(s) => s,
            None => StopReason::EndTurn,
        };
        out.push(ChatEvent::Done {
            stop,
            usage: self.usage,
        });
        out
    }
}

/// Model ids from a `/models` response.
pub(crate) fn parse_models(v: &Value) -> Vec<String> {
    let mut ids: Vec<String> = v["data"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|m| m["id"].as_str().map(str::to_string))
        .collect();
    ids.sort();
    ids
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::{ToolCall, ToolSpec};

    fn run(chunks: &[&str]) -> Vec<ChatEvent> {
        let mut d = Decoder::default();
        let mut out = vec![];
        for c in chunks {
            out.extend(d.data(c).unwrap());
        }
        out.extend(d.finish());
        out
    }

    #[test]
    fn tolerates_server_quirks() {
        // Reasoning under both names, a name repeated in every chunk, and
        // two calls without an index.
        let ev = run(&[
            r#"{"choices":[{"delta":{"reasoning_content":"think","reasoning":"think"}}]}"#,
            r#"{"choices":[{"delta":{"tool_calls":[{"index":0,"id":"a","function":{"name":"read_file","arguments":"{\"pa"}}]}}]}"#,
            r#"{"choices":[{"delta":{"tool_calls":[{"index":0,"function":{"name":"read_file","arguments":"th\":\"x\"}"}}]}}]}"#,
        ]);
        let reasoning = ev
            .iter()
            .filter(|e| matches!(e, ChatEvent::Reasoning { .. }))
            .count();
        assert_eq!(reasoning, 1);
        assert!(ev.iter().any(|e| matches!(e, ChatEvent::ToolCall { call } if call.name == "read_file" && call.arguments["path"] == "x")), "{ev:?}");

        let ev = run(&[
            r#"{"choices":[{"delta":{"tool_calls":[{"id":"a","function":{"name":"one","arguments":"{}"}}]}}]}"#,
            r#"{"choices":[{"delta":{"tool_calls":[{"id":"b","function":{"name":"two","arguments":"{}"}}]}}]}"#,
        ]);
        let names: Vec<String> = ev
            .iter()
            .filter_map(|e| match e {
                ChatEvent::ToolCall { call } => Some(call.name.clone()),
                _ => None,
            })
            .collect();
        assert_eq!(names, vec!["one", "two"]);
    }

    #[test]
    fn streams_text_and_usage() {
        let ev = run(&[
            r#"{"choices":[{"delta":{"role":"assistant","content":"Hel"}}]}"#,
            r#"{"choices":[{"delta":{"content":"lo"},"finish_reason":"stop"}]}"#,
            r#"{"choices":[],"usage":{"prompt_tokens":5,"completion_tokens":2}}"#,
            "[DONE]",
        ]);
        assert_eq!(ev[0], ChatEvent::Text { text: "Hel".into() });
        assert_eq!(ev[1], ChatEvent::Text { text: "lo".into() });
        assert_eq!(
            ev[2],
            ChatEvent::Done {
                stop: StopReason::EndTurn,
                usage: Some(Usage {
                    input_tokens: 5,
                    output_tokens: 2
                })
            }
        );
    }

    #[test]
    fn assembles_tool_call_fragments() {
        let ev = run(&[
            r#"{"choices":[{"delta":{"tool_calls":[{"index":0,"id":"call_1","type":"function","function":{"name":"get_weather","arguments":""}}]}}]}"#,
            r#"{"choices":[{"delta":{"tool_calls":[{"index":0,"function":{"arguments":"{\"city\": "}}]}}]}"#,
            r#"{"choices":[{"delta":{"tool_calls":[{"index":0,"function":{"arguments":"\"Paris\"}"}}]}}]}"#,
            r#"{"choices":[{"delta":{"tool_calls":[{"index":1,"id":"call_2","function":{"name":"bad","arguments":"{oops"}}]},"finish_reason":"tool_calls"}]}"#,
        ]);
        assert_eq!(
            ev[0],
            ChatEvent::ToolCall {
                call: ToolCall {
                    id: "call_1".into(),
                    name: "get_weather".into(),
                    arguments: json!({"city": "Paris"})
                }
            }
        );
        assert!(
            matches!(&ev[1], ChatEvent::MalformedToolCall { name, raw, .. } if name == "bad" && raw == "{oops")
        );
        assert!(matches!(
            &ev[2],
            ChatEvent::Done {
                stop: StopReason::ToolUse,
                ..
            }
        ));
    }

    #[test]
    fn reads_reasoning_fields() {
        let ev = run(&[r#"{"choices":[{"delta":{"reasoning_content":"hmm"}}]}"#]);
        assert_eq!(ev[0], ChatEvent::Reasoning { text: "hmm".into() });
    }

    #[test]
    fn builds_request_body() {
        let call = ToolCall {
            id: "c1".into(),
            name: "f".into(),
            arguments: json!({"a": 1}),
        };
        let mut asst = Message::assistant("");
        asst.tool_calls = vec![call.clone()];
        let req = ChatRequest {
            exact_generation: false,
            messages: vec![
                Message::system("s"),
                Message::user("u"),
                asst,
                Message::tool_result(&call, "42"),
            ],
            tools: vec![ToolSpec {
                name: "f".into(),
                description: "d".into(),
                parameters: json!({"type":"object"}),
            }],
            tool_choice: ToolChoice::Required,
            temperature: Some(0.2),
            max_tokens: Some(10),
            generation: crate::Generation {
                reasoning_effort: Some(crate::ReasoningEffort::Low),
                top_p: Some(0.5),
                repeat_penalty: Some(1.1),
                seed: Some(3),
                stop: vec!["END".into()],
                ..Default::default()
            },
        };
        let b = body("m", &req, true);
        assert_eq!(b["tool_choice"], "required");
        assert_eq!(
            b["messages"][2]["tool_calls"][0]["function"]["arguments"],
            "{\"a\":1}"
        );
        assert_eq!(b["messages"][3]["tool_call_id"], "c1");
        assert_eq!(b["tools"][0]["function"]["name"], "f");
        assert_eq!(b["stream_options"]["include_usage"], true);
        // OpenAI itself: max_completion_tokens (its reasoning models need it).
        assert_eq!(b["max_completion_tokens"], 10);
        assert!(b.get("max_tokens").is_none());
        assert_eq!(b["reasoning_effort"], "low");
        assert_eq!(b["top_p"], 0.5);
        assert_eq!(b["seed"], 3);
        assert_eq!(b["stop"][0], "END");
        assert!(b.get("top_k").is_none());
        let other = body("m", &req, false);
        assert_eq!(other["max_tokens"], 10);
        assert_eq!(other["repeat_penalty"], 1.1);
        assert_eq!(other["temperature"], 0.2);
        assert_eq!(other["repetition_penalty"], other["repeat_penalty"]);
    }
}
