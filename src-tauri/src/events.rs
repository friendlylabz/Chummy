//! Event names broadcast from Rust to every window. Keep in sync with
//! `ui/src/lib/ipc.ts`.

/// Payload: `SettingsView`.
pub const SETTINGS_CHANGED: &str = "settings:changed";
/// Payload: `SetupStatus`.
pub const SETUP_CHANGED: &str = "setup:changed";
/// Payload: `DownloadEvent`.
pub const LOCAL_DOWNLOAD: &str = "local:download";
/// Payload: `RuntimeEvent`.
pub const LOCAL_RUNTIME: &str = "local:runtime";
/// Payload: `chummy_local::engine::EngineStatus`.
pub const LOCAL_ENGINE: &str = "local:engine";
