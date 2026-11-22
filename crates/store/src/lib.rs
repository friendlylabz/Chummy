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

pub mod secrets;

mod doc;
mod layout;

pub use doc::{JsonDoc, Loaded, Recovery, StoreError, write_atomic};
pub use layout::ChummyDirs;
