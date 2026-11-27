//! What a bundle sets up, in a few lines, for `chummyd check`: so whoever
//! runs it can see what it would do before it does anything. It's read from
//! the opened (and fully checked) configuration; nothing is run.
//!
//! Worth a look, and so listed apart under "Look at": internet access, tools
//! that go online without asking, approvals over channels, plain http to
//! somewhere that isn't local, and who may talk to the agents.

use chummy_bundle::Config;

fn yes(b: bool) -> &'static str {
    if b { "yes" } else { "no" }
}

/// Up to `n` names, then how many more.
fn some(names: &[String], n: usize) -> String {
    if names.is_empty() {
        return "none".into();
    }
    let mut s = names.iter().take(n).cloned().collect::<Vec<_>>().join(", ");
    if names.len() > n {
        s.push_str(&format!(" (+{})", names.len() - n));
    }
    s
}

/// The lines `chummyd check` prints.
pub fn describe(c: &Config) -> Vec<String> {
    let mut out = vec![];
    let mut look = vec![];
    out.push(format!(
        "Instance “{}” ({}), time zone {}",
        c.instance.name, c.instance.id, c.instance.time_zone
    ));

    // Where conversations go.
    out.push("Models (where conversations are sent):".into());
    for p in &c.providers {
        let url = if p.base_url.trim().is_empty() {
            p.kind
                .default_base_url()
                .unwrap_or("its usual address")
                .to_string()
        } else {
            p.base_url.clone()
        };
        let key = if c.secrets.contains_key(&format!("provider:{}", p.id)) {
            ", with a key"
        } else {
            ""
        };
        out.push(format!(
            "  {} [{}]: {} at {url}{key}",
            p.name, p.id, p.model
        ));
        if chummy_llm::unencrypted_remote(&url) {
            look.push(format!("“{}” uses plain http to {url}", p.name));
        }
    }
    if c.providers.is_empty() {
        out.push("  none".into());
    }

    let perm = &c.permissions;
    out.push(format!(
        "Allowed: tools {}, internet {}, online tools ask first {}",
        yes(perm.tools),
        yes(perm.network),
        yes(perm.confirm_online_tools)
    ));
    if perm.network && perm.tools && !perm.confirm_online_tools {
        look.push("tools may go online without asking".into());
    }

    out.push("Agents:".into());
    for a in &c.agents {
        let loop_name = c
            .loops
            .get(&a.loop_id)
            .map(|l| l.name.clone())
            .unwrap_or_else(|| a.loop_id.clone());
        let model = a
            .model
            .clone()
            .or_else(|| c.default_provider.clone())
            .unwrap_or_else(|| "the default".into());
        // Each tool with its switches: "fetch (asks, online)".
        let tools: Vec<String> = if perm.tools {
            a.tools
                .iter()
                .map(|(name, t)| {
                    let online = perm.network && t.network;
                    let asks = t.ask || (online && perm.confirm_online_tools);
                    match (asks, online) {
                        (true, true) => format!("{name} (asks, online)"),
                        (true, false) => format!("{name} (asks)"),
                        (false, true) => format!("{name} (online)"),
                        (false, false) => name.clone(),
                    }
                })
                .collect()
        } else {
            vec![]
        };
        let internet = perm.tools && perm.network && a.tools.values().any(|t| t.network);
        out.push(format!(
            "  {} [{}]: loop “{loop_name}”, model {model}; tools: {}; internet: {}; approvals over channels: {}",
            a.name,
            a.id,
            some(&tools, 4),
            yes(internet),
            yes(a.channel_approvals)
        ));
        if internet {
            look.push(format!("{} may use the internet", a.name));
        }
        if a.channel_approvals {
            look.push(format!(
                "{}'s tool approvals can be given over channels",
                a.name
            ));
        }
    }

    if !c.jobs.is_empty() {
        out.push("Jobs:".into());
        for j in &c.jobs {
            let when: Vec<String> = j.triggers.iter().map(|t| t.describe()).collect();
            let when = if when.is_empty() {
                "by hand".to_string()
            } else {
                some(&when, 2)
            };
            out.push(format!(
                "  {} [{}] for {}: {when}{}",
                j.name,
                j.id,
                j.agent,
                if j.enabled { "" } else { " (off)" }
            ));
        }
    }

    let mut channels = vec![];
    if c.channels.terminal.enabled {
        channels.push("terminal".to_string());
    }
    if let Some(m) = &c.channels.matrix {
        channels.push(format!("Matrix as {} on {}", m.user_id, m.homeserver));
    }
    for t in &c.channels.telegram {
        let agent = c
            .agents
            .iter()
            .find(|a| a.id == t.agent)
            .map(|a| a.name.as_str())
            .unwrap_or(&t.agent);
        let server = if t.api_url == chummy_bundle::TELEGRAM_API {
            String::new()
        } else {
            format!(", through {}", t.api_url)
        };
        channels.push(format!(
            "Telegram bot {} for {agent}{server}",
            chummy_bundle::telegram_bot_id(&t.token).unwrap_or("?")
        ));
    }
    out.push(format!(
        "Channels: {}",
        if channels.is_empty() {
            "none".into()
        } else {
            channels.join("; ")
        }
    ));
    if let Some(m) = &c.channels.matrix {
        for (agent, users) in &m.users {
            out.push(format!("  {agent} talks to: {}", some(users, 4)));
        }
        let people: std::collections::BTreeSet<&String> = m.users.values().flatten().collect();
        if !people.is_empty() {
            look.push(format!(
                "{} {} can talk to agents in Matrix",
                people.len(),
                if people.len() == 1 {
                    "person"
                } else {
                    "people"
                }
            ));
        }
    }
    if !c.channels.telegram.is_empty() {
        look.push(
            "Telegram isn't end-to-end encrypted: Telegram can read what people and agents say there (people are let in with /pair in the terminal)"
                .into(),
        );
    }
    out.push(format!(
        "Logs: message text {}",
        if c.logging.message_text {
            "included"
        } else {
            "left out"
        }
    ));

    if !look.is_empty() {
        out.push("Look at:".into());
        out.extend(look.into_iter().map(|l| format!("  - {l}")));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn what_a_bundle_allows_is_listed() {
        let mut c = Config::new_instance("Home", "UTC").unwrap();
        c.providers.push(
            serde_json::from_value(serde_json::json!({
                "id": "m", "name": "Far away", "kind": "open_ai_compatible",
                "base_url": "http://models.example.com/v1", "model": "big"
            }))
            .unwrap(),
        );
        c.agents.push(chummy_bundle::BundleAgent {
            id: "helper".into(),
            name: "Helper".into(),
            loop_id: "none".into(),
            model: Some("m".into()),
            tools: [
                (
                    "write_file".to_string(),
                    chummy_agent::AgentTool {
                        ask: true,
                        network: false,
                    },
                ),
                (
                    "fetch".to_string(),
                    chummy_agent::AgentTool {
                        ask: false,
                        network: true,
                    },
                ),
            ]
            .into(),
            loop_config: Default::default(),
            model_options: Default::default(),
            channel_approvals: true,
        });
        c.permissions.tools = true;
        c.permissions.network = true;
        c.permissions.confirm_online_tools = false;
        let text = describe(&c).join("\n");
        assert!(
            text.contains("Far away [m]: big at http://models.example.com/v1"),
            "{text}"
        );
        assert!(
            text.contains("tools: fetch (online), write_file (asks); internet: yes"),
            "{text}"
        );
        assert!(text.contains("Look at:"), "{text}");
        assert!(text.contains("plain http"), "{text}");
        assert!(text.contains("Helper may use the internet"), "{text}");
        assert!(
            text.contains("approvals can be given over channels"),
            "{text}"
        );

        // Without the bundle's permission, the agent gets neither.
        c.permissions.tools = false;
        c.permissions.network = false;
        let text = describe(&c).join("\n");
        assert!(text.contains("tools: none; internet: no"), "{text}");
        assert!(!text.contains("may use the internet"), "{text}");

        // A Telegram bot: its id (never its token), and a word on privacy.
        let token = "123456789:AAHdqTcvCH1vGWJxfSeofSAs0K5PALDsaw";
        c.channels.telegram.push(chummy_bundle::TelegramBot {
            agent: "helper".into(),
            token: token.into(),
            api_url: chummy_bundle::TELEGRAM_API.into(),
            upload_mb: 20,
            stream: true,
        });
        let text = describe(&c).join("\n");
        assert!(text.contains("Telegram bot 123456789 for Helper"), "{text}");
        assert!(
            text.contains("Telegram isn't end-to-end encrypted"),
            "{text}"
        );
        assert!(!text.contains("AAHdq"), "{text}");
    }
}
