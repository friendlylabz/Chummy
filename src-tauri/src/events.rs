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
/// A chat run will pause after its current block. Payload: `RunPausing`.
pub const RUN_PAUSING: &str = "run:pausing";
/// The paused chat runs changed. Payload: `Vec<paused::PausedInfo>`.
pub const PAUSED_CHANGED: &str = "paused:changed";
/// A question and answer added to a running chat run's response. Payload: `RunSaid`.
pub const RUN_SAID: &str = "run:said";
/// Payload: `ChatMessageEvent`.
pub const CHAT_MESSAGE: &str = "chat:message";
/// Payload: `ChatMessageDeleted`.
pub const CHAT_MESSAGE_DELETED: &str = "chat:message_deleted";
/// Payload: `ChatMessageRestored` (a deleted message put back by Undo).
pub const CHAT_MESSAGE_RESTORED: &str = "chat:message_restored";
/// Payload: `ChatsChanged` (an agent's chat list or open chat changed).
pub const CHATS_CHANGED: &str = "chats:changed";
/// Payload: `ToolsStatus`.
pub const TOOLS_CHANGED: &str = "tools:changed";
/// Payload: `commands::sandbox::SandboxStatus`.
pub const SANDBOX_CHANGED: &str = "sandbox:changed";
/// Payload: `ToolsProgress`.
pub const TOOLS_PROGRESS: &str = "tools:progress";
/// Payload: `chummy_scheduler::Overview`.
pub const JOBS_CHANGED: &str = "jobs:changed";
/// Payload: `Vec<chummy_scheduler::Pending>`.
pub const ASKS_CHANGED: &str = "asks:changed";
/// Payload: `chummy_log::system::SysEntry`.
pub const SYSTEM_LOG: &str = "log:system";
/// Sent to the main window only. Payload: `tray::Activity`.
pub const CLOSE_REQUESTED: &str = "app:close-requested";
