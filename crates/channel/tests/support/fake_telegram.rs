//! A stand-in for Telegram's Bot API, for tests: it serves the methods the
//! Telegram channel uses over plain HTTP on 127.0.0.1, keeps every call, and
//! lets a test play the people writing to the bot. Shared by
//! chummy-channel's and chummyd's tests.

#![allow(dead_code)]

use std::collections::{HashMap, VecDeque};
use std::sync::{Arc, LazyLock, Mutex};
use std::time::{Duration, Instant};

use serde_json::{Value, json};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::Notify;

pub const BOT_ID: i64 = 123456789;
/// A made-up token for the bot, made when first used (no token is written
/// out in the code).
pub static TOKEN: LazyLock<String> =
    LazyLock::new(|| chummy_channel::telegram::fake_token(&BOT_ID.to_string()));

#[derive(Default)]
struct St {
    queue: VecDeque<Value>,
    next_update: i64,
    next_message: i64,
    calls: Vec<(String, Value)>,
    files: HashMap<String, (String, Vec<u8>)>,
    webhook: String,
    conflict: bool,
    no_drafts: bool,
    reject_html: bool,
    can_join_groups: bool,
    username: String,
}

#[derive(Clone)]
pub struct Fake {
    pub url: String,
    st: Arc<Mutex<St>>,
    wake: Arc<Notify>,
}

fn lock(m: &Mutex<St>) -> std::sync::MutexGuard<'_, St> {
    m.lock().unwrap_or_else(|e| e.into_inner())
}

fn now() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_secs() as i64
}

pub fn user(id: i64, first: &str, username: &str) -> Value {
    json!({ "id": id, "is_bot": false, "first_name": first, "username": username })
}

impl Fake {
    pub async fn start() -> Fake {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let url = format!("http://{}", listener.local_addr().unwrap());
        let fake = Fake {
            url,
            st: Arc::new(Mutex::new(St {
                next_update: 1,
                next_message: 100,
                username: "helper_bot".into(),
                ..Default::default()
            })),
            wake: Arc::new(Notify::new()),
        };
        let f = fake.clone();
        tokio::spawn(async move {
            loop {
                let Ok((s, _)) = listener.accept().await else {
                    return;
                };
                let f = f.clone();
                tokio::spawn(async move { f.connection(s).await });
            }
        });
        fake
    }

    pub fn set_conflict(&self, on: bool) {
        lock(&self.st).conflict = on;
        self.wake.notify_waiters();
    }
    pub fn set_no_drafts(&self, on: bool) {
        lock(&self.st).no_drafts = on;
    }
    pub fn set_reject_html(&self, on: bool) {
        lock(&self.st).reject_html = on;
    }
    pub fn set_webhook(&self, url: &str) {
        lock(&self.st).webhook = url.into();
    }
    pub fn set_can_join_groups(&self, on: bool) {
        lock(&self.st).can_join_groups = on;
    }

    fn push(&self, mut update: Value) -> i64 {
        let mut st = lock(&self.st);
        update["update_id"] = json!(st.next_update);
        st.next_update += 1;
        st.next_message += 1;
        let id = st.next_message;
        if let Some(m) = update.get_mut("message") {
            m["message_id"] = json!(id);
        }
        st.queue.push_back(update);
        drop(st);
        self.wake.notify_waiters();
        id
    }

    fn private(from: &Value) -> Value {
        json!({ "id": from["id"], "type": "private", "first_name": from["first_name"] })
    }

    /// Someone writes to the bot; returns the message's id.
    pub fn say(&self, from: &Value, text: &str) -> i64 {
        self.say_at(from, text, now())
    }

    pub fn say_at(&self, from: &Value, text: &str, date: i64) -> i64 {
        self.push(json!({ "message": {
            "date": date, "chat": Self::private(from), "from": from, "text": text } }))
    }

    pub fn reply(&self, from: &Value, text: &str, to: i64) -> i64 {
        self.push(json!({ "message": {
            "date": now(), "chat": Self::private(from), "from": from, "text": text,
            "reply_to_message": { "message_id": to, "date": now(), "chat": Self::private(from) } } }))
    }

    pub fn in_group(&self, from: &Value, text: &str) -> i64 {
        self.push(json!({ "message": {
            "date": now(), "chat": { "id": -1001, "type": "supergroup", "title": "G" },
            "from": from, "text": text } }))
    }

