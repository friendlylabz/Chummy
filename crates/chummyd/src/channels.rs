//! chummyd's side of its channels: the same for every kind
//! (`chummy_channel::ChannelKind`, shared with the app). It starts the
//! bundle's channels, keeps what each keeps in the sealed state (a login,
//! rooms, the people let in), gives each a folder and key of its own, and
//! verifies in the terminal: what a channel asks ([`Event::Verify`]) is
//! printed there and answered with `/verify n yes|no`; invitations are made
//! with `/invite` (see the router).

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use chummy_bundle::BundleChannel;
use chummy_channel::{ChannelHost, Event, FromChannel, Handle, Start, Verify};
use tokio::sync::mpsc::UnboundedSender;

use crate::platform::DaemonPlatform;
use crate::router::{Input, note};
use crate::saver::{Kept, Saver};

struct DaemonChannel {
    p: DaemonPlatform,
    /// The channel's id and name.
    id: String,
    name: String,
    agents: Vec<String>,
    kept: Kept,
    saver: Arc<Saver>,
    inbox: UnboundedSender<Input>,
    /// Its account was shown.
    told: AtomicBool,
}

impl DaemonChannel {
    fn agent_label(&self) -> String {
        agent_names(&self.p, &self.agents)
    }
}

/// Agents' names, as a sentence reads them.
pub fn agent_names(p: &DaemonPlatform, ids: &[String]) -> String {
    chummy_channel::kind::names(
        &ids.iter()
            .map(|id| {
                p.0.bundle
                    .agent(id)
                    .map(|a| a.name.clone())
                    .unwrap_or_else(|| id.clone())
            })
            .collect::<Vec<_>>(),
    )
}

/// How chummyd's terminal shows what a channel asks.
pub fn verify_text(channel: &str, v: &Verify) -> String {
    let mut t = format!("[{channel}] {} {}", v.who, v.ask);
    if !v.items.is_empty() {
        let line: Vec<String> = v.items.iter().map(|(s, d)| format!("{s} {d}")).collect();
        t.push_str(&format!("\n  {}", line.join("   ")));
    }
    if let Some(c) = &v.confirm {
        t.push_str(&format!("\n{c}"));
    }
    t.push_str(&format!(
        "\nThen: /verify {n} yes ({}), or /verify {n} no ({}).",
        v.yes.to_lowercase(),
        v.no.to_lowercase(),
        n = v.n
    ));
    t
}

impl ChannelHost for DaemonChannel {
    fn agent_name(&self, id: &str) -> Option<String> {
        self.p.0.bundle.agent(id).map(|a| a.name.clone())
    }

    fn kept(&self) -> Option<serde_json::Value> {
        self.kept
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .get(&self.id)
            .cloned()
    }

    fn keep(&self, kept: serde_json::Value) {
        self.kept
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .insert(self.id.clone(), kept);
        self.saver.save_now();
    }

    fn received(&self, m: FromChannel) {
        let _ = self.inbox.send(Input::Channel(self.id.clone(), m));
    }

    /// Verifying, in the terminal.
    fn event(&self, e: Event) {
        match e {
            Event::Verify(v) => {
                let _ = self.inbox.send(Input::Pending {
                    channel: self.id.clone(),
                    n: v.n,
                });
                note(&verify_text(&self.name, &v));
            }
            Event::Over { n } => {
                let _ = self.inbox.send(Input::Over(n));
            }
            Event::Note { text } => note(&format!("[{}] {text}", self.name)),
            Event::People { .. } => {}
            Event::Account { name } => {
                if !self.told.swap(true, Ordering::Relaxed) {
                    note(&format!(
                        "[{}] People reach {} as {name}.",
                        self.name,
                        self.agent_label()
                    ));
                }
            }
        }
    }

    fn save_upload(&self, agent: &str, name: &str, data: &[u8]) -> Result<String, String> {
        chummy_channel::save_to_folder(&self.p, agent, name, data)
    }
}

/// Starts one of the bundle's channels, whatever its kind: with a folder
/// of its own in the state folder, and a key from the state key.
pub fn start(
    p: &DaemonPlatform,
    saver: &Arc<Saver>,
    kept: &Kept,
    state_dir: &std::path::Path,
    state_key: &[u8],
    c: &BundleChannel,
    inbox: &UnboundedSender<Input>,
) -> Result<Handle, String> {
    let kind = chummy_channel::kind(&c.kind)
        .ok_or_else(|| format!("“{}” is a kind of channel chummyd doesn't know", c.name))?;
    let folder = state_dir.join("channels").join(&c.id);
    chummy_channel::make_private_dir(&folder)?;
    let host = Arc::new(DaemonChannel {
        p: p.clone(),
        id: c.id.clone(),
        name: c.name.clone(),
        agents: c.agents.clone(),
        kept: kept.clone(),
        saver: saver.clone(),
        inbox: inbox.clone(),
        told: AtomicBool::new(false),
    });
    kind.start(
        host,
        Start {
            channel: c.id.clone(),
            agents: c.agents.clone(),
            options: chummy_channel::kind::tidy(kind, &c.options),
            secrets: chummy_channel::kind::secrets_of(&c.secrets),
            folder,
            key: chummy_channel::kind::derive_key(state_key, &c.id),
            program: "chummyd".into(),
            place: p.0.bundle.instance.name.clone(),
        },
    )
    .map_err(|e| format!("“{}”: {e}", c.name))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn what_channels_ask_reads_the_same_for_every_kind() {
        let v = Verify {
            n: 3,
            who: "@alice:hs (session X)".into(),
            ask: "wants to verify chummyd's bot. Check that they see these emoji, in this order:"
                .into(),
            items: vec![("🐶".into(), "Dog".into()), ("🔑".into(), "Key".into())],
            yes: "They match".into(),
            no: "They don't match".into(),
            confirm: None,
        };
        let t = verify_text("Phone", &v);
        assert!(
            t.starts_with("[Phone] @alice:hs (session X) wants to verify"),
            "{t}"
        );
        assert!(t.contains("🐶 Dog   🔑 Key"), "{t}");
        assert!(
            t.ends_with("/verify 3 yes (they match), or /verify 3 no (they don't match)."),
            "{t}"
        );
        let v = Verify {
            items: vec![],
            confirm: Some("They'll be able to talk to Helper.".into()),
            yes: "Let them in".into(),
            no: "Refuse".into(),
            ..v
        };
        let t = verify_text("Phone", &v);
        assert!(
            t.contains("\nThey'll be able to talk to Helper.\nThen: /verify 3 yes (let them in)"),
            "{t}"
        );
    }
}
