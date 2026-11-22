//! Event names broadcast from Rust to every window. Keep in sync with
//! `ui/src/lib/ipc.ts`.

/// Payload: [`chummy_store::Settings`].
pub const SETTINGS_CHANGED: &str = "settings:changed";
/// Payload: [`crate::state::SetupStatus`]. Emitted once providers and loops
/// can be configured (pass 2+).
#[allow(dead_code)]
pub const SETUP_CHANGED: &str = "setup:changed";
