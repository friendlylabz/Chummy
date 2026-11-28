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
            "  {} [{}]: loop “{loop_name}”, model {model}; tools: {}; internet: {}",
            a.name,
            a.id,
            some(&tools, 4),
            yes(internet),
        ));
        if internet {
            look.push(format!("{} may use the internet", a.name));
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

    // Channels, each described by its kind (never with its secrets).
    let mut channels = vec![];
    if c.channels.terminal.enabled {
        channels.push(format!(
            "  terminal; approvals here: {}",
            yes(c.channels.terminal.approvals)
        ));
    }
    for ch in &c.channels.list {
        let agent = chummy_channel::kind::names(
            &ch.agents
                .iter()
                .map(|id| {
                    c.agents
                        .iter()
                        .find(|a| a.id == *id)
                        .map(|a| a.name.clone())
                        .unwrap_or_else(|| id.clone())
                })
                .collect::<Vec<_>>(),
        );
        let Some(k) = chummy_channel::kind(&ch.kind) else {
            channels.push(format!(
                "  {} [{}]: unknown kind {}",
                ch.name, ch.id, ch.kind
            ));
            continue;
        };
        let options = chummy_channel::kind::tidy(k, &ch.options);
        let secrets = chummy_channel::kind::secrets_of(&ch.secrets);
        let setup = chummy_channel::Setup {
            agent: &agent,
            options: &options,
            secrets: &secrets,
            program: "chummyd",
        };
        channels.push(format!(
            "  {} [{}], {} for {agent}: {}; approvals here: {}",
            ch.name,
            ch.id,
            k.label(),
            k.summary(&setup),
            yes(ch.approvals)
        ));
        look.extend(k.cautions(&setup));
        if ch.approvals {
            look.push(format!(
                "{agent}'s tool approvals can be given in “{}”",
                ch.name
            ));
        }
    }
    if channels.is_empty() {
        out.push("Channels: none".into());
    } else {
        out.push("Channels:".into());
        out.extend(channels);
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

        // Without the bundle's permission, the agent gets neither.
        c.permissions.tools = false;
        c.permissions.network = false;
        let text = describe(&c).join("\n");
        assert!(text.contains("tools: none; internet: no"), "{text}");
        assert!(!text.contains("may use the internet"), "{text}");

        // Channels: each described by its kind (a Telegram bot by its id,
        // never its token), with what's worth a look.
        let token = chummy_channel::telegram::fake_token("123456789");
        c.channels.list.push(chummy_bundle::BundleChannel {
            id: "tg".into(),
            name: "Phone".into(),
            kind: "telegram".into(),
            agents: vec!["helper".into()],
            approvals: true,
            options: Default::default(),
            secrets: [("token".to_string(), token.clone())].into(),
            reports: Default::default(),
        });
        let text = describe(&c).join("\n");
        assert!(
            text.contains("Phone [tg], Telegram for Helper: Telegram bot 123456789, not end-to-end encrypted; approvals here: yes"),
            "{text}"
        );
        assert!(
            text.contains("Telegram isn't end-to-end encrypted"),
            "{text}"
        );
        assert!(text.contains("approvals can be given in “Phone”"), "{text}");
        assert!(
            !text.contains(&token) && !text.contains("fakefake"),
            "{text}"
        );
    }
}
