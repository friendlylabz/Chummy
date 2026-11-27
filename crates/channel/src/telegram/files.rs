//! Files people send in Telegram: downloaded (Telegram's Bot API gives bots
//! files of up to 20 MB), then saved like every channel's
//! ([`crate::files`]).

use std::sync::Arc;

use serde_json::json;

use super::Ctx;
use super::api::{File, FileRef, Message};
use crate::files::{UPLOADS, clean_name};

/// The file in a message, and the name it gets.
fn file_of(m: &Message) -> Option<(FileRef, String)> {
    let stamp = jiff::Timestamp::from_second(m.date)
        .map(|t| t.strftime("%Y-%m-%d %H.%M.%S").to_string())
        .unwrap_or_else(|_| m.message_id.to_string());
    let named = |f: &FileRef, default: String| {
        let name = f
            .file_name
            .clone()
            .filter(|n| !n.trim().is_empty())
            .unwrap_or(default);
        (f.clone(), name)
    };
    if let Some(f) = &m.document {
        return Some(named(f, format!("file {stamp}")));
    }
    if let Some(p) = m.photo.as_ref().and_then(|p| p.last()) {
        return Some(named(p, format!("photo {stamp}.jpg")));
    }
    if let Some(f) = &m.video {
        return Some(named(f, format!("video {stamp}.mp4")));
    }
    if let Some(f) = &m.audio {
        return Some(named(f, format!("audio {stamp}")));
    }
    if let Some(f) = &m.voice {
        return Some(named(f, format!("voice {stamp}.ogg")));
    }
    None
}

/// Whether a message carries a file.
pub fn has_file(m: &Message) -> bool {
    file_of(m).is_some()
}

/// Saves a file someone sent to the agent's folder, and says how it went.
pub async fn save(ctx: Arc<Ctx>, m: Message) {
    let Some((f, name)) = file_of(&m) else {
        return;
    };
    let chat = m.chat.id;
    let name = clean_name(&name);
    let mb = ctx.cfg.upload_mb.min(super::MAX_UPLOAD_MB);
    let limit = u64::from(mb) * 1024 * 1024;
    let too_large = || format!("“{name}” wasn't saved: files can be {mb} MB at most.");
    if f.file_size.is_some_and(|s| s > limit) {
        ctx.notice(chat, too_large());
        return;
    }
    let file: Result<File, _> = ctx
        .api
        .send("getFile", &json!({ "file_id": f.file_id }))
        .await
        .and_then(|v| {
            serde_json::from_value(v).map_err(|_| super::api::ApiError::Net("not a file".into()))
        });
    let path = match file {
        Ok(File {
            file_size: Some(s), ..
        }) if s > limit => {
            ctx.notice(chat, too_large());
            return;
        }
        Ok(File {
            file_path: Some(p), ..
        }) => p,
        Ok(_) | Err(_) => {
            chummy_log::system::warn("telegram", "Couldn't get a file someone sent");
            ctx.notice(chat, format!("“{name}” couldn't be downloaded."));
            return;
        }
    };
    let data = match ctx.api.download(&path, limit).await {
        Ok(d) => d,
        Err(e) if e.code() == Some(413) => {
            ctx.notice(chat, too_large());
            return;
        }
        Err(e) => {
            chummy_log::system::warn("telegram", format!("Couldn't download a file: {e}"));
            ctx.notice(chat, format!("“{name}” couldn't be downloaded."));
            return;
        }
    };
    let host = ctx.host.clone();
    let agent = ctx.cfg.agent.clone();
    let saved = tokio::task::spawn_blocking(move || host.save_upload(&agent, &name, &data))
        .await
        .map_err(|e| e.to_string())
        .and_then(|r| r);
    match saved {
        Ok(n) => {
            chummy_log::system::info("telegram", "Saved a file to the agent's uploads");
            ctx.notice(chat, format!("Saved to {UPLOADS}/{n}."));
        }
        Err(e) => {
            chummy_log::system::warn("telegram", format!("Couldn't save a file: {e}"));
            ctx.notice(chat, "The file couldn't be saved.".to_string());
        }
    }
}
