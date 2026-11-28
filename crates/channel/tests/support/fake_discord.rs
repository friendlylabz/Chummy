//! A stand-in for Discord, for tests: the parts of its web API the Discord
//! channel uses (plain HTTP on 127.0.0.1) and a Gateway (a WebSocket that
//! says hello, takes identify and resume, answers heartbeats, and replays
//! what a resumed session missed). It keeps every call and lets a test play
//! the people writing to the bot. Shared by chummy-channel's and chummyd's
//! tests.

#![allow(dead_code)]

use std::collections::{BTreeMap, VecDeque};
use std::sync::{Arc, LazyLock, Mutex};
use std::time::{Duration, Instant};

use futures_util::{SinkExt, StreamExt};
use serde_json::{Value, json};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::mpsc::{UnboundedSender, unbounded_channel};
use tokio_tungstenite::tungstenite::Message as Ws;
use tokio_tungstenite::tungstenite::protocol::CloseFrame;
use tokio_tungstenite::tungstenite::protocol::frame::coding::CloseCode;

pub const BOT_ID: &str = "123456789012345678";
/// A made-up token for the bot, made when first used (no token is written
/// out in the code).
pub static TOKEN: LazyLock<String> = LazyLock::new(|| chummy_channel::discord::fake_token(BOT_ID));
pub const APP_ID: &str = "123456789012345678";

enum Out {
    Frame(Value),
    Close(u16),
}

#[derive(Default)]
struct St {
    calls: Vec<(String, String, Value)>,
    next_id: u64,
    seq: u64,
    /// Everything dispatched, for resumed sessions.
    log: Vec<(u64, Value)>,
    /// The connected Gateway session, if any.
    live: Option<UnboundedSender<Out>>,
    session: String,
    identifies: u32,
    resumes: u32,
    /// What people sent, by direct-message channel.
    history: BTreeMap<String, Vec<Value>>,
    /// What each message (the bot's) says now.
    contents: BTreeMap<String, String>,
    files: BTreeMap<String, Vec<u8>>,
    bot_public: bool,
    servers: usize,
    heartbeat_ms: u64,
}

#[derive(Clone)]
pub struct Fake {
    pub url: String,
    pub ws_url: String,
    st: Arc<Mutex<St>>,
}

fn lock(m: &Mutex<St>) -> std::sync::MutexGuard<'_, St> {
    m.lock().unwrap_or_else(|e| e.into_inner())
}

fn now_iso() -> String {
    jiff::Timestamp::now().to_string()
}

pub fn user(id: &str, name: &str) -> Value {
    json!({ "id": id, "username": name, "global_name": capital(name) })
}

fn capital(s: &str) -> String {
    let mut c = s.chars();
    c.next()
        .map(|f| f.to_uppercase().collect::<String>() + c.as_str())
        .unwrap_or_default()
}

/// The direct-message channel between the bot and someone.
pub fn dm(user_id: &str) -> String {
    format!("8{user_id}")
}

impl Fake {
    pub async fn start() -> Fake {
        let rest = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let gw = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let fake = Fake {
            url: format!("http://{}/api/v10", rest.local_addr().unwrap()),
            ws_url: format!("ws://{}", gw.local_addr().unwrap()),
            st: Arc::new(Mutex::new(St {
                next_id: 1000,
                servers: 1,
                heartbeat_ms: 45_000,
                session: "session-1".into(),
                ..Default::default()
            })),
        };
        let f = fake.clone();
        tokio::spawn(async move {
            while let Ok((s, _)) = rest.accept().await {
                let f = f.clone();
                tokio::spawn(async move { f.connection(s).await });
            }
        });
        let f = fake.clone();
        tokio::spawn(async move {
            while let Ok((s, _)) = gw.accept().await {
                let f = f.clone();
                tokio::spawn(async move { f.gateway(s).await });
            }
        });
        fake
    }

    pub fn set_bot_public(&self, on: bool) {
        lock(&self.st).bot_public = on;
    }
    pub fn set_servers(&self, n: usize) {
        lock(&self.st).servers = n;
    }
    pub fn set_heartbeat_ms(&self, ms: u64) {
        lock(&self.st).heartbeat_ms = ms;
    }
    pub fn identifies(&self) -> u32 {
        lock(&self.st).identifies
    }
    pub fn resumes(&self) -> u32 {
        lock(&self.st).resumes
    }
    pub fn connected(&self) -> bool {
        lock(&self.st).live.is_some()
    }

    /// Closes the Gateway connection with `code` (the session can be
    /// resumed unless the code says otherwise).
    pub fn drop_gateway(&self, code: u16) {
        if let Some(tx) = lock(&self.st).live.take() {
            let _ = tx.send(Out::Close(code));
        }
    }

