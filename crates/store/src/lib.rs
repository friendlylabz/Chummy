//! # chummy-store
//!
//! Internal persistence: settings, the loop library, sessions and jobs.
//!
//! Users never edit these files. Everything is configured in the app, and this
//! crate stores it in the app data directory. Secrets (API keys) are not stored
//! here; they go to the OS keychain.
//!
//! Status: pass 1 provides the [`Settings`] type and an in-memory store. Disk
//! persistence arrives in pass 2.

use serde::{Deserialize, Serialize};

/// UI theme preference.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Theme {
    /// Follow the operating system.
    #[default]
    System,
    Dark,
    Light,
}

/// App-wide settings.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct Settings {
    pub theme: Theme,
}

/// Holds app data in memory. Pass 2 backs this with the app data directory.
#[derive(Debug, Default)]
pub struct Store {
    pub settings: Settings,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn theme_defaults_to_system() {
        assert_eq!(Store::default().settings.theme, Theme::System);
        assert_eq!(serde_json::to_string(&Theme::Dark).unwrap(), "\"dark\"");
    }
}
