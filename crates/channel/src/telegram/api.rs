//! The few Bot API methods the channel uses, over HTTPS, with no Telegram
//! library. The bot's token is part of every address
//! (`/bot<token>/<method>`), so no address ever goes into an error or a
//! log: errors are described without them, and the token is blanked out of
//! anything a server says, just in case.

use std::time::Duration;

use serde::Deserialize;
use serde::de::DeserializeOwned;
use serde_json::Value;
use zeroize::Zeroizing;

/// Why a call failed.
#[derive(Debug, Clone, PartialEq)]
pub enum ApiError {
    /// Telegram couldn't be reached (or answered something that isn't the
    /// Bot API).
    Net(String),
    /// Telegram said no.
    Api {
        code: i64,
        description: String,
        /// Too many requests: wait this long (seconds).
        retry_after: Option<u64>,
    },
}

impl ApiError {
    pub fn code(&self) -> Option<i64> {
        match self {
            Self::Api { code, .. } => Some(*code),
            Self::Net(_) => None,
        }
    }

    /// The token is wrong (or the bot was deleted).
    pub fn unauthorized(&self) -> bool {
        matches!(self.code(), Some(401 | 404))
    }

    /// Something else reads the bot's messages.
    pub fn conflict(&self) -> bool {
        self.code() == Some(409)
    }

    fn description(&self) -> &str {
        match self {
            Self::Api { description, .. } => description,
            Self::Net(s) => s,
        }
    }

    /// The text couldn't be read as HTML.
    pub fn bad_markup(&self) -> bool {
        self.code() == Some(400) && self.description().contains("parse entities")
    }

    /// An edit that changes nothing.
    pub fn not_modified(&self) -> bool {
        self.description().contains("message is not modified")
    }
}

impl std::fmt::Display for ApiError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Net(s) => write!(f, "{s}"),
            Self::Api {
                code, description, ..
            } => write!(f, "Telegram said {code}: {description}"),
        }
    }
}

#[derive(Deserialize)]
struct Reply<T> {
    ok: bool,
    result: Option<T>,
    #[serde(default)]
    error_code: Option<i64>,
    #[serde(default)]
    description: Option<String>,
    #[serde(default)]
    parameters: Option<Parameters>,
}

#[derive(Deserialize)]
struct Parameters {
    #[serde(default)]
    retry_after: Option<u64>,
}

/// A bot's connection to the Bot API.
pub struct Api {
    http: reqwest::Client,
    base: String,
    token: Zeroizing<String>,
}

impl Api {
    /// `base`: the Bot API server (`https://api.telegram.org`, checked).
    pub fn new(base: &str, token: Zeroizing<String>) -> Result<Self, String> {
        let http = chummy_http::builder()
            .connect_timeout(Duration::from_secs(20))
            .build()
            .map_err(|e| chummy_http::describe(&e.without_url()))?;
        Ok(Self {
            http,
            base: base.trim_end_matches('/').to_string(),
            token,
        })
    }

    fn scrub(&self, s: &str) -> String {
        if self.token.is_empty() {
            s.to_string()
        } else {
            s.replace(self.token.as_str(), "<the bot token>")
        }
    }

    fn net(&self, e: reqwest::Error) -> ApiError {
        ApiError::Net(self.scrub(&chummy_http::describe(&e.without_url())))
    }

    /// Calls `method` with `body`; `wait`: how long the call may take.
    pub async fn call<T: DeserializeOwned>(
        &self,
        method: &str,
        body: &Value,
        wait: Duration,
    ) -> Result<T, ApiError> {
        let url = format!("{}/bot{}/{method}", self.base, self.token.as_str());
        let resp = self
            .http
            .post(url)
            .json(body)
            .timeout(wait)
            .send()
            .await
            .map_err(|e| self.net(e))?;
        let status = resp.status();
        let bytes = resp.bytes().await.map_err(|e| self.net(e))?;
        let reply: Reply<T> = serde_json::from_slice(&bytes).map_err(|_| {
            ApiError::Net(format!(
                "the Bot API server answered {status} with something that isn't the Bot API"
            ))
        })?;
        match (reply.ok, reply.result) {
            (true, Some(r)) => Ok(r),
            _ => Err(ApiError::Api {
                code: reply
                    .error_code
                    .unwrap_or_else(|| i64::from(status.as_u16())),
                description: self.scrub(&reply.description.unwrap_or_default()),
                retry_after: reply.parameters.and_then(|p| p.retry_after),
            }),
        }
    }