    fn next_id(st: &mut St) -> String {
        st.next_id += 1;
        st.next_id.to_string()
    }

    /// Sends an event to the Gateway session (or keeps it for a resume).
    fn dispatch(&self, t: &str, d: Value) {
        let mut st = lock(&self.st);
        st.seq += 1;
        let frame = json!({ "op": 0, "t": t, "s": st.seq, "d": d });
        let seq = st.seq;
        st.log.push((seq, frame.clone()));
        if let Some(tx) = &st.live {
            let _ = tx.send(Out::Frame(frame));
        }
    }

    fn message(&self, from: &Value, extra: Value) -> String {
        let id = Self::next_id(&mut lock(&self.st));
        let uid = from["id"].as_str().unwrap().to_string();
        let mut m = json!({ "id": id, "channel_id": dm(&uid), "author": from,
            "content": "", "timestamp": now_iso(), "attachments": [] });
        if let (Some(m), Some(e)) = (m.as_object_mut(), extra.as_object()) {
            for (k, v) in e {
                m.insert(k.clone(), v.clone());
            }
        }
        lock(&self.st)
            .history
            .entry(dm(&uid))
            .or_default()
            .push(m.clone());
        self.dispatch("MESSAGE_CREATE", m);
        id
    }

    /// Someone writes to the bot; returns the message's id.
    pub fn say(&self, from: &Value, text: &str) -> String {
        self.message(from, json!({ "content": text }))
    }

    pub fn say_at(&self, from: &Value, text: &str, when: &str) -> String {
        self.message(from, json!({ "content": text, "timestamp": when }))
    }

    pub fn reply(&self, from: &Value, text: &str, to: &str) -> String {
        self.message(
            from,
            json!({ "content": text, "message_reference": { "message_id": to } }),
        )
    }

    pub fn in_server(&self, from: &Value, text: &str) -> String {
        self.message(
            from,
            json!({ "content": text, "guild_id": "777", "channel_id": "999" }),
        )
    }

    pub fn send_file(&self, from: &Value, name: &str, data: &[u8]) -> String {
        let base = self.url.trim_end_matches("/api/v10").to_string();
        lock(&self.st).files.insert(name.into(), data.to_vec());
        self.message(
            from,
            json!({ "attachments": [{ "id": "1", "filename": name, "size": data.len(),
                "url": format!("{base}/attachments/{name}") }] }),
        )
    }

    /// Presses a button on one of the bot's messages; returns the
    /// interaction's id.
    pub fn press(&self, from: &Value, message_id: &str, custom_id: &str) -> String {
        let id = Self::next_id(&mut lock(&self.st));
        let uid = from["id"].as_str().unwrap().to_string();
        let content = lock(&self.st)
            .contents
            .get(message_id)
            .cloned()
            .unwrap_or_default();
        self.dispatch(
            "INTERACTION_CREATE",
            json!({ "id": id, "token": format!("itok{id}"), "type": 3, "channel_id": dm(&uid),
                "user": from, "data": { "custom_id": custom_id, "component_type": 2 },
                "message": { "id": message_id, "content": content } }),
        );
        id
    }

    /// Uses a slash command; returns the interaction's id.
    pub fn command(&self, from: &Value, name: &str, options: Value) -> String {
        let id = Self::next_id(&mut lock(&self.st));
        let uid = from["id"].as_str().unwrap().to_string();
        self.dispatch(
            "INTERACTION_CREATE",
            json!({ "id": id, "token": format!("itok{id}"), "type": 2, "channel_id": dm(&uid),
                "user": from, "data": { "name": name, "options": options } }),
        );
        id
    }

    /// Calls made: `(method, path, body)`.
    pub fn calls(&self, method: &str, path_starts: &str) -> Vec<Value> {
        lock(&self.st)
            .calls
            .iter()
            .filter(|(m, p, _)| m == method && p.starts_with(path_starts))
            .map(|(_, p, b)| {
                let mut b = b.clone();
                if let Some(o) = b.as_object_mut() {
                    o.insert("_path".into(), json!(p));
                }
                b
            })
            .collect()
    }

