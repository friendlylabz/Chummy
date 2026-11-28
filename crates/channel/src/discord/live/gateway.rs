//! Discord's Gateway: the WebSocket the bot opens to receive messages and
//! button presses. Identify (or resume a session that dropped), keep it up
//! with heartbeats, and hand the events on. Only direct messages are asked
//! for (no message-content permission needed: Discord gives bots what's
//! sent to them directly).

use std::sync::Arc;
use std::time::Duration;

use futures_util::{SinkExt, StreamExt};
use serde_json::{Value, json};
use tokio_tungstenite::tungstenite::Message as Ws;
use tokio_tungstenite::tungstenite::protocol::frame::coding::CloseCode;

use super::api::{Gateway, Interaction, Message, User};
use super::{Ctx, problem};

/// Direct messages (1 << 12): all the bot asks for.
pub const INTENTS: u64 = 1 << 12;

/// A session to resume after the connection drops.
pub struct Resume {
    url: String,
    session: String,
    seq: Option<u64>,
}

/// How a session ended.
pub struct End {
    pub why: String,
    /// It was in (the bot connected) before it ended.
    pub was_ready: bool,
}

fn end(why: impl Into<String>, was_ready: bool) -> End {
    End {
        why: why.into(),
        was_ready,
    }
}

fn jitter() -> f64 {
    let mut b = [0u8; 2];
    if getrandom::fill(&mut b).is_ok() {
        f64::from(u16::from_le_bytes(b)) / f64::from(u16::MAX)
    } else {
        0.5
    }
}

/// What a close code means, and whether the session can be resumed.
fn closed(code: u16) -> (String, bool, bool) {
    // (why, can resume, gives up: a problem for whoever runs it)
    match code {
        4004 => (
            "Discord doesn't accept the bot's token: reset it in the Developer Portal (Bot → Reset Token), and \
             paste it again."
                .into(),
            false,
            true,
        ),
        4013 | 4014 => (
            "Discord refused what the bot asks to receive (direct messages). Check the bot in the Developer \
             Portal."
                .into(),
            false,
            true,
        ),
        4010..=4012 => (format!("Discord closed the connection ({code})"), false, true),
        4007 | 4009 => ("the session ended".into(), false, false),
        _ => (format!("the connection closed ({code})"), true, false),
    }
}

