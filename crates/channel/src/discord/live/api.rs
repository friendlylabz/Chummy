//! The few parts of Discord's web API the channel uses, over HTTPS, with
//! no Discord library. The bot's token goes in a header, never in an
//! address; it's blanked out of anything a server says, just in case.

use std::time::Duration;

use reqwest::Method;
use serde::Deserialize;
use serde::de::DeserializeOwned;
use serde_json::Value;
use zeroize::Zeroizing;

/// Why a call failed.
#[derive(Debug, Clone, PartialEq)]
pub enum ApiError {
    /// Discord couldn't be reached (or answered something that isn't its
    /// API).
    Net(String),
    /// Discord said no.
    Api {
        status: u16,
        message: String,
        /// Too many requests: wait this long (seconds).
        retry_after: Option<f64>,
    },
}

impl ApiError {
    pub fn status(&self) -> Option<u16> {
        match self {
            Self::Api { status, .. } => Some(*status),
            Self::Net(_) => None,
        }
    }

    /// The token is wrong (or the bot was deleted).
    pub fn unauthorized(&self) -> bool {
        self.status() == Some(401)
    }
}

impl std::fmt::Display for ApiError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Net(s) => write!(f, "{s}"),
            Self::Api {
                status, message, ..
            } => write!(f, "Discord said {status}: {message}"),
        }
    }
}

/// A bot's connection to Discord's API.
pub struct Api {
    http: reqwest::Client,
    base: String,
    token: Zeroizing<String>,
}

impl Api {
    /// `base`: the API (`https://discord.com/api/v10`, checked).
    pub fn new(base: &str, token: Zeroizing<String>) -> Result<Self, String> {
        let http = chummy_http::builder()
            .connect_timeout(Duration::from_secs(20))
            .user_agent("DiscordBot (https://github.com/friendlylabz/Chummy, 1)")
            .build()
            .map_err(|e| chummy_http::describe(&e.without_url()))?;
        Ok(Self {
            http,
            base: base.trim_end_matches('/').to_string(),
            token,
        })
    }

    pub fn token(&self) -> &str {
        &self.token
    }

    pub fn scrub(&self, s: &str) -> String {
        if self.token.is_empty() {
            s.to_string()
        } else {
            s.replace(self.token.as_str(), "<the bot token>")
        }
    }

    fn net(&self, e: reqwest::Error) -> ApiError {
        ApiError::Net(self.scrub(&chummy_http::describe(&e.without_url())))
    }

    async fn once(
        &self,
        method: &Method,
        path: &str,
        body: Option<&Value>,
    ) -> Result<Value, ApiError> {
        let mut req = self
            .http
            .request(method.clone(), format!("{}{path}", self.base))
            .header("Authorization", format!("Bot {}", self.token.as_str()))
            .timeout(Duration::from_secs(30));
        if let Some(b) = body {
            req = req.json(b);
        }
        let resp = req.send().await.map_err(|e| self.net(e))?;
        let status = resp.status();
        let bytes = resp.bytes().await.map_err(|e| self.net(e))?;
        if status.as_u16() == 204 || (status.is_success() && bytes.is_empty()) {
            return Ok(Value::Null);
        }
        let v: Value = serde_json::from_slice(&bytes).map_err(|_| {
            ApiError::Net(format!(
                "Discord's API answered {status} with something that isn't its API"
            ))
        })?;
        if status.is_success() {
            return Ok(v);
        }
        Err(ApiError::Api {
            status: status.as_u16(),
            message: self.scrub(v["message"].as_str().unwrap_or("no reason given")),
            retry_after: v["retry_after"].as_f64(),
        })
    }

    /// Calls the API; waits and tries once more when it's asked to slow
    /// down.
    pub async fn call(
        &self,
        method: Method,
        path: &str,
        body: Option<&Value>,
    ) -> Result<Value, ApiError> {
        match self.once(&method, path, body).await {
            Err(ApiError::Api {
                status: 429,
                retry_after,
                ..
            }) => {
                let wait = retry_after.unwrap_or(1.0).clamp(0.1, 30.0);
                tokio::time::sleep(Duration::from_secs_f64(wait)).await;
                self.once(&method, path, body).await
            }
            r => r,
        }
    }

    pub async fn get<T: DeserializeOwned>(&self, path: &str) -> Result<T, ApiError> {
        let v = self.call(Method::GET, path, None).await?;
        serde_json::from_value(v)
            .map_err(|_| ApiError::Net("Discord answered something unexpected".into()))
    }

