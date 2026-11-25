//! The loops Chummy trusts, by fingerprint (`trusted-loops.json`).
//!
//! A loop runs with everything its agent may do, so Chummy only runs, opens
//! in the editor, or imports without a review the loops on this list:
//!
//! - the built-in templates (their fingerprints are worked out at build
//!   time by `crates/flow/build.rs`, so they're never stored here);
//! - every loop the user saves in the editor, when what they started from
//!   was trusted (a template, a new loop, or a loop already on the list);
//! - every loop the user reviewed and chose to trust;
//! - the loops that were already in the library the first time this
//!   version of Chummy started (they were made or imported before loops
//!   were reviewed).
//!
//! The file carries an HMAC-SHA256 keyed by a secret in the OS keychain
//! (`loop-trust-key`), so a program that edits the file can't add a loop
//! to it without that secret: a list that doesn't match its HMAC is
//! emptied (built-ins stay trusted) and every other loop needs a review
//! again. Without a keychain (some Linux setups) the list is kept without
//! an HMAC, and a list that was signed but can't be checked right now is
//! ignored until the keychain is back (and not overwritten).

use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

use chummy_log::system::{Level, log};
use chummy_store::secrets::SecretStore;
use hmac::{Hmac, KeyInit, Mac};
use serde::{Deserialize, Serialize};
use sha2::Sha256;

/// The keychain entry holding the list's HMAC key.
pub const TRUST_KEY_SECRET: &str = "loop-trust-key";
const FORMAT: &str = "chummy.trusted-loops";
const VERSION: u32 = 1;
/// Most fingerprints kept; the oldest go first. (Each save of a loop adds
/// one, so this is years of editing.)
const MAX_PRINTS: usize = 20_000;

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct TrustFile {
    format: String,
    version: u32,
    prints: Vec<String>,
    #[serde(default)]
    mac: Option<String>,
}

/// How the list was found when Chummy started.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum TrustState {
    /// Checked against the keychain key.
    Signed,
    /// No keychain, so kept without an HMAC.
    Unsigned,
    /// The file didn't match its HMAC (or couldn't be read), so it was
    /// emptied: every loop that isn't built in needs a review.
    Reset,
    /// The file is signed but the keychain can't be read right now, so
    /// only built-in loops (and ones trusted since) are trusted, and the
    /// file is left alone.
    Unverified,
}

struct Inner {
    prints: Vec<String>,
    set: HashSet<String>,
}

pub struct TrustStore {
    path: PathBuf,
    key: Option<Vec<u8>>,
    inner: Mutex<Inner>,
    pub state: TrustState,
}

fn hex(bytes: &[u8]) -> String {
    use std::fmt::Write as _;
    bytes.iter().fold(String::new(), |mut s, b| {
        let _ = write!(s, "{b:02x}");
        s
    })
}

fn unhex(s: &str) -> Option<Vec<u8>> {
    if !s.len().is_multiple_of(2) {
        return None;
    }
    (0..s.len())
        .step_by(2)
        .map(|i| u8::from_str_radix(s.get(i..i + 2)?, 16).ok())
        .collect()
}

fn mac_for(key: &[u8], prints: &[String]) -> Hmac<Sha256> {
    let mut m = <Hmac<Sha256> as KeyInit>::new_from_slice(key).expect("HMAC takes any key length");
    m.update(format!("{FORMAT}/{VERSION}\n").as_bytes());
    for p in prints {
        m.update(p.as_bytes());
        m.update(b"\n");
    }
    m
}

fn valid_print(p: &str) -> bool {
    p.len() == 64
        && p.bytes()
            .all(|b| b.is_ascii_hexdigit() && !b.is_ascii_uppercase())
}

/// The key from the keychain, made (and saved there) if there's none yet or
/// what's there isn't a key; `None` when there's no keychain. The flag says
/// whether the key is new (so no saved list can match it).
fn key(secrets: &dyn SecretStore) -> (Option<Vec<u8>>, bool) {
    let make = || -> Option<Vec<u8>> {
        let mut k = vec![0u8; 32];
        getrandom::fill(&mut k).ok()?;
        secrets.set(TRUST_KEY_SECRET, &hex(&k)).ok()?;
        log(
            Level::Info,
            "security",
            "Made a key for the list of trusted loops and saved it in the keychain",
            &[],
        );
        Some(k)
    };
    match secrets.get(TRUST_KEY_SECRET) {
        Ok(Some(s)) => match unhex(s.trim()).filter(|k| k.len() == 32) {
            Some(k) => (Some(k), false),
            None => (make(), true),
        },
        Ok(None) => (make(), true),
        Err(_) => (None, false),
    }
}

