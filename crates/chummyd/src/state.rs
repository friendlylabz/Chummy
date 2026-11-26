//! The state folder: everything chummyd keeps between runs, sealed with the
//! bundle's state key and bound to its instance.
//!
//! ```text
//! <state>/
//!   lock            held while chummyd runs (one chummyd per state folder)
//!   state.sealed    "CHUMMYD-STATE\x01", a 24-byte nonce, then
//!                   XChaCha20-Poly1305 of the saved state's JSON
//!   tools/          the tools' Python and built-in tools (not secret)
//!   matrix/         the Matrix channel's SQLite stores (its end-to-end
//!                   keys), encrypted with a passphrase from the state key
//! ```
//!
//! The associated data is the magic and the instance id, so a state file
//! from another instance (or another key) doesn't open. A state file that
//! doesn't open stops chummyd: it never starts over by itself (that would
//! quietly lose paused runs and, later, the Matrix keys). Moving the folder
//! aside starts afresh.

use std::collections::BTreeMap;
use std::fs::File;
use std::path::{Path, PathBuf};

use chacha20poly1305::aead::{Aead, KeyInit, Payload};
use chacha20poly1305::{XChaCha20Poly1305, XNonce};
use serde::{Deserialize, Serialize};
use zeroize::Zeroizing;

const MAGIC: &[u8; 14] = b"CHUMMYD-STATE\x01";
pub const STATE_FORMAT: &str = "chummyd.state";
pub const STATE_VERSION: u32 = 1;
const MAX_STATE_BYTES: u64 = 512 * 1024 * 1024;

/// What's saved.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Saved {
    pub format: String,
    pub version: u32,
    pub instance_id: String,
    /// Unix seconds.
    pub saved_at: u64,
    /// Conversations (`MemoryChats::snapshot`).
    #[serde(default)]
    pub chats: serde_json::Value,
    /// Paused runs, by `agent/chat` (base64 of what the session kept).
    #[serde(default)]
    pub paused: BTreeMap<String, String>,
    /// Job state and history (`JobStore::snapshot`).
    #[serde(default)]
    pub jobs: serde_json::Value,
    /// The day's model use (`UsageLedger::snapshot`).
    #[serde(default)]
    pub usage: serde_json::Value,
    /// The Matrix channel's login (access token, device) and rooms.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub matrix: Option<serde_json::Value>,
}

pub struct StateDir {
    pub dir: PathBuf,
    file: PathBuf,
    key: Zeroizing<[u8; 32]>,
    aad: Vec<u8>,
    instance_id: String,
    _lock: File,
}

impl StateDir {
    /// Opens (and locks) the state folder. `None`: nothing saved yet.
    pub fn open(
        dir: &Path,
        key: Zeroizing<[u8; 32]>,
        instance_id: &str,
    ) -> Result<(Self, Option<Saved>), String> {
        crate::checks::make_private_dir(dir)?;
        let lock = std::fs::OpenOptions::new()
            .create(true)
            .truncate(false)
            .write(true)
            .open(dir.join("lock"))
            .map_err(|e| format!("couldn't open the state folder's lock: {e}"))?;
        if let Err(e) = lock.try_lock() {
            return Err(match e {
                std::fs::TryLockError::WouldBlock => format!(
                    "Another chummyd is using the state folder {}.",
                    dir.display()
                ),
                std::fs::TryLockError::Error(e) => format!("couldn't lock the state folder: {e}"),
            });
        }
        let mut aad = MAGIC.to_vec();
        aad.extend_from_slice(instance_id.as_bytes());
        let me = Self {
            dir: dir.to_path_buf(),
            file: dir.join("state.sealed"),
            key,
            aad,
            instance_id: instance_id.into(),
            _lock: lock,
        };
        let saved = me.read()?;
        Ok((me, saved))
    }

    fn cipher(&self) -> XChaCha20Poly1305 {
        XChaCha20Poly1305::new_from_slice(self.key.as_ref()).expect("a 32-byte key")
    }

