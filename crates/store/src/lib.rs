//! # chummy-store
//!
//! Where Chummy keeps its own data.
//!
//! Everything lives in one per-user folder, the **Chummy folder** ([`ChummyDirs`]):
//!
//! | OS      | Location                                              |
//! |---------|-------------------------------------------------------|
//! | Linux   | `~/.local/share/com.friendlylabz.chummy`              |
//! | macOS   | `~/Library/Application Support/com.friendlylabz.chummy` |
//! | Windows | `%LOCALAPPDATA%\com.friendlylabz.chummy`              |
//!
//! The app shell resolves that path (through Tauri) and hands it to this crate.
//! The sandbox treats the whole folder as off-limits: no workspace may be inside
//! it or contain it.
//!
//! Users never edit files here. Settings are versioned JSON documents written
//! atomically with a backup ([`JsonDoc`]). API keys and tokens are not stored in
//! the folder at all; they go to the OS keychain ([`secrets`]).

pub mod paused;
pub mod secrets;

mod doc;
mod layout;

pub use doc::{JsonDoc, Loaded, Recovery, StoreError, write_atomic};
pub(crate) use layout::restrict_to_owner;
pub use layout::{ChummyDirs, is_locked, lock_to_owner};

/// `n` random bytes as hex, for ids (chats, messages, runs). If the system's
/// random source fails (practically never), falls back to the clock, the
/// process id and a counter, so ids still don't repeat. Not for secrets:
/// use `getrandom` directly and refuse on failure.
pub fn random_id(n: usize) -> String {
    use std::sync::atomic::{AtomicU64, Ordering};
    static SEQ: AtomicU64 = AtomicU64::new(0);
    let mut b = vec![0u8; n];
    if getrandom::fill(&mut b).is_err() {
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos() as u64)
            .unwrap_or(0);
        let mut x = nanos
            ^ ((std::process::id() as u64) << 32)
            ^ SEQ
                .fetch_add(1, Ordering::Relaxed)
                .wrapping_mul(0x9E37_79B9_7F4A_7C15);
        for byte in b.iter_mut() {
            // xorshift
            x ^= x << 13;
            x ^= x >> 7;
            x ^= x << 17;
            *byte = x as u8;
        }
    }
    b.iter().map(|x| format!("{x:02x}")).collect()
}
