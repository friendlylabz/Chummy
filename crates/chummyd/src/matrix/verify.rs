//! Emoji verification. People allowed to talk to an agent verify chummyd's
//! session from their Matrix app; the operator confirms the emoji in
//! chummyd's terminal (`/verify yes` or `/verify no`), so verification
//! always rests on someone comparing both screens.

use futures_util::StreamExt;
use matrix_sdk::Client;
use matrix_sdk::encryption::verification::{
    SasState, SasVerification, VerificationRequest, VerificationRequestState,
};
use matrix_sdk::ruma::UserId;

use super::Ctx;
use crate::router::note;

/// Someone asked to verify (in a room or to this device).
pub async fn requested(ctx: std::sync::Arc<Ctx>, client: Client, user: &UserId, flow_id: &str) {
    if !ctx.allowed_user(user.as_str()) {
        chummy_log::system::info(
            "matrix",
            "Ignored a verification request from someone not allowed to talk to any agent",
        );
        return;
    }
    let Some(req) = client
        .encryption()
        .get_verification_request(user, flow_id)
        .await
    else {
        return;
    };
    if let Err(e) = req.accept().await {
        chummy_log::system::warn("matrix", format!("Couldn't accept a verification: {e}"));
        return;
    }
    chummy_log::system::info("matrix", format!("{user} started verifying chummyd"));
    tokio::spawn(follow_request(ctx, req));
}

async fn follow_request(ctx: std::sync::Arc<Ctx>, req: VerificationRequest) {
    let mut changes = req.changes();
    let mut state = Some(req.state());
    loop {
        match state.take() {
            Some(VerificationRequestState::Transitioned { verification }) => {
                match verification.sas() {
                    Some(sas) => follow_sas(ctx, sas).await,
                    None => {
                        let _ = req.cancel().await;
                        note("A verification was cancelled: chummyd verifies by emoji only.");
                    }
                }
                return;
            }
            Some(VerificationRequestState::Done | VerificationRequestState::Cancelled(_)) => {
                return;
            }
            _ => {}
        }
        match changes.next().await {
            Some(s) => state = Some(s),
            None => return,
        }
    }
}

async fn follow_sas(ctx: std::sync::Arc<Ctx>, sas: SasVerification) {
    if !sas.we_started()
        && let Err(e) = sas.accept().await
    {
        chummy_log::system::warn("matrix", format!("Couldn't accept a verification: {e}"));
        return;
    }
    let who = format!(
        "{} (session {})",
        sas.other_user_id(),
        sas.other_device().device_id()
    );
    let mut changes = sas.changes();
    let mut state = Some(sas.state());
    let mut shown: Option<u32> = None;
    loop {
        match state.take() {
            Some(SasState::KeysExchanged { .. }) if shown.is_none() => {
                let Some(emoji) = sas.emoji() else {
                    let _ = sas.cancel().await;
                    note(&format!(
                        "The verification with {who} was cancelled: chummyd verifies by emoji only."
                    ));
                    return;
                };
                let n = ctx.add_verification(sas.clone());
                shown = Some(n);
                let line: Vec<String> = emoji
                    .iter()
                    .map(|e| format!("{} {}", e.symbol, e.description))
                    .collect();
                note(&format!(
                    "{who} wants to verify chummyd. Check that they see these emoji, in this order:\n  {}\nThen: /verify {n} yes if they match, /verify {n} no if they don't.",
                    line.join("   ")
                ));
                chummy_log::system::info(
                    "matrix",
                    format!("Verification {n} with {who} waits for the operator"),
                );
            }
            Some(SasState::Done { .. }) => {
                if let Some(n) = shown {
                    ctx.remove_verification(n);
                }
                note(&format!("Verified {who}."));
                chummy_log::system::info("matrix", format!("Verified {who}"));
                return;
            }
            Some(SasState::Cancelled(c)) => {
                if let Some(n) = shown {
                    ctx.remove_verification(n);
                }
                note(&format!(
                    "The verification with {who} was cancelled ({}).",
                    c.reason()
                ));
                return;
            }
            _ => {}
        }
        match changes.next().await {
            Some(s) => state = Some(s),
            None => {
                if let Some(n) = shown {
                    ctx.remove_verification(n);
                }
                return;
            }
        }
    }
}

/// The operator's `/verify [n] yes|no`.
pub async fn answer(ctx: &Ctx, n: Option<u32>, matches: bool) {
    let sas = match ctx.take_verification(n) {
        Ok(s) => s,
        Err(e) => {
            note(&e);
            return;
        }
    };
    let r = if matches {
        sas.confirm().await
    } else {
        sas.mismatch().await
    };
    match r {
        Ok(()) if matches => note("Confirmed: waiting for them to confirm too."),
        Ok(()) => note("Cancelled: the emoji didn't match."),
        Err(e) => note(&format!("Couldn't answer the verification: {e}")),
    }
}
