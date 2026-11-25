//! Paused chat runs, sealed so only this Chummy can have made them.
//!
//! A paused run holds everything the run had seen and the loop it runs,
//! and resuming it carries on running that loop with the agent's access.
//! So a paused run must never come from anywhere but Chummy itself, and
//! each one is kept like this:
//!
//! - One file per chat, `paused/<agent>/<chat>.run`, encrypted and
//!   authenticated with XChaCha20-Poly1305.
//! - Each pause gets a **new random key**, kept only in the OS keychain
//!   (`paused-run:<agent>:<chat>`), never on disk. A file that wasn't sealed
//!   with the chat's current key doesn't open: a file someone made, an
//!   older pause put back, or another chat's file renamed. The agent and
//!   chat ids are sealed in too (as associated data), so a file only opens
//!   as the chat it was made for.
//! - Resuming **takes** the run: the key is deleted before the run starts,
//!   so the same paused state can't be resumed twice, and copies of the
//!   file never open again.
//! - A file that doesn't open is deleted, with its key.
//!
//! There is no import or export of paused runs.

use std::path::{Path, PathBuf};

use base64::Engine as _;
use base64::engine::general_purpose::STANDARD as B64;
use chacha20poly1305::aead::{Aead, KeyInit, Payload};
use chacha20poly1305::{XChaCha20Poly1305, XNonce};
use zeroize::Zeroizing;

use crate::secrets::{SecretError, SecretStore};

/// Starts every sealed file (and is checked first).
const MAGIC: &[u8] = b"CHUMMY-PAUSED-RUN\x01";
const NONCE_LEN: usize = 24;
/// Largest sealed file read. Far beyond a real paused run; a bigger file
/// isn't read into memory at all.
pub const MAX_SEALED_BYTES: u64 = 512 * 1024 * 1024;

#[derive(Debug, thiserror::Error)]
pub enum PauseError {
    /// The keychain can't be used right now; the file is left alone.
    #[error(transparent)]
    Keychain(#[from] SecretError),
    /// The file isn't one this Chummy sealed for this chat (or is damaged).
    /// It has been deleted.
    #[error("the paused run couldn't be opened ({0}), so it was removed")]
    Untrusted(String),
    #[error("{0}")]
    Io(String),
}

/// Agent and chat ids become file names and keychain entries, so only a
/// safe alphabet is allowed.
fn valid_id(id: &str) -> bool {
    !id.is_empty()
        && id.len() <= 80
        && !id.starts_with('.')
        && id
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
}

fn check_ids(agent: &str, chat: &str) -> Result<(), PauseError> {
    if valid_id(agent) && valid_id(chat) {
        Ok(())
    } else {
        Err(PauseError::Io("Invalid id.".into()))
    }
}

fn key_name(agent: &str, chat: &str) -> String {
    format!("paused-run:{agent}:{chat}")
}

/// What the seal binds the file to.
fn associated(agent: &str, chat: &str) -> Vec<u8> {
    format!("chummy.paused-run/1\n{agent}\n{chat}").into_bytes()
}

/// The paused runs in the Chummy folder.
pub struct PauseVault {
    dir: PathBuf,
}

impl PauseVault {
    pub fn new(dir: impl Into<PathBuf>) -> Self {
        Self { dir: dir.into() }
    }

    fn path(&self, agent: &str, chat: &str) -> PathBuf {
        self.dir.join(agent).join(format!("{chat}.run"))
    }

    /// Seals `plain` as the chat's paused run, replacing any before it
    /// (whose key is replaced too, so it can't be opened again).
    pub fn seal(
        &self,
        secrets: &dyn SecretStore,
        agent: &str,
        chat: &str,
        plain: &[u8],
    ) -> Result<(), PauseError> {
        check_ids(agent, chat)?;
        let random = |n: usize| -> Result<Zeroizing<Vec<u8>>, PauseError> {
            let mut b = Zeroizing::new(vec![0u8; n]);
            getrandom::fill(&mut b).map_err(|e| PauseError::Io(e.to_string()))?;
            Ok(b)
        };
        let key = random(32)?;
        let nonce = random(NONCE_LEN)?;
        let cipher = XChaCha20Poly1305::new_from_slice(&key).expect("32-byte key");
        let sealed = cipher
            .encrypt(
                <&XNonce>::try_from(&nonce[..]).expect("a 24-byte nonce"),
                Payload {
                    msg: plain,
                    aad: &associated(agent, chat),
                },
            )
            .map_err(|_| PauseError::Io("couldn't encrypt the paused run".into()))?;

        // The key first: if the file can't be written after, the key is
        // just removed again.
        let encoded = Zeroizing::new(B64.encode(&*key));
        secrets.set(&key_name(agent, chat), &encoded)?;
        let mut data = Vec::with_capacity(MAGIC.len() + NONCE_LEN + sealed.len());
        data.extend_from_slice(MAGIC);
        data.extend_from_slice(&nonce);
        data.extend_from_slice(&sealed);
        if let Err(e) = write_atomic(&self.path(agent, chat), &data) {
            let _ = secrets.delete(&key_name(agent, chat));
            return Err(PauseError::Io(format!("couldn't save the paused run: {e}")));
        }
        Ok(())
    }

