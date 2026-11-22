//! Encryption for run logs.
//!
//! Each log line is sealed on its own with XChaCha20-Poly1305 (a random
//! 192-bit nonce per line, so lines can be appended without coordination),
//! under a 256-bit key that lives in the OS keychain. A sealed line reads
//! `enc1:<key id>:<base64 of nonce ‖ ciphertext>`; the key id (the first 8
//! bytes of the key's SHA-256, in hex) says which key a line needs without
//! revealing it.
//!
//! The key can be exported to a file protected by a passphrase: Argon2id
//! (64 MiB, 3 passes) derives a key-encryption key that seals the log key,
//! again with XChaCha20-Poly1305.

use argon2::{Algorithm, Argon2, Params, Version};
use base64::Engine as _;
use base64::engine::general_purpose::STANDARD as B64;
use chacha20poly1305::aead::{Aead, KeyInit, Payload};
use chacha20poly1305::{XChaCha20Poly1305, XNonce};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use zeroize::Zeroizing;

/// What every sealed line starts with.
pub const PREFIX: &str = "enc1:";
/// Bound to every sealed line, so ciphertext from elsewhere doesn't open.
const LINE_AAD: &[u8] = b"chummy-log-line/1";
const EXPORT_AAD: &[u8] = b"chummy-log-key-export/1";
pub const EXPORT_FORMAT: &str = "chummy.logkey";
/// Shortest passphrase accepted for an export.
pub const MIN_PASSPHRASE: usize = 10;

/// A 256-bit log key. Wiped from memory when dropped.
#[derive(Clone)]
pub struct LogKey(Zeroizing<[u8; 32]>);

impl std::fmt::Debug for LogKey {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "LogKey({})", self.id())
    }
}

impl PartialEq for LogKey {
    fn eq(&self, other: &Self) -> bool {
        // Comparing ids avoids a timing difference on the key itself.
        self.id() == other.id()
    }
}

fn random<const N: usize>() -> Result<[u8; N], String> {
    let mut b = [0u8; N];
    getrandom::fill(&mut b).map_err(|e| format!("no secure random numbers: {e}"))?;
    Ok(b)
}

impl LogKey {
    /// A new random key from the OS's secure random source.
    pub fn generate() -> Result<LogKey, String> {
        Ok(LogKey(Zeroizing::new(random::<32>()?)))
    }

    /// For storing in the keychain.
    pub fn to_base64(&self) -> Zeroizing<String> {
        Zeroizing::new(B64.encode(self.0.as_ref()))
    }

    pub fn from_base64(s: &str) -> Result<LogKey, String> {
        let bytes = Zeroizing::new(B64.decode(s.trim()).map_err(|_| "the log key is damaged")?);
        let arr: [u8; 32] = bytes
            .as_slice()
            .try_into()
            .map_err(|_| "the log key is the wrong length")?;
        Ok(LogKey(Zeroizing::new(arr)))
    }

    /// The key's id: the first 8 bytes of its SHA-256, in hex.
    pub fn id(&self) -> String {
        let d = Sha256::digest(self.0.as_ref());
        d[..8].iter().map(|b| format!("{b:02x}")).collect()
    }

    /// The id in groups, for people to compare: `ab12-cd34-ef56-7890`.
    pub fn fingerprint(&self) -> String {
        let id = self.id();
        id.as_bytes()
            .chunks(4)
            .map(|c| std::str::from_utf8(c).unwrap_or(""))
            .collect::<Vec<_>>()
            .join("-")
    }

    fn cipher(&self) -> XChaCha20Poly1305 {
        XChaCha20Poly1305::new(self.0.as_ref().into())
    }