    /// Calls `method` and waits up to 30 s, keeping only whether it worked.
    pub async fn send(&self, method: &str, body: &Value) -> Result<Value, ApiError> {
        self.call(method, body, Duration::from_secs(30)).await
    }

    /// Downloads a file (its `file_path` from getFile), refusing more than
    /// `limit` bytes.
    pub async fn download(&self, path: &str, limit: u64) -> Result<Vec<u8>, ApiError> {
        let url = format!("{}/file/bot{}/{path}", self.base, self.token.as_str());
        let mut resp = self
            .http
            .get(url)
            .timeout(Duration::from_secs(300))
            .send()
            .await
            .map_err(|e| self.net(e))?;
        if !resp.status().is_success() {
            return Err(ApiError::Api {
                code: i64::from(resp.status().as_u16()),
                description: "the file couldn't be downloaded".into(),
                retry_after: None,
            });
        }
        let mut data = Vec::new();
        while let Some(chunk) = resp.chunk().await.map_err(|e| self.net(e))? {
            data.extend_from_slice(&chunk);
            if data.len() as u64 > limit {
                return Err(ApiError::Api {
                    code: 413,
                    description: "the file is too large".into(),
                    retry_after: None,
                });
            }
        }
        Ok(data)
    }
}

// What the channel reads from the Bot API. Fields it doesn't use are
// ignored, so new ones never break it.

#[derive(Debug, Clone, Deserialize)]
pub struct Me {
    pub id: i64,
    #[serde(default)]
    pub username: Option<String>,
    #[serde(default)]
    pub can_join_groups: Option<bool>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct WebhookInfo {
    #[serde(default)]
    pub url: String,
}

#[derive(Debug, Clone, Deserialize)]
pub struct Update {
    pub update_id: i64,
    #[serde(default)]
    pub message: Option<Message>,
    #[serde(default)]
    pub callback_query: Option<CallbackQuery>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct User {
    pub id: i64,
    #[serde(default)]
    pub is_bot: bool,
    #[serde(default)]
    pub first_name: String,
    #[serde(default)]
    pub last_name: Option<String>,
    #[serde(default)]
    pub username: Option<String>,
}

impl User {
    /// "Alice Smith (@alice)".
    pub fn name(&self) -> String {
        let mut n = self.first_name.trim().to_string();
        if let Some(l) = self
            .last_name
            .as_deref()
            .map(str::trim)
            .filter(|l| !l.is_empty())
        {
            if !n.is_empty() {
                n.push(' ');
            }
            n.push_str(l);
        }
        if n.is_empty() {
            n = self.id.to_string();
        }
        match self.username.as_deref().filter(|u| !u.is_empty()) {
            Some(u) => format!("{n} (@{u})"),
            None => n,
        }
    }
}

#[derive(Debug, Clone, Deserialize)]
pub struct Chat {
    pub id: i64,
    #[serde(rename = "type", default)]
    pub kind: String,
}

#[derive(Debug, Clone, Deserialize)]
pub struct Message {
    pub message_id: i64,
    #[serde(default)]
    pub date: i64,
    pub chat: Chat,
    #[serde(default)]
    pub from: Option<User>,
    #[serde(default)]
    pub text: Option<String>,
    #[serde(default)]
    pub reply_to_message: Option<Box<Message>>,
    #[serde(default)]
    pub document: Option<FileRef>,
    #[serde(default)]
    pub photo: Option<Vec<FileRef>>,
    #[serde(default)]
    pub video: Option<FileRef>,
    #[serde(default)]
    pub audio: Option<FileRef>,
    #[serde(default)]
    pub voice: Option<FileRef>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct FileRef {
    pub file_id: String,
    #[serde(default)]
    pub file_name: Option<String>,
    #[serde(default)]
    pub file_size: Option<u64>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct File {
    #[serde(default)]
    pub file_path: Option<String>,
    #[serde(default)]
    pub file_size: Option<u64>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct CallbackQuery {
    pub id: String,
    pub from: User,
    #[serde(default)]
    pub message: Option<ButtonMessage>,
    #[serde(default)]
    pub data: Option<String>,
}

/// The message a button was on (only what's needed of it).
#[derive(Debug, Clone, Deserialize)]
pub struct ButtonMessage {
    pub message_id: i64,
    pub chat: Chat,
}

#[derive(Debug, Clone, Deserialize)]
pub struct Sent {
    pub message_id: i64,
}