    pub async fn post(&self, path: &str, body: &Value) -> Result<Value, ApiError> {
        self.call(Method::POST, path, Some(body)).await
    }

    pub async fn patch(&self, path: &str, body: &Value) -> Result<Value, ApiError> {
        self.call(Method::PATCH, path, Some(body)).await
    }

    pub async fn put(&self, path: &str, body: &Value) -> Result<Value, ApiError> {
        self.call(Method::PUT, path, Some(body)).await
    }

    /// Downloads a file someone sent (its attachment address), refusing
    /// more than `limit` bytes. No token goes with it.
    pub async fn download(&self, url: &str, limit: u64) -> Result<Vec<u8>, ApiError> {
        if !(url.starts_with("https://") || url.starts_with("http://")) {
            return Err(ApiError::Net("not a web address".into()));
        }
        let mut resp = self
            .http
            .get(url)
            .timeout(Duration::from_secs(300))
            .send()
            .await
            .map_err(|e| self.net(e))?;
        if !resp.status().is_success() {
            return Err(ApiError::Api {
                status: resp.status().as_u16(),
                message: "the file couldn't be downloaded".into(),
                retry_after: None,
            });
        }
        let mut data = Vec::new();
        while let Some(chunk) = resp.chunk().await.map_err(|e| self.net(e))? {
            data.extend_from_slice(&chunk);
            if data.len() as u64 > limit {
                return Err(ApiError::Api {
                    status: 413,
                    message: "the file is too large".into(),
                    retry_after: None,
                });
            }
        }
        Ok(data)
    }
}

// What the channel reads from Discord. Fields it doesn't use are ignored,
// so new ones never break it.

#[derive(Debug, Clone, Deserialize)]
pub struct User {
    pub id: String,
    #[serde(default)]
    pub username: String,
    #[serde(default)]
    pub global_name: Option<String>,
    #[serde(default)]
    pub bot: Option<bool>,
}

impl User {
    /// "Alice (@alice)".
    pub fn name(&self) -> String {
        let shown = self
            .global_name
            .as_deref()
            .map(str::trim)
            .filter(|n| !n.is_empty());
        match (shown, self.username.trim()) {
            (Some(n), u) if !u.is_empty() && n != u => format!("{n} (@{u})"),
            (Some(n), _) => n.to_string(),
            (None, u) if !u.is_empty() => format!("@{u}"),
            _ => self.id.clone(),
        }
    }
}

#[derive(Debug, Clone, Deserialize)]
pub struct Application {
    pub id: String,
    #[serde(default)]
    pub bot_public: Option<bool>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct Gateway {
    pub url: String,
}

#[derive(Debug, Clone, Deserialize)]
pub struct Channel {
    pub id: String,
}

#[derive(Debug, Clone, Deserialize)]
pub struct Message {
    pub id: String,
    pub channel_id: String,
    #[serde(default)]
    pub guild_id: Option<String>,
    pub author: User,
    #[serde(default)]
    pub content: String,
    #[serde(default)]
    pub timestamp: String,
    #[serde(default)]
    pub attachments: Vec<Attachment>,
    #[serde(default)]
    pub message_reference: Option<Reference>,
}

impl Message {
    /// When it was sent, in seconds (0 if Discord didn't say).
    pub fn sent_at(&self) -> u64 {
        self.timestamp
            .parse::<jiff::Timestamp>()
            .map(|t| t.as_second().max(0) as u64)
            .unwrap_or(0)
    }
}

#[derive(Debug, Clone, Deserialize)]
pub struct Reference {
    #[serde(default)]
    pub message_id: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct Attachment {
    #[serde(default)]
    pub filename: String,
    #[serde(default)]
    pub size: u64,
    #[serde(default)]
    pub url: String,
}

/// A button pressed or a slash command used.
#[derive(Debug, Clone, Deserialize)]
pub struct Interaction {
    pub id: String,
    pub token: String,
    #[serde(rename = "type")]
    pub kind: u8,
    #[serde(default)]
    pub channel_id: Option<String>,
    #[serde(default)]
    pub guild_id: Option<String>,
    /// Who, in a direct message.
    #[serde(default)]
    pub user: Option<User>,
    #[serde(default)]
    pub data: Option<Value>,
    #[serde(default)]
    pub message: Option<ButtonMessage>,
}

/// The message a button was on (only what's needed of it).
#[derive(Debug, Clone, Deserialize)]
pub struct ButtonMessage {
    #[serde(default)]
    pub content: String,
}
