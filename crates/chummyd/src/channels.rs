//! chummyd's side of its channels (`chummy_channel`'s Matrix and Telegram,
//! shared with the app): the bundle's settings, the sealed state for what
//! each keeps (Matrix's login and rooms, the people paired with a Telegram
//! bot), and the terminal, where the operator confirms Matrix emoji and
//! lets people in to Telegram bots.

#![cfg_attr(not(any(feature = "matrix", feature = "telegram")), allow(dead_code))]

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use chummy_channel::{ChannelHost, Event, FromChannel};
use tokio::sync::mpsc::UnboundedSender;

use crate::platform::DaemonPlatform;
use crate::router::{Input, note};
use crate::saver::{Kept, Saver};

/// The channel id of an agent's Telegram bot.
pub fn telegram_id(agent: &str) -> String {
    format!("telegram:{agent}")
}

struct DaemonChannel {
    p: DaemonPlatform,
    /// The channel's id.
    id: String,
    kept: Kept,
    saver: Arc<Saver>,
    inbox: UnboundedSender<Input>,
    /// The bot's name was shown.
    told: AtomicBool,
}

impl DaemonChannel {
    fn agent(&self) -> String {
        let id = self.id.strip_prefix("telegram:").unwrap_or("");
        self.p
            .0
            .bundle
            .agent(id)
            .map(|a| a.name.clone())
            .unwrap_or_else(|| id.to_string())
    }
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

    /// What waits is answered in the terminal.
    fn event(&self, e: Event) {
        match e {
            Event::Emoji { n, who, emoji } => {
                let line: Vec<String> = emoji.iter().map(|(s, d)| format!("{s} {d}")).collect();
                note(&format!(
                    "{who} wants to verify chummyd. Check that they see these emoji, in this order:\n  {}\nThen: /verify {n} yes if they match, /verify {n} no if they don't.",
                    line.join("   ")
                ));
            }
            Event::Pairing { n, who } => {
                let _ = self.inbox.send(Input::Pairing {
                    channel: self.id.clone(),
                    n,
                });
                note(&format!(
                    "{who} sent the pairing code for {}'s Telegram bot. Telegram can read what they say to it.\nThen: /pair {n} yes to let them talk to it, /pair {n} no if you don't know them.",
                    self.agent()
                ));
            }
            Event::Over { n } => {
                let _ = self.inbox.send(Input::Over(n));
            }
            Event::Note { text } => note(&text),
            Event::People { .. } => {}
            Event::Account { name } => {
                if !self.told.swap(true, Ordering::Relaxed) {
                    note(&format!(
                        "{}'s Telegram bot is {name}. /pair makes a code to let someone talk to it.",
                        self.agent()
                    ));
                }
            }
        }
    }

    fn save_upload(&self, agent: &str, name: &str, data: &[u8]) -> Result<String, String> {
        chummy_channel::save_to_folder(&self.p, agent, name, data)
    }
}

fn host(
    p: &DaemonPlatform,
    id: &str,
    kept: &Kept,
    saver: &Arc<Saver>,
    inbox: &UnboundedSender<Input>,
) -> Arc<DaemonChannel> {
    Arc::new(DaemonChannel {
        p: p.clone(),
        id: id.into(),
        kept: kept.clone(),
        saver: saver.clone(),
        inbox: inbox.clone(),
        told: AtomicBool::new(false),
    })
}

/// Starts the bundle's Matrix channel.
#[cfg(feature = "matrix")]
pub fn start_matrix(
    p: &DaemonPlatform,
    saver: &Arc<Saver>,
    kept: &Kept,
    store: std::path::PathBuf,
    state_key: zeroize::Zeroizing<[u8; 32]>,
    inbox: &UnboundedSender<Input>,
) -> chummy_channel::Handle {
    use chummy_channel::matrix::MatrixSettings;
    let bundle = p.0.bundle.clone();
    let m = bundle.channels.matrix.clone().expect("a Matrix channel");
    let settings = MatrixSettings {
        channel: crate::router::MATRIX.into(),
        homeserver: m.homeserver,
        user_id: m.user_id,
        password: zeroize::Zeroizing::new(m.password),
        users: m.users,
        upload_mb: m.upload_mb,
        label: bundle.instance.name.clone(),
        device: format!("chummyd ({})", bundle.instance.name),
        // One Matrix channel: each person's chat is theirs alone.
        chat_scope: String::new(),
        name: "chummyd".into(),
    };
    chummy_channel::matrix::start(
        host(p, crate::router::MATRIX, kept, saver, inbox),
        settings,
        store,
        chummy_channel::matrix::store_passphrase(&*state_key),
    )
}

/// Starts an agent's Telegram bot.
#[cfg(feature = "telegram")]
pub fn start_telegram(
    p: &DaemonPlatform,
    saver: &Arc<Saver>,
    kept: &Kept,
    bot: &chummy_bundle::TelegramBot,
    inbox: &UnboundedSender<Input>,
) -> Result<chummy_channel::Handle, String> {
    use chummy_channel::telegram::TelegramSettings;
    let id = telegram_id(&bot.agent);
    let settings = TelegramSettings {
        channel: id.clone(),
        api_url: bot.api_url.clone(),
        token: zeroize::Zeroizing::new(bot.token.clone()),
        agent: bot.agent.clone(),
        upload_mb: bot.upload_mb,
        stream: bot.stream,
        // Apart from Matrix's: each person has a chat per channel.
        chat_scope: "telegram".into(),
        name: "chummyd".into(),
    };
    chummy_channel::telegram::start(host(p, &id, kept, saver, inbox), settings)
}
