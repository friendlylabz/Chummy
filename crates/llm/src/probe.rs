//! "Test connection": checks a model end to end, including whether it can
//! call tools, which small models often can't do reliably.

use std::time::Instant;

use serde::Serialize;
use serde_json::json;

use crate::client::Client;
use crate::error::LlmError;
use crate::types::{ChatEvent, ChatRequest, Message, ToolSpec};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum StepStatus {
    Pass,
    Warn,
    Fail,
    Skipped,
}

#[derive(Debug, Clone, Serialize)]
pub struct ProbeStep {
    pub name: &'static str,
    pub status: StepStatus,
    pub detail: String,
    pub ms: u64,
}

#[derive(Debug, Clone, Serialize)]
pub struct ProbeReport {
    pub steps: Vec<ProbeStep>,
    /// Models the endpoint listed, if it supports listing.
    pub models: Vec<String>,
    /// True when the connection is usable (reply step passed).
    pub ok: bool,
    /// The context length the server reports for the model, if it does.
    pub context: Option<u32>,
}

fn step(
    name: &'static str,
    status: StepStatus,
    detail: impl Into<String>,
    t: Instant,
) -> ProbeStep {
    ProbeStep {
        name,
        status,
        detail: detail.into(),
        ms: t.elapsed().as_millis() as u64,
    }
}

fn models_text(n: usize) -> String {
    if n == 1 {
        "1 model".into()
    } else {
        format!("{n} models")
    }
}

pub async fn probe(client: &Client) -> ProbeReport {
    let mut steps = Vec::new();
    let mut models = Vec::new();

    // 1. Model list (optional: many servers don't implement it).
    let t = Instant::now();
    match client.list_models().await {
        Ok(list) => {
            let found = list.iter().any(|m| m == client.model());
            let detail = if list.is_empty() {
                "The server listed no models.".to_string()
            } else if found {
                format!(
                    "{} available, including {}.",
                    models_text(list.len()),
                    client.model()
                )
            } else {
                format!(
                    "{} available, but {} isn't one of them.",
                    models_text(list.len()),
                    client.model()
                )
            };
            let status = if found || list.is_empty() {
                StepStatus::Pass
            } else {
                StepStatus::Warn
            };
            steps.push(step("Reach the server", status, detail, t));
            models = list;
        }
        Err(e @ (LlmError::Connect { .. } | LlmError::Auth { .. } | LlmError::Config(_))) => {
            steps.push(step("Reach the server", StepStatus::Fail, e.to_string(), t));
            steps.push(step("Get a reply", StepStatus::Skipped, "", Instant::now()));
            steps.push(step("Call a tool", StepStatus::Skipped, "", Instant::now()));
            return ProbeReport {
                steps,
                models,
                ok: false,
                context: None,
            };
        }
        Err(e) => steps.push(step(
            "Reach the server",
            StepStatus::Warn,
            format!("Couldn't list models ({e}); trying a reply anyway."),
            t,
        )),
    }

    // 2. A plain reply.
    let t = Instant::now();
    let mut first_token_ms = None;
    let req = ChatRequest {
        messages: vec![Message::user("Reply with the single word: ready")],
        max_tokens: Some(256),
        temperature: Some(0.0),
        ..Default::default()
    };
    let reply = client
        .chat(&req, |ev| {
            if first_token_ms.is_none()
                && matches!(ev, ChatEvent::Text { .. } | ChatEvent::Reasoning { .. })
            {
                first_token_ms = Some(t.elapsed().as_millis());
            }
        })
        .await;
    let ok = match reply {
        Ok(r) if !r.text.trim().is_empty() => {
            let preview: String = r.text.trim().chars().take(60).collect();
            let speed = first_token_ms
                .map(|ms| format!(" First word after {ms} ms."))
                .unwrap_or_default();
            steps.push(step(
                "Get a reply",
                StepStatus::Pass,
                format!("“{preview}”.{speed}"),
                t,
            ));
            true
        }
        Ok(_) => {
            steps.push(step(
                "Get a reply",
                StepStatus::Warn,
                "The model answered with no text.",
                t,
            ));
            true
        }
        Err(e) => {
            steps.push(step("Get a reply", StepStatus::Fail, e.to_string(), t));
            steps.push(step("Call a tool", StepStatus::Skipped, "", Instant::now()));
            return ProbeReport {
                steps,
                models,
                ok: false,
                context: None,
            };
        }
    };

    // 3. A tool call.
    let t = Instant::now();
    let req = ChatRequest {
        exact_generation: false,
        messages: vec![Message::user(
            "What's the weather in Paris right now? Use the get_weather tool.",
        )],
        tools: vec![ToolSpec {
            name: "get_weather".into(),
            description: "Get the current weather for a city.".into(),
            parameters: json!({
                "type": "object",
                "properties": { "city": { "type": "string", "description": "City name" } },
                "required": ["city"]
            }),
        }],
        tool_choice: Default::default(),
        max_tokens: Some(512),
        temperature: Some(0.0),
        generation: Default::default(),
    };
    match client.chat(&req, |_| {}).await {
        Ok(r) => {
            let good = r.tool_calls.iter().find(|c| c.name == "get_weather");
            let s = match good {
                Some(c)
                    if c.arguments["city"]
                        .as_str()
                        .is_some_and(|city| city.to_lowercase().contains("paris")) =>
                {
                    step(
                        "Call a tool",
                        StepStatus::Pass,
                        format!("Called get_weather({}).", c.arguments),
                        t,
                    )
                }
                Some(c) => step(
                    "Call a tool",
                    StepStatus::Warn,
                    format!("Called the tool with unexpected arguments: {}", c.arguments),
                    t,
                ),
                None if !r.malformed.is_empty() => step(
                    "Call a tool",
                    StepStatus::Warn,
                    format!(
                        "Tried to call a tool but the arguments weren't valid JSON: {}",
                        r.malformed[0].1
                    ),
                    t,
                ),
                None => step(
                    "Call a tool",
                    StepStatus::Warn,
                    "The model answered without calling the tool. Agents that use tools may not work well with it.",
                    t,
                ),
            };
            steps.push(s);
        }
        Err(e) => steps.push(step(
            "Call a tool",
            StepStatus::Warn,
            format!("The server refused a request with tools: {e}"),
            t,
        )),
    }

    // 4. The context length (most servers say, OpenAI's own API doesn't).
    let t = Instant::now();
    let context = client.context_length().await;
    steps.push(match context {
        Some(n) => step(
            "Find the context length",
            StepStatus::Pass,
            format!("{} tokens, as the server reports.", thousands(n)),
            t,
        ),
        None => step(
            "Find the context length",
            StepStatus::Warn,
            format!(
                "The server doesn't say. Chummy assumes {} tokens unless you set it here.",
                thousands(crate::DEFAULT_CONTEXT)
            ),
            t,
        ),
    });

    ProbeReport {
        steps,
        models,
        ok,
        context,
    }
}

/// 32768 → "32,768".
fn thousands(n: u32) -> String {
    let s = n.to_string();
    let mut out = String::new();
    for (i, c) in s.chars().enumerate() {
        if i > 0 && (s.len() - i).is_multiple_of(3) {
            out.push(',');
        }
        out.push(c);
    }
    out
}
