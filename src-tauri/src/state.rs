//! Shared app state. Rust is the single source of truth; windows read it
//! through commands and hear about changes through events.

use std::sync::Mutex;

use chummy_flow::LoopGraph;
use chummy_llm::ProviderConfig;
use chummy_store::Store;
use serde::Serialize;

#[derive(Default)]
pub struct AppState {
    pub store: Mutex<Store>,
    /// Configured model endpoints. Empty on first launch.
    pub providers: Mutex<Vec<ProviderConfig>>,
    /// Loop library. Empty on first launch: nothing is installed by default.
    pub loops: Mutex<Vec<LoopGraph>>,
}

/// What the main window needs to know before it unlocks chat.
#[derive(Debug, Clone, Copy, Serialize)]
pub struct SetupStatus {
    pub model_configured: bool,
    pub loop_configured: bool,
}

impl AppState {
    pub fn setup_status(&self) -> SetupStatus {
        SetupStatus {
            model_configured: !lock(&self.providers).is_empty(),
            loop_configured: !lock(&self.loops).is_empty(),
        }
    }
}

/// Locks a mutex, recovering the data if a previous holder panicked.
pub fn lock<T>(m: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|e| e.into_inner())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn first_launch_is_not_ready() {
        let status = AppState::default().setup_status();
        assert!(!status.model_configured);
        assert!(!status.loop_configured);
    }
}
