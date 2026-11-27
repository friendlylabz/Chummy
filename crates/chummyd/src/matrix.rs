//! chummyd's side of the Matrix channel (`chummy_channel::matrix`, shared
//! with the app): the bundle's settings, the sealed state for the login,
//! the state key for the stores, and the terminal for verifications.

use std::path::PathBuf;
use std::sync::Arc;

use chummy_channel::FromMatrix;
use chummy_channel::matrix::{MatrixHandle, MatrixHost, MatrixSettings, VerifyEvent};
use tokio::sync::mpsc::UnboundedSender;

use crate::platform::DaemonPlatform;
use crate::router::{Input, MATRIX, note};
use crate::saver::{MatrixKept, Saver};

struct DaemonMatrix {
    p: DaemonPlatform,
    kept: MatrixKept,
    saver: Arc<Saver>,
    inbox: UnboundedSender<Input>,
}

impl MatrixHost for DaemonMatrix {
    fn agent_name(&self, id: &str) -> Option<String> {
        self.p.0.bundle.agent(id).map(|a| a.name.clone())
    }

    fn kept(&self) -> Option<serde_json::Value> {
        self.kept.lock().unwrap_or_else(|e| e.into_inner()).clone()
    }

    fn keep(&self, kept: serde_json::Value) {
        *self.kept.lock().unwrap_or_else(|e| e.into_inner()) = Some(kept);
        self.saver.save_now();
    }

    fn received(&self, m: FromMatrix) {
        let _ = self.inbox.send(Input::Matrix(m));
    }

    /// Verifications are confirmed in the terminal.
    fn verification(&self, v: VerifyEvent) {
        match v {
            VerifyEvent::Waiting { n, who, emoji } => {
                let line: Vec<String> = emoji.iter().map(|(s, d)| format!("{s} {d}")).collect();
                note(&format!(
                    "{who} wants to verify chummyd. Check that they see these emoji, in this order:\n  {}\nThen: /verify {n} yes if they match, /verify {n} no if they don't.",
                    line.join("   ")
                ));
            }
            VerifyEvent::Over { .. } => {}
            VerifyEvent::Note { text } => note(&text),
        }
    }

    fn save_upload(&self, agent: &str, name: &str, data: &[u8]) -> Result<String, String> {
        chummy_channel::matrix::save_to_folder(&self.p, agent, name, data)
    }
}

/// Starts the bundle's Matrix channel.
pub fn start(
    p: DaemonPlatform,
    saver: Arc<Saver>,
    kept: MatrixKept,
    store: PathBuf,
    state_key: zeroize::Zeroizing<[u8; 32]>,
    inbox: UnboundedSender<Input>,
) -> MatrixHandle {
    let bundle = p.0.bundle.clone();
    let m = bundle.channels.matrix.clone().expect("a Matrix channel");
    let settings = MatrixSettings {
        channel: MATRIX.into(),
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
    let host = Arc::new(DaemonMatrix {
        p,
        kept,
        saver,
        inbox,
    });
    chummy_channel::matrix::start(
        host,
        settings,
        store,
        chummy_channel::matrix::store_passphrase(&*state_key),
    )
}
