//! The HTTP client that runs requests through the right adapter.

use std::time::Duration;

use futures_util::StreamExt;
use reqwest::header::{CONTENT_TYPE, HeaderMap, HeaderValue};
use serde_json::Value;

use crate::config::{ProviderConfig, ProviderKind};
use crate::error::LlmError;
use crate::sse::SseParser;
use crate::types::{ChatEvent, ChatRequest, ChatResponse};
use crate::{anthropic, gemini, openai};

/// The most an answer may be, streamed or not. Real answers are far smaller
/// (32k tokens is about 130 KB); this only stops a broken or hostile server
/// from growing Chummy's memory without end.
pub(crate) const MAX_ANSWER_BYTES: usize = 32 * 1024 * 1024;
/// The most read of an error message or a model list.
pub(crate) const MAX_SMALL_BYTES: usize = 4 * 1024 * 1024;

/// Reads a response body, up to `limit` bytes.
pub(crate) async fn body_text(resp: reqwest::Response, limit: usize) -> Result<String, String> {
    let mut out = Vec::new();
    let mut stream = resp.bytes_stream();
    while let Some(chunk) = stream.next().await {
        let chunk = chunk.map_err(|e| e.to_string())?;
        out.extend_from_slice(&chunk);
        if out.len() > limit {
            return Err(format!(
                "the server's answer is longer than {} MB",
                limit / (1024 * 1024)
            ));
        }
    }
    Ok(String::from_utf8_lossy(&out).into_owned())
}

/// A ready-to-use connection to one model.
#[derive(Clone)]
pub struct Client {
    kind: ProviderKind,
    base_url: String,
    model: String,
    api_key: Option<String>,
    default_temperature: Option<f32>,
    generation: crate::Generation,
    http: reqwest::Client,
}

enum Decoder {
    OpenAi(openai::Decoder),
    Anthropic(anthropic::Decoder),
    Gemini(gemini::Decoder),
}

impl Decoder {
    fn data(&mut self, d: &str) -> Result<Vec<ChatEvent>, String> {
        match self {
            Decoder::OpenAi(x) => x.data(d),
            Decoder::Anthropic(x) => x.data(d),
            Decoder::Gemini(x) => x.data(d),
        }
    }
    fn finish(&mut self) -> Vec<ChatEvent> {
        match self {
            Decoder::OpenAi(x) => x.finish(),
            Decoder::Anthropic(x) => x.finish(),
            Decoder::Gemini(x) => x.finish(),
        }
    }
}

impl Client {
    /// Builds a client. `api_key` comes from the keychain; `None` means the
    /// endpoint needs no key.
    pub fn new(cfg: &ProviderConfig, api_key: Option<String>) -> Result<Self, LlmError> {
        let base = cfg.base_url.trim().trim_end_matches('/').to_string();
        if base.is_empty() {
            return Err(LlmError::Config(
                "this connection has no server address".into(),
            ));
        }
        if !(base.starts_with("http://") || base.starts_with("https://")) {
            return Err(LlmError::Config(
                "the server address must start with http:// or https://".into(),
            ));
        }
        if cfg.model.trim().is_empty() {
            return Err(LlmError::Config(
                "choose a model for this connection".into(),
            ));
        }
        let http = chummy_http::builder()
            .connect_timeout(Duration::from_secs(10))
            // Time between bytes, not total: local models can think for a while.
            .read_timeout(Duration::from_secs(300))
            .user_agent(concat!("Chummy/", env!("CARGO_PKG_VERSION")))
            .build()
            .map_err(|e| LlmError::Config(e.to_string()))?;
        Ok(Self {
            kind: cfg.kind,
            base_url: base,
            model: cfg.model.trim().to_string(),
            // Pasted keys often carry a newline, which makes the header invalid.
            api_key: api_key
                .map(|k| k.trim().to_string())
                .filter(|k| !k.is_empty()),
            default_temperature: cfg.temperature,
            generation: cfg.generation.clone(),
            http,
        })
    }

    pub fn model(&self) -> &str {
        &self.model
    }

    pub(crate) fn kind(&self) -> ProviderKind {
        self.kind
    }

    pub(crate) fn http(&self) -> &reqwest::Client {
        &self.http
    }

    pub(crate) fn auth_headers(&self) -> HeaderMap {
        self.headers()
    }

    pub(crate) fn root_url(&self, suffix: &str) -> &str {
        self.root(suffix)
    }

    pub(crate) fn model_list_url(&self) -> String {
        self.models_url()
    }