    pub fn send_file(&self, from: &Value, name: &str, data: &[u8]) -> i64 {
        let file_id = format!("f{}", lock(&self.st).files.len() + 1);
        lock(&self.st).files.insert(
            file_id.clone(),
            (format!("documents/{file_id}.bin"), data.to_vec()),
        );
        self.push(json!({ "message": {
            "date": now(), "chat": Self::private(from), "from": from,
            "document": { "file_id": file_id, "file_name": name, "file_size": data.len() } } }))
    }

    pub fn press(&self, from: &Value, message_id: i64, data: &str) {
        self.push(json!({ "callback_query": {
            "id": format!("cb{message_id}"), "from": from, "data": data,
            "message": { "message_id": message_id, "chat": Self::private(from) } } }));
    }

    pub fn calls(&self, method: &str) -> Vec<Value> {
        lock(&self.st)
            .calls
            .iter()
            .filter(|(m, _)| m == method)
            .map(|(_, b)| b.clone())
            .collect()
    }

    /// Waits for a call to `method` whose body passes `test`.
    pub async fn wait_call(&self, method: &str, test: impl Fn(&Value) -> bool) -> Value {
        let until = Instant::now() + Duration::from_secs(20);
        loop {
            if let Some(b) = self.calls(method).into_iter().find(|b| test(b)) {
                return b;
            }
            assert!(
                Instant::now() < until,
                "no {method} call like that came; calls: {:#?}",
                lock(&self.st).calls
            );
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
    }

    /// The text of messages sent to a chat.
    pub fn sent_to(&self, chat: i64) -> Vec<String> {
        self.calls("sendMessage")
            .into_iter()
            .filter(|b| b["chat_id"] == json!(chat))
            .map(|b| b["text"].as_str().unwrap_or("").to_string())
            .collect()
    }

    fn answer(&self, method: &str, path: &str, body: &Value) -> (u16, Value) {
        let mut st = lock(&self.st);
        st.calls.push((method.to_string(), body.clone()));
        let ok = |r: Value| (200, json!({ "ok": true, "result": r }));
        let no = |code: u16, d: &str| {
            (
                code,
                json!({ "ok": false, "error_code": code, "description": d }),
            )
        };
        if let Some(tok) = path.strip_prefix("/bot").and_then(|p| p.split('/').next())
            && tok != TOKEN.as_str()
        {
            return no(401, "Unauthorized");
        }
        let html = body["parse_mode"] == json!("HTML");
        match method {
            "getMe" => ok(json!({
                "id": BOT_ID, "is_bot": true, "first_name": "Helper",
                "username": st.username, "can_join_groups": st.can_join_groups })),
            "getWebhookInfo" => ok(json!({ "url": st.webhook, "pending_update_count": 0 })),
            "setMyCommands" | "sendChatAction" | "answerCallbackQuery" | "leaveChat" => {
                ok(json!(true))
            }
            "sendMessage" | "editMessageText" if html && st.reject_html => {
                no(400, "Bad Request: can't parse entities: unsupported tag")
            }
            "sendMessage" => {
                st.next_message += 1;
                // Kept with the call, for tests to find the message.
                let id = st.next_message;
                if let Some((_, b)) = st.calls.last_mut() {
                    b["_message_id"] = json!(id);
                }
                ok(json!({ "message_id": st.next_message, "date": now(),
                    "chat": { "id": body["chat_id"], "type": "private" }, "text": body["text"] }))
            }
            "editMessageText" | "editMessageReplyMarkup" => ok(json!({
                "message_id": body["message_id"], "date": now(),
                "chat": { "id": body["chat_id"], "type": "private" } })),
            "sendMessageDraft" if st.no_drafts => no(404, "Not Found: method not found"),
            "sendMessageDraft" => ok(json!(true)),
            "getFile" => {
                let id = body["file_id"].as_str().unwrap_or("");
                match st.files.get(id) {
                    Some((p, d)) => {
                        ok(json!({ "file_id": id, "file_path": p, "file_size": d.len() }))
                    }
                    None => no(400, "Bad Request: invalid file_id"),
                }
            }
            _ => no(404, "Not Found: method not found"),
        }
    }

    /// getUpdates: what's queued from `offset` on (waiting a little when
    /// there's nothing).
    async fn updates(&self, body: &Value) -> (u16, Value) {
        let offset = body["offset"].as_i64().unwrap_or(0);
        let timeout = body["timeout"].as_u64().unwrap_or(0);
        lock(&self.st)
            .calls
            .push(("getUpdates".into(), body.clone()));
        if offset < 0 {
            let mut st = lock(&self.st);
            let last: Vec<Value> = st.queue.back().cloned().into_iter().collect();
            st.queue.clear();
            return (200, json!({ "ok": true, "result": last }));
        }
        let until = Instant::now() + Duration::from_millis((timeout * 1000).min(1500));
        loop {
            {
                let mut st = lock(&self.st);
                if st.conflict {
                    return (
                        409,
                        json!({ "ok": false, "error_code": 409,
                        "description": "Conflict: terminated by other getUpdates request" }),
                    );
                }
                st.queue
                    .retain(|u| u["update_id"].as_i64().unwrap_or(0) >= offset);
                if !st.queue.is_empty() || Instant::now() >= until {
                    let r: Vec<Value> = st.queue.iter().cloned().collect();
                    return (200, json!({ "ok": true, "result": r }));
                }
            }
            let _ = tokio::time::timeout(Duration::from_millis(100), self.wake.notified()).await;
        }
    }

    async fn connection(&self, mut s: TcpStream) {
        let mut buf = Vec::new();
        loop {
            // Head.
            let head_end = loop {
                if let Some(i) = buf.windows(4).position(|w| w == b"\r\n\r\n") {
                    break i;
                }
                let mut chunk = [0u8; 8192];
                match s.read(&mut chunk).await {
                    Ok(0) | Err(_) => return,
                    Ok(n) => buf.extend_from_slice(&chunk[..n]),
                }
            };
            let head = String::from_utf8_lossy(&buf[..head_end]).to_string();
            let len: usize = head
                .lines()
                .find_map(|l| {
                    let (k, v) = l.split_once(':')?;
                    k.eq_ignore_ascii_case("content-length")
                        .then(|| v.trim().parse().ok())?
                })
                .unwrap_or(0);
            while buf.len() < head_end + 4 + len {
                let mut chunk = [0u8; 8192];
                match s.read(&mut chunk).await {
                    Ok(0) | Err(_) => return,
                    Ok(n) => buf.extend_from_slice(&chunk[..n]),
                }
            }
            let body: Value =
                serde_json::from_slice(&buf[head_end + 4..head_end + 4 + len]).unwrap_or(json!({}));
            buf.drain(..head_end + 4 + len);
            let path = head
                .lines()
                .next()
                .and_then(|l| l.split_whitespace().nth(1))
                .unwrap_or("/")
                .to_string();
            let (status, bytes, kind) = if let Some(rest) = path.strip_prefix("/file/bot") {
                let (tok, file) = rest.split_once('/').unwrap_or(("", ""));
                let st = lock(&self.st);
                match st.files.values().find(|(p, _)| p == file) {
                    Some((_, d)) if tok == TOKEN.as_str() => {
                        (200, d.clone(), "application/octet-stream")
                    }
                    _ => (404, b"not found".to_vec(), "text/plain"),
                }
            } else {
                let method = path.rsplit('/').next().unwrap_or("").to_string();
                let (status, v) = if method == "getUpdates" {
                    let tok = path
                        .strip_prefix("/bot")
                        .and_then(|p| p.split('/').next())
                        .unwrap_or("");
                    if tok == TOKEN.as_str() {
                        self.updates(&body).await
                    } else {
                        (
                            401,
                            json!({ "ok": false, "error_code": 401, "description": "Unauthorized" }),
                        )
                    }
                } else {
                    self.answer(&method, &path, &body)
                };
                (status, v.to_string().into_bytes(), "application/json")
            };
            let reason = if status == 200 { "OK" } else { "Error" };
            let head = format!(
                "HTTP/1.1 {status} {reason}\r\nContent-Type: {kind}\r\nContent-Length: {}\r\n\r\n",
                bytes.len()
            );
            if s.write_all(head.as_bytes()).await.is_err() || s.write_all(&bytes).await.is_err() {
                return;
            }
        }
    }
}