    /// The chat's paused run, if it has one, without taking it. A file that
    /// doesn't open is deleted (with its key) and reported as
    /// [`PauseError::Untrusted`].
    pub fn open(
        &self,
        secrets: &dyn SecretStore,
        agent: &str,
        chat: &str,
    ) -> Result<Option<Zeroizing<Vec<u8>>>, PauseError> {
        check_ids(agent, chat)?;
        let path = self.path(agent, chat);
        let meta = match std::fs::symlink_metadata(&path) {
            Ok(m) => m,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Err(e) => return Err(PauseError::Io(e.to_string())),
        };
        if !meta.file_type().is_file() {
            return Err(self.reject(secrets, agent, chat, "it isn't a plain file"));
        }
        if meta.len() > MAX_SEALED_BYTES {
            return Err(self.reject(secrets, agent, chat, "it's too large"));
        }
        // A keychain that can't be read right now leaves the file alone.
        let Some(encoded) = secrets.get(&key_name(agent, chat))?.map(Zeroizing::new) else {
            return Err(self.reject(secrets, agent, chat, "Chummy holds no key for it"));
        };
        let key = match B64.decode(encoded.as_bytes()) {
            Ok(k) if k.len() == 32 => Zeroizing::new(k),
            _ => return Err(self.reject(secrets, agent, chat, "its key is damaged")),
        };
        let data = std::fs::read(&path).map_err(|e| PauseError::Io(e.to_string()))?;
        let Some(rest) = data.strip_prefix(MAGIC) else {
            return Err(self.reject(secrets, agent, chat, "it isn't a paused run"));
        };
        if rest.len() < NONCE_LEN {
            return Err(self.reject(secrets, agent, chat, "it's cut short"));
        }
        let (nonce, sealed) = rest.split_at(NONCE_LEN);
        let cipher = XChaCha20Poly1305::new_from_slice(&key).expect("32-byte key");
        match cipher.decrypt(
            <&XNonce>::try_from(nonce).expect("a 24-byte nonce"),
            Payload {
                msg: sealed,
                aad: &associated(agent, chat),
            },
        ) {
            Ok(plain) => Ok(Some(Zeroizing::new(plain))),
            Err(_) => Err(self.reject(
                secrets,
                agent,
                chat,
                "it wasn't sealed by this Chummy for this chat",
            )),
        }
    }

    /// Opens the chat's paused run and removes it, key first, so it can
    /// only be resumed this once. If the key can't be deleted, nothing is
    /// taken.
    pub fn take(
        &self,
        secrets: &dyn SecretStore,
        agent: &str,
        chat: &str,
    ) -> Result<Option<Zeroizing<Vec<u8>>>, PauseError> {
        let Some(plain) = self.open(secrets, agent, chat)? else {
            return Ok(None);
        };
        secrets.delete(&key_name(agent, chat))?;
        let _ = std::fs::remove_file(self.path(agent, chat));
        Ok(Some(plain))
    }

    /// Removes the chat's paused run, if any.
    pub fn discard(
        &self,
        secrets: &dyn SecretStore,
        agent: &str,
        chat: &str,
    ) -> Result<(), PauseError> {
        check_ids(agent, chat)?;
        // The key first: without it, a file left behind never opens.
        secrets.delete(&key_name(agent, chat))?;
        match std::fs::remove_file(self.path(agent, chat)) {
            Ok(()) => Ok(()),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(e) => Err(PauseError::Io(e.to_string())),
        }
    }

    /// Removes every paused run of an agent.
    pub fn discard_agent(&self, secrets: &dyn SecretStore, agent: &str) {
        for (a, chat) in self.list() {
            if a == agent {
                let _ = self.discard(secrets, &a, &chat);
            }
        }
        let _ = std::fs::remove_dir(self.dir.join(agent));
    }

    /// The (agent, chat) of every paused run file there is, whether or not
    /// it opens.
    pub fn list(&self) -> Vec<(String, String)> {
        let mut out = Vec::new();
        let Ok(agents) = std::fs::read_dir(&self.dir) else {
            return out;
        };
        for a in agents.flatten() {
            let agent = a.file_name().to_string_lossy().to_string();
            if !valid_id(&agent) || !a.file_type().is_ok_and(|t| t.is_dir()) {
                continue;
            }
            let Ok(files) = std::fs::read_dir(a.path()) else {
                continue;
            };
            for f in files.flatten() {
                let name = f.file_name().to_string_lossy().to_string();
                if let Some(chat) = name.strip_suffix(".run").filter(|c| valid_id(c)) {
                    out.push((agent.clone(), chat.to_string()));
                }
            }
        }
        out.sort();
        out
    }

