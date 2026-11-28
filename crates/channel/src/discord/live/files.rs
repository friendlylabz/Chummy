//! Files people send on Discord: downloaded from the address Discord gives
//! (no token goes with it), then saved like every channel's
//! ([`crate::files`]).

use std::sync::Arc;

use super::Ctx;
use super::api::Attachment;
use crate::files::{UPLOADS, clean_name};

/// Saves a file someone sent to the agent's folder, and says how it went.
pub async fn save(ctx: Arc<Ctx>, user: String, a: Attachment) {
    let name = clean_name(if a.filename.trim().is_empty() {
        "file"
    } else {
        &a.filename
    });
    let mb = ctx.cfg.upload_mb.clamp(1, super::super::MAX_UPLOAD_MB);
    let limit = u64::from(mb) * 1024 * 1024;
    let too_large = || format!("“{name}” wasn't saved: files can be {mb} MB at most.");
    if a.size > limit {
        ctx.notice(&user, too_large());
        return;
    }
    let data = match ctx.api.download(&a.url, limit).await {
        Ok(d) => d,
        Err(e) if e.status() == Some(413) => {
            ctx.notice(&user, too_large());
            return;
        }
        Err(e) => {
            chummy_log::system::warn("discord", format!("Couldn't download a file: {e}"));
            ctx.notice(&user, format!("“{name}” couldn't be downloaded."));
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
            chummy_log::system::info("discord", "Saved a file to the agent's uploads");
            ctx.notice(&user, format!("Saved to {UPLOADS}/{n}."));
        }
        Err(e) => {
            chummy_log::system::warn("discord", format!("Couldn't save a file: {e}"));
            ctx.notice(&user, "The file couldn't be saved.".to_string());
        }
    }
}