    /// Seals one line of text.
    pub fn seal(&self, line: &str) -> Result<String, String> {
        let nonce = random::<24>()?;
        let ct = self
            .cipher()
            .encrypt(
                XNonce::from_slice(&nonce),
                Payload {
                    msg: line.as_bytes(),
                    aad: LINE_AAD,
                },
            )
            .map_err(|_| "encryption failed")?;
        let mut blob = nonce.to_vec();
        blob.extend_from_slice(&ct);
        Ok(format!("{PREFIX}{}:{}", self.id(), B64.encode(blob)))
    }
}

/// Why a sealed line couldn't be opened.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum OpenError {
    /// There's no key to open it with.
    NoKey,
    /// It was sealed with another key (its id).
    OtherKey(String),
    /// It's damaged or was changed.
    Damaged,
}

/// Opens a line: plain lines come back as they are, sealed ones are
/// decrypted with `key`.
pub fn open(line: &str, key: Option<&LogKey>) -> Result<String, OpenError> {
    let Some(rest) = line.strip_prefix(PREFIX) else {
        return Ok(line.to_string());
    };
    let (id, b64) = rest.split_once(':').ok_or(OpenError::Damaged)?;
    let key = key.ok_or(OpenError::NoKey)?;
    if id != key.id() {
        return Err(OpenError::OtherKey(id.to_string()));
    }
    let blob = B64.decode(b64.trim()).map_err(|_| OpenError::Damaged)?;
    if blob.len() < 24 + 16 {
        return Err(OpenError::Damaged);
    }
    let (nonce, ct) = blob.split_at(24);
    let pt = key
        .cipher()
        .decrypt(
            XNonce::from_slice(nonce),
            Payload {
                msg: ct,
                aad: LINE_AAD,
            },
        )
        .map_err(|_| OpenError::Damaged)?;
    String::from_utf8(pt).map_err(|_| OpenError::Damaged)
}

/// An exported key, as saved to a file.
#[derive(Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct KeyExport {
    pub format: String,
    pub version: u32,
    /// The key's fingerprint, to check before importing.
    pub fingerprint: String,
    pub kdf: String,
    /// Argon2id memory in KiB, passes and lanes.
    pub m_cost: u32,
    pub t_cost: u32,
    pub p_cost: u32,
    pub salt: String,
    pub nonce: String,
    pub ciphertext: String,
}

const M_COST: u32 = 64 * 1024;
const T_COST: u32 = 3;
const P_COST: u32 = 1;

fn kek(
    passphrase: &str,
    salt: &[u8],
    m: u32,
    t: u32,
    p: u32,
) -> Result<Zeroizing<[u8; 32]>, String> {
    let params = Params::new(m, t, p, Some(32)).map_err(|e| format!("bad key settings: {e}"))?;
    let mut out = Zeroizing::new([0u8; 32]);
    Argon2::new(Algorithm::Argon2id, Version::V0x13, params)
        .hash_password_into(passphrase.as_bytes(), salt, out.as_mut())
        .map_err(|e| format!("couldn't derive a key: {e}"))?;
    Ok(out)
}

/// Seals `key` with a passphrase, for saving to a file.
pub fn export_key(key: &LogKey, passphrase: &str) -> Result<String, String> {
    if passphrase.chars().count() < MIN_PASSPHRASE {
        return Err(format!(
            "Use a passphrase of at least {MIN_PASSPHRASE} characters."
        ));
    }
    let salt = random::<16>()?;
    let nonce = random::<24>()?;
    let k = kek(passphrase, &salt, M_COST, T_COST, P_COST)?;
    let ct = XChaCha20Poly1305::new(k.as_ref().into())
        .encrypt(
            XNonce::from_slice(&nonce),
            Payload {
                msg: key.0.as_ref(),
                aad: EXPORT_AAD,
            },
        )
        .map_err(|_| "encryption failed")?;
    let e = KeyExport {
        format: EXPORT_FORMAT.into(),
        version: 1,
        fingerprint: key.fingerprint(),
        kdf: "argon2id".into(),
        m_cost: M_COST,
        t_cost: T_COST,
        p_cost: P_COST,
        salt: B64.encode(salt),
        nonce: B64.encode(nonce),
        ciphertext: B64.encode(ct),
    };
    serde_json::to_string_pretty(&e).map_err(|e| e.to_string())
}

