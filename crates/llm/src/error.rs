#[derive(Debug, thiserror::Error)]
pub enum LlmError {
    #[error("{0}")]
    Config(String),
    #[error("couldn't reach {url}: {detail}")]
    Connect { url: String, detail: String },
    #[error("the server rejected the API key (HTTP {status}): {message}")]
    Auth { status: u16, message: String },
    #[error("HTTP {status}: {message}")]
    Api { status: u16, message: String },
    #[error("the stream broke off: {0}")]
    Stream(String),
    #[error("unexpected response: {0}")]
    Protocol(String),
}

impl LlmError {
    pub(crate) fn from_reqwest(url: &str, e: reqwest::Error) -> Self {
        let mut detail = chummy_http::describe(&e);
        if detail.starts_with("no connection could be made") || detail.starts_with("its address") {
            detail.push_str(". Is the server running, and is the address right?");
        }
        LlmError::Connect {
            url: url.to_string(),
            detail,
        }
    }

    /// Builds an error from a non-success HTTP response body, pulling out the
    /// provider's message when it uses the usual `{"error": {"message": …}}` shape.
    pub(crate) fn from_status(status: u16, body: &str) -> Self {
        let message = serde_json::from_str::<serde_json::Value>(body)
            .ok()
            .and_then(|v| {
                v["error"]["message"]
                    .as_str()
                    .or_else(|| v["error"].as_str())
                    .or_else(|| v["message"].as_str())
                    .or_else(|| v["detail"].as_str())
                    .map(str::to_string)
            })
            .unwrap_or_else(|| {
                let t = body.trim();
                if t.is_empty() {
                    "no details".into()
                } else {
                    t.chars().take(300).collect()
                }
            });
        if status == 401 || status == 403 {
            LlmError::Auth { status, message }
        } else {
            LlmError::Api { status, message }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extracts_provider_messages() {
        let e = LlmError::from_status(401, r#"{"error":{"message":"bad key","type":"auth"}}"#);
        assert!(matches!(e, LlmError::Auth { status: 401, ref message } if message == "bad key"));
        let e = LlmError::from_status(500, "oops");
        assert!(matches!(e, LlmError::Api { ref message, .. } if message == "oops"));
    }
}
