//! Files people send in Matrix: decrypted, then saved like every
//! channel's ([`crate::files`]).

use matrix_sdk::Client;
use matrix_sdk::ruma::events::room::message::MessageType;

use super::Ctx;
use crate::files::{UPLOADS, clean_name};

/// Saves a file someone sent to `agent`'s folder, and says how it went.
pub async fn save(ctx: &Ctx, client: &Client, room: &str, agent: &str, msg: &MessageType) {
    let (name, size) = match msg {
        MessageType::File(c) => (
            c.filename.clone().unwrap_or_else(|| c.body.clone()),
            c.info.as_ref().and_then(|i| i.size),
        ),
        MessageType::Image(c) => (
            c.filename.clone().unwrap_or_else(|| c.body.clone()),
            c.info.as_ref().and_then(|i| i.size),
        ),
        MessageType::Video(c) => (
            c.filename.clone().unwrap_or_else(|| c.body.clone()),
            c.info.as_ref().and_then(|i| i.size),
        ),
        MessageType::Audio(c) => (
            c.filename.clone().unwrap_or_else(|| c.body.clone()),
            c.info.as_ref().and_then(|i| i.size),
        ),
        _ => return,
    };
    let name = clean_name(&name);
    let limit = u64::from(ctx.cfg.upload_mb) * 1024 * 1024;
    let too_large = || {
        format!(
            "“{name}” wasn't saved: files can be {} MB at most.",
            ctx.cfg.upload_mb
        )
    };
    if size.is_some_and(|s| u64::from(s) > limit) {
        ctx.notice(room, too_large());
        return;
    }
    let media = client.media();
    let data = match msg {
        MessageType::File(c) => media.get_file(c, false).await,
        MessageType::Image(c) => media.get_file(c, false).await,
        MessageType::Video(c) => media.get_file(c, false).await,
        MessageType::Audio(c) => media.get_file(c, false).await,
        _ => return,
    };
    let data = match data {
        Ok(Some(d)) => d,
        Ok(None) | Err(_) => {
            chummy_log::system::warn("matrix", "Couldn't download a file someone sent");
            ctx.notice(room, format!("“{name}” couldn't be downloaded."));
            return;
        }
    };
    if data.len() as u64 > limit {
        ctx.notice(room, too_large());
        return;
    }
    let host = ctx.host.clone();
    let agent_id = agent.to_string();
    let saved = tokio::task::spawn_blocking(move || host.save_upload(&agent_id, &name, &data))
        .await
        .map_err(|e| e.to_string())
        .and_then(|r| r);
    match saved {
        Ok(n) => {
            chummy_log::system::info("matrix", format!("Saved a file to {agent}'s uploads"));
            ctx.notice(room, format!("Saved to {UPLOADS}/{n}."));
        }
        Err(e) => {
            chummy_log::system::warn("matrix", format!("Couldn't save a file: {e}"));
            ctx.notice(room, "The file couldn't be saved.".to_string());
        }
    }
}
