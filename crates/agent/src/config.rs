//! The configuration an agent runs under: its model, and what it may do.
//!
//! The app keeps this in its settings and chummyd in its bundle. Both read
//! it through [`AgentConfig`], so the rules (which model an agent uses, and
//! how the switches combine into a run's permissions) are the same in both.

use chummy_llm::ProviderConfig;
use chummy_runner::{AgentGrant, AgentSetup, ToolGrant};

use crate::{Agent, Permissions};

pub trait AgentConfig {
    fn agents(&self) -> &[Agent];
    fn providers(&self) -> &[ProviderConfig];
    /// The model connection used unless an agent or block says otherwise.
    fn default_provider(&self) -> Option<&str>;
    fn permissions(&self) -> &Permissions;

    fn agent(&self, id: &str) -> Option<&Agent> {
        self.agents().iter().find(|a| a.id == id)
    }

    fn provider(&self, id: &str) -> Option<&ProviderConfig> {
        self.providers().iter().find(|p| p.id == id)
    }

    /// The default model: the chosen default if it still exists, else the first.
    fn default_model(&self) -> Option<&ProviderConfig> {
        self.default_provider()
            .and_then(|d| self.provider(d))
            .or_else(|| self.providers().first())
    }

    /// The model an agent uses: its own choice if that still exists, else the default.
    fn agent_model(&self, agent: &Agent) -> Option<&ProviderConfig> {
        agent
            .model
            .as_deref()
            .and_then(|m| self.provider(m))
            .or_else(|| self.default_model())
    }

    /// What a run of `agent` (or of no agent: an editor test run) may use,
    /// with `model` as its model. The agent's own model settings apply to
    /// every connection it may call. Only the configuration is filled in:
    /// the caller adds the run's surroundings (its folder, tools installed,
    /// the sandbox, who to ask, time zone, usage).
    fn setup(&self, agent: Option<&Agent>, model: Option<ProviderConfig>) -> AgentSetup {
        let (providers, model) = match agent.map(|a| &a.model_options) {
            Some(o) if !o.is_empty() => (
                self.providers().iter().map(|p| o.apply(p)).collect(),
                model.map(|m| o.apply(&m)),
            ),
            _ => (self.providers().to_vec(), model),
        };
        let p = self.permissions();
        AgentSetup {
            providers,
            model,
            permissions: chummy_runner::Permissions {
                tools: p.tools,
                network: p.network,
                confirm_online_tools: p.confirm_online_tools,
            },
            // The agent's own tools, with its switches for each.
            tool_grants: agent
                .map(|a| {
                    a.tools
                        .iter()
                        .map(|(name, t)| {
                            (
                                name.clone(),
                                ToolGrant {
                                    enabled: true,
                                    network: t.network,
                                    ask: t.ask,
                                },
                            )
                        })
                        .collect()
                })
                .unwrap_or_default(),
            agent: agent.map(|a| AgentGrant {
                tools: a.tools.keys().cloned().collect(),
            }),
            installed: vec![],
            sandbox: None,
            jail: None,
            asks: None,
            time_zone: None,
            usage: None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Workspace;

    #[derive(Default)]
    struct Config {
        agents: Vec<Agent>,
        providers: Vec<ProviderConfig>,
        default: Option<String>,
        permissions: Permissions,
    }

    impl AgentConfig for Config {
        fn agents(&self) -> &[Agent] {
            &self.agents
        }
        fn providers(&self) -> &[ProviderConfig] {
            &self.providers
        }
        fn default_provider(&self) -> Option<&str> {
            self.default.as_deref()
        }
        fn permissions(&self) -> &Permissions {
            &self.permissions
        }
    }

    fn provider(id: &str) -> ProviderConfig {
        serde_json::from_value(serde_json::json!({
            "id": id, "name": id, "kind": "open_ai_compatible",
            "base_url": "http://localhost:1/v1", "model": "m"
        }))
        .unwrap()
    }

    fn agent(model: Option<&str>) -> Agent {
        Agent {
            id: "a".into(),
            name: "A".into(),
            loop_id: "l".into(),
            model: model.map(Into::into),
            workspace: Workspace {
                path: "/tmp/a".into(),
                confirmed_at: 1,
            },
            created_at: 1,
            tools: [(
                "calculator".to_string(),
                crate::AgentTool {
                    ask: true,
                    network: false,
                },
            )]
            .into(),
            loop_config: Default::default(),
            model_options: Default::default(),
        }
    }

    #[test]
    fn an_agent_uses_its_model_else_the_default_else_the_first() {
        let mut c = Config {
            providers: vec![provider("x"), provider("y")],
            ..Default::default()
        };
        assert_eq!(c.agent_model(&agent(None)).unwrap().id, "x");
        c.default = Some("y".into());
        assert_eq!(c.agent_model(&agent(None)).unwrap().id, "y");
        assert_eq!(c.agent_model(&agent(Some("x"))).unwrap().id, "x");
        // A model that's gone falls back to the default.
        assert_eq!(c.agent_model(&agent(Some("gone"))).unwrap().id, "y");
        c.default = Some("gone".into());
        assert_eq!(c.default_model().unwrap().id, "x");
    }

    #[test]
    fn a_setup_carries_every_switch_and_nothing_else() {
        let mut c = Config {
            providers: vec![provider("x")],
            ..Default::default()
        };
        c.permissions.tools = true;
        let a = agent(None);
        let s = c.setup(Some(&a), c.agent_model(&a).cloned());
        assert!(s.permissions.tools && !s.permissions.network);
        assert!(s.permissions.confirm_online_tools, "on by default");
        assert_eq!(
            s.tool_grants["calculator"],
            ToolGrant {
                enabled: true,
                network: false,
                ask: true
            }
        );
        let grant = s.agent.unwrap();
        assert!(grant.tools.contains("calculator"));
        // The run's surroundings are the caller's to add.
        assert!(s.jail.is_none() && s.asks.is_none() && s.usage.is_none());
        assert!(s.installed.is_empty() && s.sandbox.is_none());
        // Without an agent (an editor test run), no tools at all.
        let none = c.setup(None, None);
        assert!(none.agent.is_none() && none.tool_grants.is_empty());
    }
}