impl TrustStore {
    /// Opens the list. `existing` gives the fingerprints of the loops in the
    /// library, trusted when there's no list yet (the first start of a
    /// version of Chummy that reviews loops).
    pub fn open(
        path: &Path,
        secrets: &dyn SecretStore,
        existing: impl FnOnce() -> Vec<String>,
    ) -> Self {
        let (key, new_key) = key(secrets);
        let mut state = if key.is_some() {
            TrustState::Signed
        } else {
            TrustState::Unsigned
        };
        let mut prints: Vec<String> = Vec::new();
        let mut save = false;
        match std::fs::read_to_string(path) {
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                prints = existing();
                save = true;
                log(
                    Level::Info,
                    "security",
                    "Started the list of trusted loops with the loops already in the library",
                    &[("loops", &prints.len().to_string())],
                );
            }
            Err(e) => {
                state = TrustState::Reset;
                save = true;
                log(
                    Level::Warn,
                    "security",
                    "Couldn't read the list of trusted loops, so it was emptied",
                    &[("error", &e.to_string())],
                );
            }
            Ok(text) => match serde_json::from_str::<TrustFile>(&text) {
                Ok(f) if f.format == FORMAT && f.version == VERSION => {
                    let good = match (&key, &f.mac) {
                        (Some(k), Some(m)) => {
                            !new_key
                                && unhex(m)
                                    .is_some_and(|m| mac_for(k, &f.prints).verify_slice(&m).is_ok())
                        }
                        (Some(_), None) => false,
                        (None, None) => true,
                        (None, Some(_)) => {
                            state = TrustState::Unverified;
                            false
                        }
                    };
                    if good {
                        prints = f.prints.into_iter().filter(|p| valid_print(p)).collect();
                    } else if state != TrustState::Unverified {
                        state = TrustState::Reset;
                        save = true;
                        log(
                            Level::Warn,
                            "security",
                            "The list of trusted loops didn't match its keychain check (it was changed outside Chummy, or the key was replaced), so it was emptied. Loops that aren't built in need a review again.",
                            &[],
                        );
                    } else {
                        log(
                            Level::Warn,
                            "security",
                            "The keychain can't be read, so the list of trusted loops can't be checked; only built-in loops are trusted until it can",
                            &[],
                        );
                    }
                }
                _ => {
                    state = TrustState::Reset;
                    save = true;
                    log(
                        Level::Warn,
                        "security",
                        "The list of trusted loops isn't valid, so it was emptied",
                        &[],
                    );
                }
            },
        }
        prints.retain(|p| valid_print(p));
        let set = prints.iter().cloned().collect();
        let store = TrustStore {
            path: path.to_path_buf(),
            key,
            inner: Mutex::new(Inner { prints, set }),
            state,
        };
        if save {
            store.save(&store.lock());
        }
        store
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, Inner> {
        self.inner.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// Whether a loop with this fingerprint may run and be opened without
    /// a review.
    pub fn is_trusted(&self, print: &str) -> bool {
        chummy_flow::builtin_prints().any(|p| p == print) || self.lock().set.contains(print)
    }

    /// Adds a fingerprint to the list.
    pub fn trust(&self, print: &str) {
        if !valid_print(print) || chummy_flow::builtin_prints().any(|p| p == print) {
            return;
        }
        let mut inner = self.lock();
        if inner.set.contains(print) {
            return;
        }
        inner.set.insert(print.to_string());
        inner.prints.push(print.to_string());
        if inner.prints.len() > MAX_PRINTS {
            let extra = inner.prints.len() - MAX_PRINTS;
            let gone: Vec<String> = inner.prints.drain(..extra).collect();
            for g in gone {
                inner.set.remove(&g);
            }
        }
        self.save(&inner);
    }

    fn save(&self, inner: &Inner) {
        if self.state == TrustState::Unverified {
            // Don't replace a signed list we can't check with one that
            // isn't signed; it's used again once the keychain is back.
            return;
        }
        let file = TrustFile {
            format: FORMAT.into(),
            version: VERSION,
            prints: inner.prints.clone(),
            mac: self
                .key
                .as_ref()
                .map(|k| hex(&mac_for(k, &inner.prints).finalize().into_bytes())),
        };
        let text = serde_json::to_string_pretty(&file).unwrap_or_default() + "\n";
        if let Err(e) = chummy_store::write_atomic(&self.path, text.as_bytes(), false) {
            log(
                Level::Error,
                "security",
                "Couldn't save the list of trusted loops",
                &[("error", &e.to_string())],
            );
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chummy_store::secrets::{MemorySecrets, SecretError};

    struct NoKeychain;
    impl SecretStore for NoKeychain {
        fn get(&self, _: &str) -> Result<Option<String>, SecretError> {
            Err(SecretError::Unavailable("none here".into()))
        }
        fn set(&self, _: &str, _: &str) -> Result<(), SecretError> {
            Err(SecretError::Unavailable("none here".into()))
        }
        fn delete(&self, _: &str) -> Result<(), SecretError> {
            Ok(())
        }
    }

    fn print(n: u8) -> String {
        hex(&[n; 32])
    }

    #[test]
    fn starts_with_the_library_then_keeps_what_was_trusted() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("trusted-loops.json");
        let secrets = MemorySecrets::default();
        let t = TrustStore::open(&path, &secrets, || vec![print(1)]);
        assert_eq!(t.state, TrustState::Signed);
        assert!(t.is_trusted(&print(1)));
        assert!(!t.is_trusted(&print(2)));
        t.trust(&print(2));
        t.trust("not a print");
        let builtin = chummy_flow::builtin_prints().next().unwrap();
        assert!(t.is_trusted(builtin), "built-ins are always trusted");
        drop(t);
        let t = TrustStore::open(&path, &secrets, || panic!("only the first time"));
        assert_eq!(t.state, TrustState::Signed);
        assert!(t.is_trusted(&print(1)) && t.is_trusted(&print(2)));
        let saved = std::fs::read_to_string(&path).unwrap();
        assert!(!saved.contains(builtin), "built-ins aren't stored");
        assert!(!saved.contains("not a print"));
    }

    #[test]
    fn a_list_changed_outside_chummy_is_emptied() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("trusted-loops.json");
        let secrets = MemorySecrets::default();
        TrustStore::open(&path, &secrets, || vec![print(1)]).trust(&print(2));
        // Malware adds its own loop to the list.
        let mut f: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
        f["prints"].as_array_mut().unwrap().push(print(3).into());
        std::fs::write(&path, f.to_string()).unwrap();
        let t = TrustStore::open(&path, &secrets, Vec::new);
        assert_eq!(t.state, TrustState::Reset);
        assert!(!t.is_trusted(&print(3)) && !t.is_trusted(&print(1)));
        assert!(t.is_trusted(chummy_flow::builtin_prints().next().unwrap()));
        // Stripping the HMAC doesn't help either.
        f.as_object_mut().unwrap().remove("mac");
        std::fs::write(&path, f.to_string()).unwrap();
        assert_eq!(
            TrustStore::open(&path, &secrets, Vec::new).state,
            TrustState::Reset
        );
        // Nor does replacing the key (the old list can't be checked).
        TrustStore::open(&path, &secrets, Vec::new).trust(&print(4));
        secrets.delete(TRUST_KEY_SECRET).unwrap();
        let t = TrustStore::open(&path, &secrets, Vec::new);
        assert_eq!(t.state, TrustState::Reset);
        assert!(!t.is_trusted(&print(4)));
    }

    #[test]
    fn without_a_keychain_the_list_is_kept_unsigned() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("trusted-loops.json");
        let t = TrustStore::open(&path, &NoKeychain, || vec![print(1)]);
        assert_eq!(t.state, TrustState::Unsigned);
        t.trust(&print(2));
        let t = TrustStore::open(&path, &NoKeychain, Vec::new);
        assert!(t.is_trusted(&print(1)) && t.is_trusted(&print(2)));
    }

    #[test]
    fn a_signed_list_waits_for_the_keychain() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("trusted-loops.json");
        let secrets = MemorySecrets::default();
        TrustStore::open(&path, &secrets, || vec![print(1)]);
        let before = std::fs::read_to_string(&path).unwrap();
        let t = TrustStore::open(&path, &NoKeychain, Vec::new);
        assert_eq!(t.state, TrustState::Unverified);
        assert!(!t.is_trusted(&print(1)));
        t.trust(&print(2));
        assert!(t.is_trusted(&print(2)), "for now");
        assert_eq!(
            std::fs::read_to_string(&path).unwrap(),
            before,
            "left alone"
        );
        let t = TrustStore::open(&path, &secrets, Vec::new);
        assert_eq!(t.state, TrustState::Signed);
        assert!(t.is_trusted(&print(1)));
    }
}
