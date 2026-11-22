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
/// Payload: `Vec<LoopSummary>`.
pub const LOOPS_CHANGED: &str = "loops:changed";
/// Payload: `RunInfo`.
pub const RUN_STARTED: &str = "run:started";
/// Payload: `RunEvent`.
pub const RUN_EVENT: &str = "run:event";
/// Payload: `RunFinished`.
pub const RUN_FINISHED: &str = "run:finished";
/// Payload: `ChatMessageEvent`.
pub const CHAT_MESSAGE: &str = "chat:message";
/// Payload: `ToolsStatus`.
pub const TOOLS_CHANGED: &str = "tools:changed";
/// Payload: `ToolsProgress`.
pub const TOOLS_PROGRESS: &str = "tools:progress";