    /// Deletes a file that didn't open, and its key.
    fn reject(&self, secrets: &dyn SecretStore, agent: &str, chat: &str, why: &str) -> PauseError {
        let _ = secrets.delete(&key_name(agent, chat));
        let _ = std::fs::remove_file(self.path(agent, chat));
        PauseError::Untrusted(why.into())
    }
}

/// Writes through a temporary file in the same folder, so a crash never
/// leaves half a file.
fn write_atomic(path: &Path, data: &[u8]) -> std::io::Result<()> {
    let dir = path.parent().expect("a file in a folder");
    std::fs::create_dir_all(dir)?;
    crate::restrict_to_owner(dir)?;
    let tmp = dir.join(format!(
        ".{}.tmp",
        path.file_name().unwrap_or_default().to_string_lossy()
    ));
    std::fs::write(&tmp, data)?;
    crate::restrict_to_owner(&tmp)?;
    std::fs::rename(&tmp, path)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::secrets::MemorySecrets;

    fn vault() -> (tempfile::TempDir, PauseVault, MemorySecrets) {
        let dir = tempfile::tempdir().unwrap();
        let v = PauseVault::new(dir.path().join("paused"));
        (dir, v, MemorySecrets::default())
    }

    #[test]
    fn a_sealed_run_opens_once_for_its_own_chat() {
        let (_d, v, keys) = vault();
        v.seal(&keys, "agent1", "chatA", b"state").unwrap();
        let file = std::fs::read(v.path("agent1", "chatA")).unwrap();
        assert!(!file.windows(5).any(|w| w == b"state"), "encrypted on disk");
        assert_eq!(v.list(), [("agent1".to_string(), "chatA".to_string())]);
        assert_eq!(
            &**v.open(&keys, "agent1", "chatA").unwrap().unwrap(),
            b"state"
        );
        assert_eq!(
            &**v.take(&keys, "agent1", "chatA").unwrap().unwrap(),
            b"state"
        );
        // Taken: gone, key and all.
        assert!(v.open(&keys, "agent1", "chatA").unwrap().is_none());
        assert!(keys.get("paused-run:agent1:chatA").unwrap().is_none());

        // A copy of the file put back doesn't open: its key is gone.
        v.seal(&keys, "agent1", "chatA", b"new").unwrap();
        let copy = std::fs::read(v.path("agent1", "chatA")).unwrap();
        v.take(&keys, "agent1", "chatA").unwrap();
        std::fs::write(v.path("agent1", "chatA"), &copy).unwrap();
        assert!(matches!(
            v.open(&keys, "agent1", "chatA"),
            Err(PauseError::Untrusted(_))
        ));
        assert!(!v.path("agent1", "chatA").exists(), "removed");
    }

    #[test]
    fn files_chummy_did_not_seal_never_open() {
        let (_d, v, keys) = vault();
        // An older pause of the same chat, put back over a newer one.
        v.seal(&keys, "a", "c", b"old").unwrap();
        let old = std::fs::read(v.path("a", "c")).unwrap();
        v.seal(&keys, "a", "c", b"newer").unwrap();
        std::fs::write(v.path("a", "c"), &old).unwrap();
        assert!(matches!(
            v.open(&keys, "a", "c"),
            Err(PauseError::Untrusted(_))
        ));

        // Another chat's file, moved over (even with its key copied).
        v.seal(&keys, "a", "other", b"theirs").unwrap();
        v.seal(&keys, "a", "c", b"mine").unwrap();
        std::fs::copy(v.path("a", "other"), v.path("a", "c")).unwrap();
        let theirs = keys.get("paused-run:a:other").unwrap().unwrap();
        keys.set("paused-run:a:c", &theirs).unwrap();
        assert!(matches!(
            v.open(&keys, "a", "c"),
            Err(PauseError::Untrusted(_))
        ));

        // A file put there with no key at all.
        std::fs::write(v.path("a", "made"), b"CHUMMY-PAUSED-RUN\x01whatever").unwrap();
        assert!(matches!(
            v.open(&keys, "a", "made"),
            Err(PauseError::Untrusted(_))
        ));
        assert!(!v.path("a", "made").exists());

        // One bit changed.
        v.seal(&keys, "a", "flip", b"state").unwrap();
        let mut data = std::fs::read(v.path("a", "flip")).unwrap();
        let last = data.len() - 1;
        data[last] ^= 1;
        std::fs::write(v.path("a", "flip"), &data).unwrap();
        assert!(matches!(
            v.open(&keys, "a", "flip"),
            Err(PauseError::Untrusted(_))
        ));

        // Odd ids are refused before touching anything.
        assert!(v.seal(&keys, "../x", "c", b"s").is_err());
        assert!(v.open(&keys, "a", "c/../../x").is_err());
    }

    #[test]
    fn discarding_removes_the_run_and_its_key() {
        let (_d, v, keys) = vault();
        v.seal(&keys, "a", "c1", b"1").unwrap();
        v.seal(&keys, "a", "c2", b"2").unwrap();
        v.seal(&keys, "b", "c3", b"3").unwrap();
        v.discard(&keys, "a", "c1").unwrap();
        assert!(v.open(&keys, "a", "c1").unwrap().is_none());
        v.discard_agent(&keys, "a");
        assert_eq!(v.list(), [("b".to_string(), "c3".to_string())]);
        assert!(keys.get("paused-run:a:c2").unwrap().is_none());
    }
}
