//! Small files sealed with a key kept only in the OS keychain: what a
//! channel keeps between runs (its login, an access token) in the Chummy
//! folder without the folder alone giving it away.
//!
//! The key (`key_name`, 32 random bytes) is made the first time and kept in
//! the keychain; the file is encrypted and authenticated with
//! XChaCha20-Poly1305, with `aad` (what it's for) sealed in too.

use std::path::Path;

use base64::Engine as _;
use base64::engine::general_purpose::STANDARD as B64;
use chacha20poly1305::aead::{Aead, KeyInit, Payload};
use chacha20poly1305::{XChaCha20Poly1305, XNonce};
use zeroize::Zeroizing;

use crate::secrets::SecretStore;
use crate::write_atomic;

const MAGIC: &[u8] = b"CHUMMY-SEALED\x01";
const NONCE_LEN: usize = 24;

fn random(n: usize) -> Result<Zeroizing<Vec<u8>>, String> {
    let mut b = Zeroizing::new(vec![0u8; n]);
    getrandom::fill(&mut b).map_err(|e| e.to_string())?;
    Ok(b)
}

/// The key named `key_name`, made (and kept in the keychain) if there's
/// none yet.
pub fn key(secrets: &dyn SecretStore, key_name: &str) -> Result<Zeroizing<Vec<u8>>, String> {
    if let Some(k) = secrets.get(key_name).map_err(|e| e.to_string())?
        && let Ok(bytes) = B64.decode(k.as_bytes())
        && bytes.len() == 32
    {
        return Ok(Zeroizing::new(bytes));
    }
    let k = random(32)?;
    secrets
        .set(key_name, &Zeroizing::new(B64.encode(&*k)))
        .map_err(|e| e.to_string())?;
    Ok(k)
}

/// Seals `plain` into `path` with the key `key_name`.
pub fn seal(
    secrets: &dyn SecretStore,
    key_name: &str,
    path: &Path,
    aad: &str,
    plain: &[u8],
) -> Result<(), String> {
    let key = key(secrets, key_name)?;
    let nonce = random(NONCE_LEN)?;
    let cipher = XChaCha20Poly1305::new_from_slice(&key).expect("32-byte key");
    let sealed = cipher
        .encrypt(
            <&XNonce>::try_from(&nonce[..]).expect("a 24-byte nonce"),
            Payload {
                msg: plain,
                aad: aad.as_bytes(),
            },
        )
        .map_err(|_| "couldn't encrypt".to_string())?;
    let mut data = Vec::with_capacity(MAGIC.len() + NONCE_LEN + sealed.len());
    data.extend_from_slice(MAGIC);
    data.extend_from_slice(&nonce);
    data.extend_from_slice(&sealed);
    write_atomic(path, &data, false).map_err(|e| e.to_string())
}

/// What's sealed in `path`, if there's a file that opens with the key
/// `key_name` (and `aad`). A file that doesn't open counts as none.
pub fn open(
    secrets: &dyn SecretStore,
    key_name: &str,
    path: &Path,
    aad: &str,
) -> Result<Option<Zeroizing<Vec<u8>>>, String> {
    let Ok(data) = std::fs::read(path) else {
        return Ok(None);
    };
    let Some(rest) = data.strip_prefix(MAGIC) else {
        return Ok(None);
    };
    if rest.len() < NONCE_LEN {
        return Ok(None);
    }
    let (nonce, sealed) = rest.split_at(NONCE_LEN);
    let key = key(secrets, key_name)?;
    let cipher = XChaCha20Poly1305::new_from_slice(&key).expect("32-byte key");
    Ok(cipher
        .decrypt(
            <&XNonce>::try_from(nonce).expect("a 24-byte nonce"),
            Payload {
                msg: sealed,
                aad: aad.as_bytes(),
            },
        )
        .ok()
        .map(Zeroizing::new))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::secrets::MemorySecrets;

    #[test]
    fn sealed_files_open_only_with_their_key_and_purpose() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("login.sealed");
        let s = MemorySecrets::default();
        assert_eq!(open(&s, "k", &path, "login").unwrap(), None);
        seal(&s, "k", &path, "login", b"token").unwrap();
        assert_eq!(
            open(&s, "k", &path, "login")
                .unwrap()
                .as_deref()
                .map(|v| v.as_slice()),
            Some(&b"token"[..])
        );
        assert_eq!(
            open(&s, "k", &path, "other").unwrap(),
            None,
            "another purpose"
        );
        assert_eq!(open(&s, "k2", &path, "login").unwrap(), None, "another key");
        let raw = std::fs::read(&path).unwrap();
        assert!(!raw.windows(5).any(|w| w == b"token"));
        // The key is kept, not made again.
        assert_eq!(*key(&s, "k").unwrap(), *key(&s, "k").unwrap());
    }
}