/// Opens an exported key with its passphrase.
pub fn import_key(file: &str, passphrase: &str) -> Result<LogKey, String> {
    let e: KeyExport =
        serde_json::from_str(file).map_err(|_| "This isn't a Chummy log key file.")?;
    if e.format != EXPORT_FORMAT || e.version != 1 || e.kdf != "argon2id" {
        return Err("This log key file is from a newer or different version of Chummy.".into());
    }
    // Refuse settings that would take forever or exhaust memory.
    if !(8 * 1024..=1024 * 1024).contains(&e.m_cost)
        || !(1..=10).contains(&e.t_cost)
        || !(1..=8).contains(&e.p_cost)
    {
        return Err("This log key file's settings aren't allowed.".into());
    }
    let salt = B64
        .decode(&e.salt)
        .map_err(|_| "The log key file is damaged.")?;
    let nonce = B64
        .decode(&e.nonce)
        .map_err(|_| "The log key file is damaged.")?;
    let ct = B64
        .decode(&e.ciphertext)
        .map_err(|_| "The log key file is damaged.")?;
    if nonce.len() != 24 || salt.len() < 16 {
        return Err("The log key file is damaged.".into());
    }
    let k = kek(passphrase, &salt, e.m_cost, e.t_cost, e.p_cost)?;
    let pt = Zeroizing::new(
        XChaCha20Poly1305::new(k.as_ref().into())
            .decrypt(
                XNonce::from_slice(&nonce),
                Payload {
                    msg: &ct,
                    aad: EXPORT_AAD,
                },
            )
            .map_err(|_| "Wrong passphrase, or the file was changed.")?,
    );
    let arr: [u8; 32] = pt
        .as_slice()
        .try_into()
        .map_err(|_| "The log key file is damaged.")?;
    let key = LogKey(Zeroizing::new(arr));
    if key.fingerprint() != e.fingerprint {
        return Err("The log key file is damaged.".into());
    }
    Ok(key)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lines_round_trip_and_resist_tampering() {
        let k = LogKey::generate().unwrap();
        let sealed = k.seal("{\"secret\":\"sk-123\"}").unwrap();
        assert!(sealed.starts_with(PREFIX));
        assert!(!sealed.contains("sk-123"));
        assert_eq!(open(&sealed, Some(&k)).unwrap(), "{\"secret\":\"sk-123\"}");
        // Same text, different nonce.
        assert_ne!(sealed, k.seal("{\"secret\":\"sk-123\"}").unwrap());
        assert_eq!(open("plain", None).unwrap(), "plain");
        assert_eq!(open(&sealed, None), Err(OpenError::NoKey));
        let other = LogKey::generate().unwrap();
        assert!(matches!(
            open(&sealed, Some(&other)),
            Err(OpenError::OtherKey(_))
        ));
        let mut bad = sealed.clone();
        let last = bad.pop().unwrap();
        bad.push(if last == 'A' { 'B' } else { 'A' });
        assert_eq!(open(&bad, Some(&k)), Err(OpenError::Damaged));
        let again = LogKey::from_base64(&k.to_base64()).unwrap();
        assert_eq!(again, k);
        assert_eq!(k.fingerprint().len(), 19);
    }

    #[test]
    fn exports_need_the_passphrase() {
        let k = LogKey::generate().unwrap();
        assert!(export_key(&k, "short").is_err());
        let file = export_key(&k, "correct horse battery").unwrap();
        assert!(!file.contains(k.to_base64().as_str()));
        assert_eq!(import_key(&file, "correct horse battery").unwrap(), k);
        assert!(
            import_key(&file, "wrong horse battery")
                .unwrap_err()
                .contains("Wrong passphrase")
        );
        assert!(import_key("{}", "x").is_err());
    }
}
