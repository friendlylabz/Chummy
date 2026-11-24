//! Finding a model's context length. OpenAI's own API doesn't report it, but
//! most other servers do, each in its own way: vLLM and SGLang put
//! `max_model_len` in `/v1/models`, OpenRouter `context_length`, LM Studio
//! `max_context_length` / `loaded_context_length` (in its `/api/v0` API),
//! llama.cpp `n_ctx` (in `/props`, and `meta.n_ctx_train` in `/v1/models`),
//! Ollama `num_ctx` or `<arch>.context_length` (in `/api/show`), Gemini
//! `inputTokenLimit`, Anthropic `max_input_tokens`.

use std::time::Duration;

use serde_json::{Value, json};

use crate::client::Client;
use crate::config::ProviderKind;

/// Fields that hold a context length, most specific first: what the server
/// is running with before what the model could take.
const FIELDS: &[&str] = &[
    "/loaded_context_length",
    "/max_model_len",
    "/n_ctx",
    "/meta/n_ctx",
    "/context_length",
    "/context_window",
    "/max_context_length",
    "/max_input_tokens",
    "/inputTokenLimit",
    "/top_provider/context_length",
    "/meta/n_ctx_train",
];

fn number(v: &Value) -> Option<u32> {
    let n = v.as_u64().or_else(|| v.as_f64().map(|f| f as u64))?;
    (n > 0).then(|| n.min(u64::from(u32::MAX)) as u32)
}

/// A context length in one model's description.
pub(crate) fn from_entry(e: &Value) -> Option<u32> {
    FIELDS.iter().find_map(|f| e.pointer(f).and_then(number))
}

/// The context length of `model` in a model list (`{"data": […]}`,
/// `{"models": […]}` or a bare list), if its entry says.
pub(crate) fn from_model_list(v: &Value, model: &str) -> Option<u32> {
    let list = v["data"]
        .as_array()
        .or_else(|| v["models"].as_array())
        .or_else(|| v.as_array())?;
    let bare = model.strip_prefix("models/").unwrap_or(model);
    list.iter()
        .find(|e| {
            [&e["id"], &e["name"], &e["model"]].iter().any(|x| {
                x.as_str()
                    .is_some_and(|s| s == model || s.strip_prefix("models/").unwrap_or(s) == bare)
            })
        })
        .and_then(from_entry)
}

/// Ollama's `/api/show`: the `num_ctx` it runs with, else the model's own.
pub(crate) fn from_ollama_show(v: &Value) -> Option<u32> {
    let set = v["parameters"].as_str().and_then(|p| {
        p.lines().find_map(|l| {
            let mut w = l.split_whitespace();
            (w.next() == Some("num_ctx")).then(|| w.next()?.parse::<u32>().ok())?
        })
    });
    set.filter(|n| *n > 0).or_else(|| {
        v["model_info"].as_object().and_then(|m| {
            m.iter()
                .find(|(k, _)| k.ends_with(".context_length"))
                .and_then(|(_, v)| number(v))
        })
    })
}

/// llama.cpp's `/props`.
pub(crate) fn from_props(v: &Value) -> Option<u32> {
    v.pointer("/default_generation_settings/n_ctx")
        .and_then(number)
        .or_else(|| v.get("n_ctx").and_then(number))
}

impl Client {
    async fn fetch(&self, url: &str, body: Option<Value>) -> Option<Value> {
        let req = match body {
            Some(b) => self.http().post(url).json(&b),
            None => self.http().get(url),
        };
        let resp = req
            .headers(self.auth_headers())
            .timeout(Duration::from_secs(8))
            .send()
            .await
            .ok()?;
        if !resp.status().is_success() {
            return None;
        }
        let text = crate::client::body_text(resp, crate::client::MAX_SMALL_BYTES)
            .await
            .ok()?;
        serde_json::from_str(&text).ok()
    }

    /// Asks the server how long this model's context is. `None`: it
    /// doesn't say (or couldn't be reached).
    pub async fn context_length(&self) -> Option<u32> {
        let model = self.model().to_string();
        let root = self.root_url("/v1").to_string();
        if self.kind() == ProviderKind::Ollama {
            let show = self
                .fetch(&format!("{root}/api/show"), Some(json!({ "model": model })))
                .await;
            if let Some(n) = show.as_ref().and_then(from_ollama_show) {
                return Some(n);
            }
        }
        if let Some(n) = self
            .fetch(&self.model_list_url(), None)
            .await
            .and_then(|v| from_model_list(&v, &model))
        {
            return Some(n);
        }
        if matches!(self.kind(), ProviderKind::Anthropic | ProviderKind::Gemini) {
            return None;
        }
        // llama.cpp, then LM Studio's own API.
        if let Some(n) = self
            .fetch(&format!("{root}/props"), None)
            .await
            .and_then(|v| from_props(&v))
        {
            return Some(n);
        }
        self.fetch(&format!("{root}/api/v0/models/{model}"), None)
            .await
            .and_then(|v| from_entry(&v))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_each_servers_way() {
        // vLLM / SGLang
        let v = json!({ "data": [{ "id": "a" }, { "id": "m", "max_model_len": 32768 }] });
        assert_eq!(from_model_list(&v, "m"), Some(32768));
        assert_eq!(from_model_list(&v, "a"), None);
        // OpenAI itself: nothing
        assert_eq!(
            from_model_list(
                &json!({ "data": [{ "id": "gpt", "object": "model" }] }),
                "gpt"
            ),
            None
        );
        // OpenRouter
        let v = json!({ "data": [{ "id": "m", "top_provider": { "context_length": 128000 } }] });
        assert_eq!(from_model_list(&v, "m"), Some(128000));
        // llama.cpp /v1/models, then /props (what it runs with wins)
        let v = json!({ "data": [{ "id": "m", "meta": { "n_ctx_train": 131072 } }] });
        assert_eq!(from_model_list(&v, "m"), Some(131072));
        assert_eq!(
            from_props(&json!({ "default_generation_settings": { "n_ctx": 8192 } })),
            Some(8192)
        );
        // LM Studio: the loaded context before the model's maximum
        let v = json!({ "id": "m", "max_context_length": 131072, "loaded_context_length": 4096 });
        assert_eq!(from_entry(&v), Some(4096));
        // Gemini names models "models/…"
        let v = json!({ "models": [{ "name": "models/gemini-x", "inputTokenLimit": 1048576 }] });
        assert_eq!(from_model_list(&v, "gemini-x"), Some(1048576));
        // Anthropic
        let v = json!({ "data": [{ "id": "claude-x", "max_input_tokens": 200000 }] });
        assert_eq!(from_model_list(&v, "claude-x"), Some(200000));
        // Ollama: num_ctx if set, else the model's own
        let v = json!({
            "parameters": "stop \"<|im_end|>\"\nnum_ctx 16384",
            "model_info": { "qwen3.context_length": 40960 }
        });
        assert_eq!(from_ollama_show(&v), Some(16384));
        let v = json!({ "model_info": { "qwen3.context_length": 40960 } });
        assert_eq!(from_ollama_show(&v), Some(40960));
        assert_eq!(from_entry(&json!({ "context_length": 0 })), None);
    }
}
