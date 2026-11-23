//! What the shared runner needs from the app: secrets from the keychain, the
//! tool sandbox, the built-in engine, and each run's permissions from the
//! settings.

use std::sync::Arc;

use chummy_llm::ProviderConfig;
use chummy_runner::{AgentGrant, AgentHost, AgentSetup, Permissions, Services, ToolGrant};
use chummy_sandbox::Jail;
use chummy_scheduler::Origin;
use chummy_tools::{SandboxBackend, ToolPaths};
use tauri::{AppHandle, Manager};

use crate::commands::local::start_engine_for;
use crate::settings::{Agent, Settings};
use crate::state::AppState;

pub struct AppServices {
    pub app: AppHandle,
}

impl Services for AppServices {
    fn secret(&self, key: &str) -> Option<String> {
        self.app.state::<AppState>().secrets.get(key).ok().flatten()
    }

    async fn sandbox(&self) -> Result<Arc<dyn SandboxBackend>, String> {
        self.app.state::<AppState>().sandbox().await
    }

    fn tool_paths(&self) -> ToolPaths {
        self.app.state::<AppState>().tool_paths.clone()
    }

    async fn local_engine(&self, p: &ProviderConfig) -> Result<(String, String), String> {
        let state = self.app.state::<AppState>();
        if chummy_local::runtime::status(&state.runtime).is_none() {
            return Err(
                "The local engine isn't set up yet. Set it up in Settings → Local engine.".into(),
            );
        }
        let ep = start_engine_for(&self.app, &state, p).await?;
        Ok((ep.base_url, ep.token))
    }
}

pub type AppHost = AgentHost<AppServices>;

/// A host for one run, with the permissions in `s` right now.
pub fn agent_host(
    app: &AppHandle,
    s: &Settings,
    agent: Option<&Agent>,
    model: Option<ProviderConfig>,
    jail: Option<Jail>,
    origin: Option<Origin>,
) -> AppHost {
    let state = app.state::<AppState>();
    let setup = AgentSetup {
        providers: s.providers.clone(),
        model,
        permissions: Permissions {
            tools: s.permissions.tools,
            network: s.permissions.network,
            confirm_online_tools: s.permissions.confirm_online_tools,
        },
        tool_grants: s
            .tools
            .iter()
            .map(|(name, p)| {
                (
                    name.clone(),
                    ToolGrant {
                        enabled: p.enabled,
                        network: p.network,
                        ask: p.ask,
                    },
                )
            })
            .collect(),
        agent: agent.map(|a| AgentGrant {
            tools: a.tools.clone(),
            network: a.network,
        }),
        installed: if s.permissions.tools {
            chummy_tools::installed(&state.tool_paths)
        } else {
            vec![]
        },
        sandbox: Some(state.sandbox_health(&s.sandbox)),
        jail,
        asks: origin.map(|o| (state.asks.clone(), o)),
        time_zone: iana_time_zone::get_timezone().ok(),
        usage: Some(state.usage.clone()),
    };
    AgentHost::new(Arc::new(AppServices { app: app.clone() }), setup)
}