    /// Waits for a call whose body passes `test`.
    pub async fn wait_call(
        &self,
        method: &str,
        path_starts: &str,
        test: impl Fn(&Value) -> bool,
    ) -> Value {
        let until = Instant::now() + Duration::from_secs(20);
        loop {
            if let Some(b) = self
                .calls(method, path_starts)
                .into_iter()
                .find(|b| test(b))
            {
                return b;
            }
            assert!(
                Instant::now() < until,
                "no {method} {path_starts} call like that came; calls: {:#?}",
                lock(&self.st).calls
            );
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
    }

    /// The text of messages the bot sent someone.
    pub fn sent_to(&self, user_id: &str) -> Vec<String> {
        self.calls(
            "POST",
            &format!("/api/v10/channels/{}/messages", dm(user_id)),
        )
        .into_iter()
        .map(|b| b["content"].as_str().unwrap_or("").to_string())
        .collect()
    }

    fn answer(
        &self,
        method: &str,
        path: &str,
        auth: &str,
        body: &Value,
    ) -> (u16, Vec<u8>, &'static str) {
        let mut st = lock(&self.st);
        st.calls.push((method.into(), path.into(), body.clone()));
        let json_ok = |v: Value| (200, v.to_string().into_bytes(), "application/json");
        let no = |code: u16, m: &str| {
            (
                code,
                json!({ "message": m, "code": 0 }).to_string().into_bytes(),
                "application/json",
            )
        };
        let (route, query) = path.split_once('?').unwrap_or((path, ""));
        if let Some(name) = route.strip_prefix("/attachments/") {
            return match st.files.get(name) {
                Some(d) => (200, d.clone(), "application/octet-stream"),
                None => no(404, "Unknown Attachment"),
            };
        }
        let Some(route) = route.strip_prefix("/api/v10") else {
            return no(404, "404: Not Found");
        };
        if route == "/gateway" {
            return json_ok(json!({ "url": self.ws_url }));
        }
        if !route.starts_with("/interactions/") && auth != format!("Bot {}", TOKEN.as_str()) {
            return no(401, "401: Unauthorized");
        }
        let parts: Vec<&str> = route.trim_start_matches('/').split('/').collect();
        match (method, parts.as_slice()) {
            ("GET", ["gateway", "bot"]) => json_ok(json!({ "url": self.ws_url, "shards": 1 })),
            ("GET", ["users", "@me"]) => {
                json_ok(json!({ "id": BOT_ID, "username": "helper_bot", "bot": true }))
            }
            ("GET", ["oauth2", "applications", "@me"]) => {
                json_ok(json!({ "id": APP_ID, "name": "Helper", "bot_public": st.bot_public }))
            }
            ("PUT", ["applications", _, "commands"]) => json_ok(body.clone()),
            ("POST", ["users", "@me", "channels"]) => {
                let r = body["recipient_id"].as_str().unwrap_or("");
                json_ok(json!({ "id": dm(r), "type": 1 }))
            }
            ("POST", ["channels", ch, "messages"]) => {
                let id = Self::next_id(&mut st);
                let content = body["content"].as_str().unwrap_or("").to_string();
                st.contents.insert(id.clone(), content.clone());
                if let Some((_, _, b)) = st.calls.last_mut() {
                    b["_message_id"] = json!(id);
                }
                json_ok(json!({ "id": id, "channel_id": ch, "content": content }))
            }
            ("PATCH", ["channels", ch, "messages", id]) => {
                if let Some(c) = body["content"].as_str() {
                    st.contents.insert(id.to_string(), c.to_string());
                }
                json_ok(json!({ "id": id, "channel_id": ch }))
            }
            ("POST", ["channels", _, "typing"]) => (204, vec![], "application/json"),
            ("GET", ["channels", ch, "messages"]) => {
                let after: u64 = query
                    .split('&')
                    .find_map(|kv| kv.strip_prefix("after="))
                    .and_then(|v| v.parse().ok())
                    .unwrap_or(0);
                let mut ms: Vec<Value> = st
                    .history
                    .get(*ch)
                    .map(|h| {
                        h.iter()
                            .filter(|m| {
                                m["id"]
                                    .as_str()
                                    .and_then(|i| i.parse::<u64>().ok())
                                    .unwrap_or(0)
                                    > after
                            })
                            .cloned()
                            .collect()
                    })
                    .unwrap_or_default();
                ms.reverse();
                json_ok(json!(ms))
            }
            ("POST", ["interactions", _, _, "callback"]) => (204, vec![], "application/json"),
            _ => no(404, "404: Not Found"),
        }
    }

    async fn connection(&self, mut s: TcpStream) {
        let mut buf = Vec::new();
        loop {
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
            let header = |name: &str| {
                head.lines()
                    .find_map(|l| {
                        let (k, v) = l.split_once(':')?;
                        k.eq_ignore_ascii_case(name).then(|| v.trim().to_string())
                    })
                    .unwrap_or_default()
            };
            let len: usize = header("content-length").parse().unwrap_or(0);
            let auth = header("authorization");
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
            let mut first = head.lines().next().unwrap_or("").split_whitespace();
            let method = first.next().unwrap_or("GET").to_string();
            let path = first.next().unwrap_or("/").to_string();
            let (status, bytes, kind) = self.answer(&method, &path, &auth, &body);
            let reason = if status < 300 { "OK" } else { "Error" };
            let head = format!(
                "HTTP/1.1 {status} {reason}\r\nContent-Type: {kind}\r\nContent-Length: {}\r\n\r\n",
                bytes.len()
            );
            if s.write_all(head.as_bytes()).await.is_err() || s.write_all(&bytes).await.is_err() {
                return;
            }
        }
    }

    fn ready(&self) -> Value {
        let mut st = lock(&self.st);
        st.seq += 1;
        let guilds: Vec<Value> = (0..st.servers)
            .map(|i| json!({ "id": format!("{}", 500 + i), "unavailable": true }))
            .collect();
        json!({ "op": 0, "t": "READY", "s": st.seq, "d": {
            "v": 10, "user": { "id": BOT_ID, "username": "helper_bot", "bot": true },
            "session_id": st.session, "resume_gateway_url": self.ws_url,
            "guilds": guilds, "application": { "id": APP_ID } } })
    }

    async fn gateway(&self, s: TcpStream) {
        let Ok(ws) = tokio_tungstenite::accept_async(s).await else {
            return;
        };
        let (mut sink, mut stream) = ws.split();
        let hb = lock(&self.st).heartbeat_ms;
        if sink
            .send(Ws::text(
                json!({ "op": 10, "d": { "heartbeat_interval": hb } }).to_string(),
            ))
            .await
            .is_err()
        {
            return;
        }
        let (tx, mut rx) = unbounded_channel::<Out>();
        let mut pending: VecDeque<Value> = VecDeque::new();
        // The first thing the bot says: identify or resume.
        let Some(Ok(Ws::Text(t))) = stream.next().await else {
            return;
        };
        let v: Value = serde_json::from_str(t.as_str()).unwrap_or(json!({}));
        match v["op"].as_u64() {
            Some(2) => {
                if v["d"]["token"] != json!(TOKEN.as_str()) {
                    let _ = sink
                        .send(Ws::Close(Some(CloseFrame {
                            code: CloseCode::from(4004),
                            reason: "Authentication failed.".into(),
                        })))
                        .await;
                    return;
                }
                assert_eq!(
                    v["d"]["intents"],
                    json!(1 << 12),
                    "only direct messages are asked for"
                );
                {
                    let mut st = lock(&self.st);
                    st.identifies += 1;
                    st.live = Some(tx.clone());
                }
                pending.push_back(self.ready());
            }
            Some(6) => {
                let resumed = {
                    let mut st = lock(&self.st);
                    if v["d"]["session_id"] != json!(st.session)
                        || v["d"]["token"] != json!(TOKEN.as_str())
                    {
                        false
                    } else {
                        st.resumes += 1;
                        let from = v["d"]["seq"].as_u64().unwrap_or(0);
                        for (_, f) in st.log.iter().filter(|(s, _)| *s > from) {
                            pending.push_back(f.clone());
                        }
                        st.seq += 1;
                        pending.push_back(json!({ "op": 0, "t": "RESUMED", "s": st.seq, "d": {} }));
                        st.live = Some(tx.clone());
                        true
                    }
                };
                if !resumed {
                    let _ = sink
                        .send(Ws::text(json!({ "op": 9, "d": false }).to_string()))
                        .await;
                    return;
                }
            }
            _ => return,
        }
        for f in pending {
            if sink.send(Ws::text(f.to_string())).await.is_err() {
                return;
            }
        }
        loop {
            tokio::select! {
                out = rx.recv() => match out {
                    Some(Out::Frame(f)) => {
                        if sink.send(Ws::text(f.to_string())).await.is_err() {
                            return;
                        }
                    }
                    Some(Out::Close(code)) => {
                        let _ = sink.send(Ws::Close(Some(CloseFrame {
                            code: CloseCode::from(code), reason: "closed by the test".into() }))).await;
                        return;
                    }
                    None => return,
                },
                frame = stream.next() => match frame {
                    Some(Ok(Ws::Text(t))) => {
                        let v: Value = serde_json::from_str(t.as_str()).unwrap_or(json!({}));
                        if v["op"] == 1 {
                            let _ = sink.send(Ws::text(json!({ "op": 11 }).to_string())).await;
                        }
                    }
                    Some(Ok(_)) => {}
                    _ => {
                        let mut st = lock(&self.st);
                        if st.live.as_ref().is_some_and(|l| l.same_channel(&tx)) {
                            st.live = None;
                        }
                        return;
                    }
                },
            }
        }
    }
}