    fn headers(&self) -> HeaderMap {
        let mut h = HeaderMap::new();
        let key = self.api_key.as_deref().unwrap_or_default();
        let val = |s: &str| HeaderValue::from_str(s).unwrap_or(HeaderValue::from_static(""));
        match self.kind {
            ProviderKind::Anthropic => {
                h.insert("x-api-key", val(key));
                h.insert(
                    "anthropic-version",
                    HeaderValue::from_static(anthropic::VERSION),
                );
            }
            ProviderKind::Gemini => {
                h.insert("x-goog-api-key", val(key));
            }
            _ => {
                if !key.is_empty() {
                    h.insert("authorization", val(&format!("Bearer {key}")));
                }
                h.insert("x-title", HeaderValue::from_static("Chummy"));
            }
        }
        h
    }

    /// Base URL with any version suffix the user pasted removed.
    fn root(&self, suffix: &str) -> &str {
        self.base_url.strip_suffix(suffix).unwrap_or(&self.base_url)
    }

    fn chat_url(&self) -> String {
        match self.kind {
            ProviderKind::Anthropic => format!("{}/v1/messages", self.root("/v1")),
            ProviderKind::Gemini => format!(
                "{}/v1beta/models/{}:streamGenerateContent?alt=sse",
                self.root("/v1beta"),
                self.model.strip_prefix("models/").unwrap_or(&self.model)
            ),
            _ => format!("{}/chat/completions", self.base_url),
        }
    }

    fn models_url(&self) -> String {
        match self.kind {
            ProviderKind::Anthropic => format!("{}/v1/models?limit=1000", self.root("/v1")),
            ProviderKind::Gemini => format!("{}/v1beta/models?pageSize=1000", self.root("/v1beta")),
            _ => format!("{}/models", self.base_url),
        }
    }

    /// Lists the models the endpoint offers.
    pub async fn list_models(&self) -> Result<Vec<String>, LlmError> {
        let url = self.models_url();
        let resp = self
            .http
            .get(&url)
            .headers(self.headers())
            .timeout(Duration::from_secs(20))
            .send()
            .await
            .map_err(|e| LlmError::from_reqwest(&url, e))?;
        let status = resp.status().as_u16();
        let text = body_text(resp, MAX_SMALL_BYTES)
            .await
            .map_err(LlmError::Protocol)?;
        if !(200..300).contains(&status) {
            return Err(LlmError::from_status(status, &text));
        }
        let v: Value = serde_json::from_str(&text)
            .map_err(|e| LlmError::Protocol(format!("model list isn't JSON: {e}")))?;
        Ok(match self.kind {
            ProviderKind::Anthropic => anthropic::parse_models(&v),
            ProviderKind::Gemini => gemini::parse_models(&v),
            _ => openai::parse_models(&v),
        })
    }

    /// Sends a chat request and streams the answer, calling `on_event` for each
    /// event as it arrives. Returns everything collected.
    pub async fn chat(
        &self,
        req: &ChatRequest,
        mut on_event: impl FnMut(&ChatEvent),
    ) -> Result<ChatResponse, LlmError> {
        let mut req = req.clone();
        if req.temperature.is_none() {
            req.temperature = self.default_temperature;
        }
        req.generation = if req.exact_generation {
            req.generation.for_kind(self.kind)
        } else {
            req.generation.or(&self.generation).for_kind(self.kind)
        };
        let (body, mut decoder) = match self.kind {
            k if k.speaks_openai() => (
                openai::body(&self.model, &req, k == ProviderKind::OpenAi),
                Decoder::OpenAi(Default::default()),
            ),
            ProviderKind::Anthropic => (
                anthropic::body(&self.model, &req),
                Decoder::Anthropic(Default::default()),
            ),
            _ => (gemini::body(&req), Decoder::Gemini(Default::default())),
        };

        let url = self.chat_url();
        let resp = self
            .http
            .post(&url)
            .headers(self.headers())
            .json(&body)
            .send()
            .await
            .map_err(|e| LlmError::from_reqwest(&url, e))?;

        let status = resp.status().as_u16();
        if !(200..300).contains(&status) {
            let text = body_text(resp, MAX_SMALL_BYTES).await.unwrap_or_default();
            return Err(LlmError::from_status(status, &text));
        }

        let mut out = ChatResponse::default();
        let mut emit = |events: Vec<ChatEvent>, out: &mut ChatResponse| {
            for ev in events {
                on_event(&ev);
                out.absorb(&ev);
            }
        };

        let is_sse = resp
            .headers()
            .get(CONTENT_TYPE)
            .and_then(|v| v.to_str().ok())
            .is_some_and(|ct| ct.contains("event-stream"));

        if !is_sse {
            // Some servers ignore `stream: true` and answer in one JSON body.
            let text = body_text(resp, MAX_ANSWER_BYTES)
                .await
                .map_err(LlmError::Stream)?;
            let chunk = non_streamed_as_chunk(&text)?;
            let events = decoder.data(&chunk).map_err(LlmError::Protocol)?;
            emit(events, &mut out);
            emit(decoder.finish(), &mut out);
            return Ok(out);
        }

        let mut parser = SseParser::default();
        let mut stream = resp.bytes_stream();
        let mut received = 0usize;
        while let Some(chunk) = stream.next().await {
            let chunk = chunk.map_err(|e| LlmError::Stream(e.to_string()))?;
            received += chunk.len();
            if received > MAX_ANSWER_BYTES {
                return Err(LlmError::Stream(format!(
                    "the answer is longer than {} MB, so Chummy stopped reading it",
                    MAX_ANSWER_BYTES / (1024 * 1024)
                )));
            }
            for sse in parser.feed(&chunk) {
                let events = decoder.data(&sse.data).map_err(LlmError::Stream)?;
                emit(events, &mut out);
            }
        }
        for sse in parser.finish() {
            let events = decoder.data(&sse.data).map_err(LlmError::Stream)?;
            emit(events, &mut out);
        }
        emit(decoder.finish(), &mut out);
        Ok(out)
    }
}