    fn read(&self) -> Result<Option<Saved>, String> {
        let unreadable = |why: &str| {
            format!(
                "The state folder {} can't be opened ({why}). It may belong to another instance, have been changed, or come from a bundle with another state key. chummyd won't start with it: move it aside to start afresh.",
                self.dir.display()
            )
        };
        let meta = match std::fs::symlink_metadata(&self.file) {
            Ok(m) => m,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Err(e) => return Err(unreadable(&e.to_string())),
        };
        if !meta.is_file() || meta.len() > MAX_STATE_BYTES {
            return Err(unreadable("it isn't a state file"));
        }
        let bytes = std::fs::read(&self.file).map_err(|e| unreadable(&e.to_string()))?;
        if bytes.len() < MAGIC.len() + 24 + 16 || &bytes[..MAGIC.len()] != MAGIC {
            return Err(unreadable("it isn't a state file"));
        }
        let (nonce, ct) = bytes[MAGIC.len()..].split_at(24);
        let plain = Zeroizing::new(
            self.cipher()
                .decrypt(
                    <&XNonce>::try_from(nonce).expect("24 bytes"),
                    Payload {
                        msg: ct,
                        aad: &self.aad,
                    },
                )
                .map_err(|_| unreadable("it doesn't match this bundle"))?,
        );
        let saved: Saved =
            serde_json::from_slice(&plain).map_err(|_| unreadable("its contents aren't valid"))?;
        if saved.format != STATE_FORMAT
            || saved.version != STATE_VERSION
            || saved.instance_id != self.instance_id
        {
            return Err(unreadable("it's from another version or instance"));
        }
        Ok(Some(saved))
    }

    /// Seals and saves `saved`: a flushed temp file renamed over the old.
    pub fn save(&self, saved: &Saved) -> Result<(), String> {
        let mut nonce = [0u8; 24];
        getrandom::fill(&mut nonce).map_err(|e| format!("no randomness: {e}"))?;
        let plain = Zeroizing::new(serde_json::to_vec(saved).map_err(|e| e.to_string())?);
        let ct = self
            .cipher()
            .encrypt(
                <&XNonce>::try_from(&nonce[..]).expect("24 bytes"),
                Payload {
                    msg: &plain,
                    aad: &self.aad,
                },
            )
            .map_err(|_| "encryption failed".to_string())?;
        let mut out = Vec::with_capacity(MAGIC.len() + 24 + ct.len());
        out.extend_from_slice(MAGIC);
        out.extend_from_slice(&nonce);
        out.extend_from_slice(&ct);
        chummy_store_write(&self.file, &out)
    }

    /// A fresh [`Saved`] for this instance.
    pub fn blank(&self) -> Saved {
        Saved {
            format: STATE_FORMAT.into(),
            version: STATE_VERSION,
            instance_id: self.instance_id.clone(),
            ..Default::default()
        }
    }
}

fn chummy_store_write(path: &Path, bytes: &[u8]) -> Result<(), String> {
    use std::io::Write;
    let tmp = path.with_extension("sealed.tmp");
    {
        let mut f = std::fs::OpenOptions::new()
            .create(true)
            .truncate(true)
            .write(true)
            .open(&tmp)
            .map_err(|e| format!("couldn't save the state: {e}"))?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let _ = f.set_permissions(std::fs::Permissions::from_mode(0o600));
        }
        f.write_all(bytes)
            .and_then(|_| f.sync_all())
            .map_err(|e| format!("couldn't save the state: {e}"))?;
    }
    std::fs::rename(&tmp, path).map_err(|e| format!("couldn't save the state: {e}"))?;
    #[cfg(unix)]
    if let Some(parent) = path.parent()
        && let Ok(d) = File::open(parent)
    {
        let _ = d.sync_all();
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn key(n: u8) -> Zeroizing<[u8; 32]> {
        Zeroizing::new([n; 32])
    }

    #[test]
    fn state_opens_only_for_its_instance_and_key() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path().join("s");
        let (st, saved) = StateDir::open(&dir, key(1), "i-1").unwrap();
        assert!(saved.is_none(), "nothing yet");
        // One chummyd per state folder.
        assert!(StateDir::open(&dir, key(1), "i-1").is_err());
        let mut s = st.blank();
        s.chats = serde_json::json!({ "a": 1 });
        s.paused.insert("a/c".into(), "eHl6".into());
        st.save(&s).unwrap();
        let raw = std::fs::read(dir.join("state.sealed")).unwrap();
        assert!(!String::from_utf8_lossy(&raw).contains("eHl6"));
        drop(st);

        let (_, back) = StateDir::open(&dir, key(1), "i-1").unwrap();
        assert_eq!(back.unwrap(), s);
        // Another key or instance: refused, and the file is left alone.
        let e = StateDir::open(&dir, key(2), "i-1").err().unwrap();
        assert!(e.contains("won't start"), "{e}");
        assert!(StateDir::open(&dir, key(1), "i-2").is_err());
        assert!(dir.join("state.sealed").exists());
        // A changed byte too.
        let mut bad = raw.clone();
        let last = bad.len() - 1;
        bad[last] ^= 1;
        std::fs::write(dir.join("state.sealed"), &bad).unwrap();
        assert!(StateDir::open(&dir, key(1), "i-1").is_err());
    }
}