/// One connection to the Gateway, until it ends.
pub async fn session(ctx: &Arc<Ctx>, resume: &mut Option<Resume>) -> End {
    let url = match resume {
        Some(r) => r.url.clone(),
        None => match ctx.api.get::<Gateway>("/gateway/bot").await {
            Ok(g) => g.url,
            Err(e) => return end(problem(&e), false),
        },
    };
    if !(url.starts_with("wss://") || url.starts_with("ws://")) {
        return end("Discord gave an odd Gateway address", false);
    }
    let address = format!("{}/?v=10&encoding=json", url.trim_end_matches('/'));
    let connector =
        chummy_http::tls_config().map(|c| tokio_tungstenite::Connector::Rustls(Arc::new(c)));
    let connect = tokio_tungstenite::connect_async_tls_with_config(address, None, false, connector);
    let mut ws = match tokio::time::timeout(Duration::from_secs(30), connect).await {
        Ok(Ok((ws, _))) => ws,
        Ok(Err(e)) => {
            return end(
                ctx.api
                    .scrub(&format!("couldn't connect to Discord's Gateway: {e}")),
                false,
            );
        }
        Err(_) => return end("Discord's Gateway didn't answer", false),
    };

    // Hello: how often to send heartbeats.
    let hello = loop {
        match tokio::time::timeout(Duration::from_secs(20), ws.next()).await {
            Ok(Some(Ok(Ws::Text(t)))) => {
                let v: Value = serde_json::from_str(&t).unwrap_or(Value::Null);
                if v["op"] == 10 {
                    break v;
                }
            }
            Ok(Some(Ok(_))) => continue,
            _ => return end("Discord's Gateway didn't say hello", false),
        }
    };
    let every = Duration::from_millis(
        hello["d"]["heartbeat_interval"]
            .as_u64()
            .unwrap_or(41_250)
            .clamp(1_000, 120_000),
    );

    let hello_back = match resume {
        Some(r) => json!({ "op": 6, "d": {
            "token": ctx.api.token(), "session_id": r.session, "seq": r.seq } }),
        None => json!({ "op": 2, "d": {
            "token": ctx.api.token(),
            "intents": INTENTS,
            "properties": { "os": std::env::consts::OS, "browser": "chummy", "device": "chummy" } } }),
    };
    if ws.send(Ws::text(hello_back.to_string())).await.is_err() {
        return end("couldn't talk to Discord's Gateway", false);
    }

    let mut was_ready = false;
    let mut acked = true;
    let mut seq: Option<u64> = resume.as_ref().and_then(|r| r.seq);
    let first = Duration::from_secs_f64(every.as_secs_f64() * jitter());
    let mut beat = tokio::time::interval_at(tokio::time::Instant::now() + first, every);
    loop {
        tokio::select! {
            _ = beat.tick() => {
                if !acked {
                    // No answer to the last heartbeat: the connection is dead.
                    return end("Discord stopped answering", was_ready);
                }
                acked = false;
                if ws.send(Ws::text(json!({ "op": 1, "d": seq }).to_string())).await.is_err() {
                    return end("the connection broke", was_ready);
                }
            }
            frame = ws.next() => {
                let text = match frame {
                    Some(Ok(Ws::Text(t))) => t,
                    Some(Ok(Ws::Close(c))) => {
                        let code = c.map(|c| u16::from(c.code)).unwrap_or(u16::from(CloseCode::Abnormal));
                        let (why, resumable, gives_up) = closed(code);
                        if !resumable {
                            *resume = None;
                        }
                        return end(why, was_ready && !gives_up);
                    }
                    Some(Ok(_)) => continue,
                    Some(Err(e)) => return end(ctx.api.scrub(&format!("the connection broke: {e}")), was_ready),
                    None => return end("the connection closed", was_ready),
                };
                let v: Value = match serde_json::from_str(&text) {
                    Ok(v) => v,
                    Err(_) => continue,
                };
                if let Some(s) = v["s"].as_u64() {
                    seq = Some(s);
                    if let Some(r) = resume.as_mut() {
                        r.seq = Some(s);
                    }
                }
                match v["op"].as_u64() {
                    Some(0) => {
                        let d = v["d"].clone();
                        match v["t"].as_str().unwrap_or("") {
                            "READY" => {
                                let Ok(me) = serde_json::from_value::<User>(d["user"].clone()) else {
                                    return end("Discord said something unexpected", false);
                                };
                                *resume = Some(Resume {
                                    url: d["resume_gateway_url"].as_str().unwrap_or(&url).to_string(),
                                    session: d["session_id"].as_str().unwrap_or("").to_string(),
                                    seq,
                                });
                                was_ready = true;
                                let servers = d["guilds"].as_array().map_or(0, Vec::len);
                                super::ready(ctx, me, servers).await;
                            }
                            "RESUMED" => {
                                was_ready = true;
                                ctx.host.status(crate::Status::Connected);
                                chummy_log::system::info("discord", "Back on Discord");
                            }
                            "MESSAGE_CREATE" => {
                                if let Ok(m) = serde_json::from_value::<Message>(d) {
                                    super::message(ctx, m).await;
                                }
                            }
                            "INTERACTION_CREATE" => {
                                if let Ok(i) = serde_json::from_value::<Interaction>(d) {
                                    tokio::spawn(super::interaction(ctx.clone(), i));
                                }
                            }
                            _ => {}
                        }
                    }
                    // Discord asks for a heartbeat now.
                    Some(1) => {
                        if ws.send(Ws::text(json!({ "op": 1, "d": seq }).to_string())).await.is_err() {
                            return end("the connection broke", was_ready);
                        }
                    }
                    // Reconnect (and resume).
                    Some(7) => return end("Discord asked to reconnect", was_ready),
                    // The session can't go on (or be resumed, if false).
                    Some(9) => {
                        if v["d"] != json!(true) {
                            *resume = None;
                        }
                        tokio::time::sleep(Duration::from_secs_f64(1.0 + 4.0 * jitter())).await;
                        return end("Discord ended the session", was_ready);
                    }
                    Some(11) => acked = true,
                    _ => {}
                }
            }
        }
    }
}