/// Converts a non-streamed OpenAI completion into one streaming-style chunk.
fn non_streamed_as_chunk(text: &str) -> Result<String, LlmError> {
    let v: Value = serde_json::from_str(text)
        .map_err(|e| LlmError::Protocol(format!("response isn't JSON: {e}")))?;
    let choice = &v["choices"][0];
    if choice.is_null() {
        return Err(LlmError::Protocol("response has no choices".into()));
    }
    let mut delta = choice["message"].clone();
    if let Some(calls) = delta["tool_calls"].as_array_mut() {
        for (i, c) in calls.iter_mut().enumerate() {
            c["index"] = i.into();
        }
    }
    Ok(serde_json::json!({
        "choices": [{ "delta": delta, "finish_reason": choice["finish_reason"] }],
        "usage": v["usage"],
    })
    .to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::{Message, ToolSpec};
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    /// Serves one canned HTTP response and returns the request it received.
    async fn serve_once(
        content_type: &'static str,
        body: String,
    ) -> (String, tokio::task::JoinHandle<String>) {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let handle = tokio::spawn(async move {
            let (mut sock, _) = listener.accept().await.unwrap();
            let mut req = Vec::new();
            let mut buf = [0u8; 4096];
            loop {
                let n = sock.read(&mut buf).await.unwrap();
                req.extend_from_slice(&buf[..n]);
                let s = String::from_utf8_lossy(&req);
                if let Some(h) = s.find("\r\n\r\n") {
                    let len = s[..h]
                        .lines()
                        .find_map(|l| {
                            l.to_ascii_lowercase()
                                .strip_prefix("content-length:")
                                .map(|v| v.trim().parse::<usize>().unwrap())
                        })
                        .unwrap_or(0);
                    if req.len() >= h + 4 + len {
                        break;
                    }
                }
            }
            let head = format!(
                "HTTP/1.1 200 OK\r\ncontent-type: {content_type}\r\ncontent-length: {}\r\nconnection: close\r\n\r\n",
                body.len()
            );
            sock.write_all(head.as_bytes()).await.unwrap();
            sock.write_all(body.as_bytes()).await.unwrap();
            String::from_utf8_lossy(&req).into_owned()
        });
        (format!("http://{addr}/v1"), handle)
    }

    fn cfg(base: String, kind: ProviderKind) -> ProviderConfig {
        ProviderConfig {
            id: "t".into(),
            name: "t".into(),
            kind,
            base_url: base,
            model: "m".into(),
            local: None,
            temperature: None,
            daily_tokens: Some(crate::DEFAULT_DAILY_TOKENS),
            max_tokens: crate::DEFAULT_MAX_TOKENS,
            context_max: None,
            context_tokens: None,
            compact_at: None,
            use_in_app: false,
            generation: Default::default(),
        }
    }

    #[tokio::test]
    async fn streams_a_tool_call_end_to_end() {
        let body = [
            r#"{"choices":[{"delta":{"content":"Checking. "}}]}"#,
            r#"{"choices":[{"delta":{"tool_calls":[{"index":0,"id":"c1","function":{"name":"get_weather","arguments":"{\"city\":\"Paris\"}"}}]},"finish_reason":"tool_calls"}]}"#,
            "[DONE]",
        ]
        .iter()
        .map(|d| format!("data: {d}\n\n"))
        .collect::<String>();
        let (base, server) = serve_once("text/event-stream", body).await;
        let client = Client::new(
            &cfg(base, ProviderKind::OpenAiCompatible),
            Some("sk-test".into()),
        )
        .unwrap();

        let req = ChatRequest {
            messages: vec![Message::user("weather?")],
            tools: vec![ToolSpec {
                name: "get_weather".into(),
                description: "".into(),
                parameters: serde_json::json!({"type":"object"}),
            }],
            ..Default::default()
        };
        let mut seen = 0;
        let resp = client.chat(&req, |_| seen += 1).await.unwrap();
        assert_eq!(resp.text, "Checking. ");
        assert_eq!(resp.tool_calls[0].arguments["city"], "Paris");
        assert_eq!(seen, 3);

        let raw = server.await.unwrap();
        assert!(raw.starts_with("POST /v1/chat/completions"));
        assert!(
            raw.to_ascii_lowercase()
                .contains("authorization: bearer sk-test")
        );
    }

    #[tokio::test]
    async fn accepts_non_streamed_answers() {
        let body = r#"{"choices":[{"message":{"role":"assistant","content":"hi"},"finish_reason":"stop"}],"usage":{"prompt_tokens":1,"completion_tokens":1}}"#;
        let (base, _server) = serve_once("application/json", body.into()).await;
        let client = Client::new(&cfg(base, ProviderKind::Ollama), None).unwrap();
        let resp = client
            .chat(
                &ChatRequest {
                    messages: vec![Message::user("x")],
                    ..Default::default()
                },
                |_| {},
            )
            .await
            .unwrap();
        assert_eq!(resp.text, "hi");
        assert_eq!(resp.usage.unwrap().output_tokens, 1);
    }

    #[tokio::test]
    async fn sends_the_connections_generation_settings_it_supports() {
        let body = r#"{"choices":[{"message":{"role":"assistant","content":"hi"},"finish_reason":"stop"}]}"#;
        let (base, server) = serve_once("application/json", body.into()).await;
        let mut c = cfg(base, ProviderKind::OpenAi);
        c.generation = crate::Generation {
            top_p: Some(0.5),
            min_p: Some(0.1), // OpenAI doesn't take it: never sent
            seed: Some(1),
            ..Default::default()
        };
        let client = Client::new(&c, Some("sk".into())).unwrap();
        let req = ChatRequest {
            messages: vec![Message::user("x")],
            // The request's own setting wins over the connection's.
            generation: crate::Generation {
                seed: Some(9),
                ..Default::default()
            },
            ..Default::default()
        };
        client.chat(&req, |_| {}).await.unwrap();
        let raw = server.await.unwrap();
        assert!(raw.contains(r#""top_p":0.5"#), "{raw}");
        assert!(raw.contains(r#""seed":9"#), "{raw}");
        assert!(!raw.contains("min_p"), "{raw}");
    }

    #[tokio::test]
    async fn exact_settings_leave_out_what_the_block_turned_off() {
        let body = r#"{"choices":[{"message":{"role":"assistant","content":"hi"},"finish_reason":"stop"}]}"#;
        let (base, server) = serve_once("application/json", body.into()).await;
        let mut c = cfg(base, ProviderKind::OpenAiCompatible);
        c.generation = crate::Generation {
            top_p: Some(0.5),
            seed: Some(1),
            ..Default::default()
        };
        let client = Client::new(&c, None).unwrap();
        // A block that turned top_p off and set its own effort.
        let req = ChatRequest {
            messages: vec![Message::user("x")],
            generation: crate::Generation {
                seed: Some(1),
                reasoning_effort: Some(crate::ReasoningEffort::Low),
                ..Default::default()
            },
            exact_generation: true,
            ..Default::default()
        };
        client.chat(&req, |_| {}).await.unwrap();
        let raw = server.await.unwrap();
        assert!(!raw.contains("top_p"), "{raw}");
        assert!(raw.contains(r#""seed":1"#), "{raw}");
        assert!(raw.contains(r#""reasoning_effort":"low""#), "{raw}");
    }

    #[test]
    fn rejects_bad_config() {
        assert!(Client::new(&cfg("".into(), ProviderKind::OpenAiCompatible), None).is_err());
        assert!(Client::new(&cfg("ftp://x".into(), ProviderKind::OpenAiCompatible), None).is_err());

        // (was urls_tolerate_pasted_suffixes)
        {
            let c = Client::new(
                &cfg(
                    "https://api.anthropic.com/v1/".into(),
                    ProviderKind::Anthropic,
                ),
                None,
            )
            .unwrap();
            assert_eq!(c.chat_url(), "https://api.anthropic.com/v1/messages");
            let g = Client::new(
                &cfg(
                    "https://generativelanguage.googleapis.com/v1beta".into(),
                    ProviderKind::Gemini,
                ),
                None,
            )
            .unwrap();
            assert_eq!(
                g.chat_url(),
                "https://generativelanguage.googleapis.com/v1beta/models/m:streamGenerateContent?alt=sse"
            );
        }
    }
}
