//! chummyd's bundle: its whole configuration ([`Config`]) in one file,
//! encrypted with a key from a password. The Chummy app makes it
//! ([`seal`]); chummyd opens it when it starts ([`open`]) and runs exactly
//! that until it stops.
//!
//! On disk:
//!
//! ```text
//! "CHUMMYD\x02"              8 bytes
//! header length              u32, little-endian (at most 4 KiB)
//! header                     JSON: format, version, instance id, Argon2id settings
//! nonce                      24 bytes
//! ciphertext                 XChaCha20-Poly1305 of the configuration's JSON
//! ```
//!
//! The key ([`BundleKey`]) is Argon2id of the password, salted with the
//! instance: every bundle of an instance exported with the same password
//! has the same key. So chummyd can be given the key (a password file holds
//! it, as [`BundleKey::to_line`]) and never the password itself. The
//! associated data is everything before the nonce, so the header can't be
//! changed either. Nothing in it is ever written to disk unencrypted.
//!
//! Version 1 (`CHUMMYD\x01`, a random salt in the header) isn't opened any
//! more: export it again.

mod config;
mod password;

use argon2::{Algorithm, Argon2, Params, Version};
use chacha20poly1305::aead::{Aead, KeyInit, Payload};
use chacha20poly1305::{XChaCha20Poly1305, XNonce};
use serde::{Deserialize, Serialize};
use zeroize::Zeroizing;

pub use config::{
    BundleAgent, CONFIG_FORMAT, CONFIG_VERSION, Channels, Config, Instance, Limits, Logging,
    MAX_CHAT_MESSAGES, MAX_CHATS, MAX_RUNS, MAX_UPLOAD_MB, MatrixChannel, Report, TerminalChannel,
    valid_id, valid_matrix_id,
};
pub use password::{generate_password, password_problem};

const MAGIC: &[u8; 8] = b"CHUMMYD\x02";
const MAGIC_V1: &[u8; 8] = b"CHUMMYD\x01";
pub const BUNDLE_FORMAT: &str = "chummyd.bundle";
pub const BUNDLE_VERSION: u32 = 2;
/// How a password file's line starts.
pub const KEY_LINE: &str = "chummyd-key-2";
/// The file name ending the app gives bundles.
pub const EXTENSION: &str = "chummyd";
/// Largest bundle opened.
pub const MAX_BUNDLE_BYTES: usize = 64 * 1024 * 1024;
const MAX_HEADER_BYTES: usize = 4096;

/// Argon2id settings: 64 MiB and 3 passes (about a second on a small
/// computer, like a Raspberry Pi). Fixed for version 2, since a key made
/// once must open every later bundle of the instance.
const M_COST: u32 = 64 * 1024;
const T_COST: u32 = 3;
const P_COST: u32 = 1;

#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum BundleError {
    #[error("This isn't a chummyd bundle.")]
    NotABundle,
    #[error("This bundle is from a newer or different version of Chummy.")]
    Version,
    #[error("This bundle was made by an older Chummy: export it again from Chummy.")]
    Old,
    #[error("The password is wrong, or the bundle was changed.")]
    WrongPassword,
    #[error("This password file is for another chummyd instance ({0}).")]
    OtherInstance(String),
    #[error("The bundle's configuration has problems: {}", .0.join(" "))]
    Invalid(Vec<String>),
    #[error("{0}")]
    Other(String),
}

/// The part of a bundle that isn't encrypted.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Header {
    pub format: String,
    pub version: u32,
    /// The instance the bundle configures (not secret).
    pub instance_id: String,
    pub kdf: Kdf,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Kdf {
    pub name: String,
    pub memory_kib: u32,
    pub iterations: u32,
    pub lanes: u32,
}

impl Kdf {
    fn current() -> Self {
        Self {
            name: "argon2id".into(),
            memory_kib: M_COST,
            iterations: T_COST,
            lanes: P_COST,
        }
    }
}

pub(crate) fn random<const N: usize>() -> Result<[u8; N], String> {
    let mut b = [0u8; N];
    getrandom::fill(&mut b).map_err(|e| format!("no randomness: {e}"))?;
    Ok(b)
}

/// The key a bundle is encrypted with: Argon2id of its password, salted with
/// its instance. What chummyd's password file holds, instead of the
/// password: it opens this instance's bundles, but doesn't give away the
/// password (which may be used elsewhere).
#[derive(Clone, PartialEq, Eq)]
pub struct BundleKey {
    instance_id: String,
    key: Zeroizing<[u8; 32]>,
}

impl std::fmt::Debug for BundleKey {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "BundleKey({}, …)", self.instance_id)
    }
}

impl BundleKey {
    /// The key for `instance_id`'s bundles from their password (about a
    /// second's work).
    pub fn from_password(password: &str, instance_id: &str) -> Result<Self, BundleError> {
        use sha2::{Digest, Sha256};
        let salt = Sha256::new()
            .chain_update(b"chummyd bundle key salt\0")
            .chain_update(instance_id.as_bytes())
            .finalize();
        let params = Params::new(M_COST, T_COST, P_COST, Some(32))
            .map_err(|e| BundleError::Other(format!("bad key settings: {e}")))?;
        let mut key = Zeroizing::new([0u8; 32]);
        Argon2::new(Algorithm::Argon2id, Version::V0x13, params)
            .hash_password_into(password.as_bytes(), &salt[..16], key.as_mut())
            .map_err(|e| BundleError::Other(format!("couldn't derive a key: {e}")))?;
        Ok(Self {
            instance_id: instance_id.to_string(),
            key,
        })
    }

    /// The key for a bundle, from its password (its instance is in its
    /// header).
    pub fn for_bundle(bytes: &[u8], password: &str) -> Result<Self, BundleError> {
        Self::from_password(password, &header(bytes)?.instance_id)
    }

    pub fn instance_id(&self) -> &str {
        &self.instance_id
    }

    /// As a password file holds it: `chummyd-key-2 <instance> <64 hex>`.
    pub fn to_line(&self) -> Zeroizing<String> {
        let hex: String = self.key.iter().map(|b| format!("{b:02x}")).collect();
        Zeroizing::new(format!("{KEY_LINE} {} {hex}", self.instance_id))
    }

    /// Reads a password file's line. Anything else (a password itself,
    /// say) is refused, saying how to make the file.
    pub fn from_line(line: &str) -> Result<Self, BundleError> {
        let made = "A password file holds the password's hash, never the password: make it with chummyd-docker-init, or with chummyd hash-password.";
        let parts: Vec<&str> = line.trim().split(' ').collect();
        let [tag, instance, hex] = parts.as_slice() else {
            return Err(BundleError::Other(made.into()));
        };
        if *tag != KEY_LINE || !valid_id(instance) || hex.len() != 64 {
            return Err(BundleError::Other(made.into()));
        }
        let mut key = Zeroizing::new([0u8; 32]);
        for (i, b) in key.iter_mut().enumerate() {
            *b = u8::from_str_radix(&hex[2 * i..2 * i + 2], 16)
                .map_err(|_| BundleError::Other(made.into()))?;
        }
        Ok(Self {
            instance_id: instance.to_string(),
            key,
        })
    }
}

/// Checks `config` and seals it with `password` (which must be strong: see
/// [`password_problem`]). Refused if the checks find errors.
pub fn seal(config: &Config, password: &str) -> Result<Vec<u8>, BundleError> {
    if let Some(problem) = password_problem(password) {
        return Err(BundleError::Other(problem));
    }
    seal_with(
        config,
        &BundleKey::from_password(password, &config.instance.id)?,
    )
}

/// Checks `config` and seals it with a key for its instance.
pub fn seal_with(config: &Config, key: &BundleKey) -> Result<Vec<u8>, BundleError> {
    if key.instance_id != config.instance.id {
        return Err(BundleError::OtherInstance(key.instance_id.clone()));
    }
    let report = config.check();
    if !report.ok() {
        return Err(BundleError::Invalid(report.errors));
    }
    let header = Header {
        format: BUNDLE_FORMAT.into(),
        version: BUNDLE_VERSION,
        instance_id: config.instance.id.clone(),
        kdf: Kdf::current(),
    };
    let header_json = serde_json::to_vec(&header).map_err(|e| BundleError::Other(e.to_string()))?;
    let mut out = Vec::with_capacity(64 * 1024);
    out.extend_from_slice(MAGIC);
    out.extend_from_slice(&(header_json.len() as u32).to_le_bytes());
    out.extend_from_slice(&header_json);
    let nonce = random::<24>().map_err(BundleError::Other)?;
    let plain =
        Zeroizing::new(serde_json::to_vec(config).map_err(|e| BundleError::Other(e.to_string()))?);
    let ct = XChaCha20Poly1305::new_from_slice(key.key.as_ref())
        .expect("a 32-byte key")
        .encrypt(
            <&XNonce>::try_from(&nonce[..]).expect("a 24-byte nonce"),
            Payload {
                msg: &plain,
                aad: &out,
            },
        )
        .map_err(|_| BundleError::Other("encryption failed".into()))?;
    out.extend_from_slice(&nonce);
    out.extend_from_slice(&ct);
    Ok(out)
}

/// A bundle's header, read without its password.
pub fn header(bytes: &[u8]) -> Result<Header, BundleError> {
    Ok(split(bytes)?.0)
}

/// A bundle taken apart: header, everything before the nonce (the
/// associated data), nonce, ciphertext.
type Parts<'a> = (Header, &'a [u8], &'a [u8], &'a [u8]);

fn split(bytes: &[u8]) -> Result<Parts<'_>, BundleError> {
    if bytes.len() > MAX_BUNDLE_BYTES {
        return Err(BundleError::Other("This bundle is too large.".into()));
    }
    if bytes.len() >= 8 && &bytes[..8] == MAGIC_V1 {
        return Err(BundleError::Old);
    }
    if bytes.len() < 12 || &bytes[..8] != MAGIC {
        return Err(BundleError::NotABundle);
    }
    let len = u32::from_le_bytes(bytes[8..12].try_into().expect("4 bytes")) as usize;
    if len > MAX_HEADER_BYTES || bytes.len() < 12 + len + 24 + 16 {
        return Err(BundleError::NotABundle);
    }
    let header: Header =
        serde_json::from_slice(&bytes[12..12 + len]).map_err(|_| BundleError::NotABundle)?;
    if header.format != BUNDLE_FORMAT {
        return Err(BundleError::NotABundle);
    }
    if header.version != BUNDLE_VERSION || header.kdf != Kdf::current() {
        return Err(BundleError::Version);
    }
    let aad_end = 12 + len;
    Ok((
        header,
        &bytes[..aad_end],
        &bytes[aad_end..aad_end + 24],
        &bytes[aad_end + 24..],
    ))
}

/// Opens a bundle with its key, and checks what's in it. A bundle whose
/// configuration has errors isn't opened.
pub fn open(bytes: &[u8], key: &BundleKey) -> Result<Config, BundleError> {
    let config = open_unchecked(bytes, key)?;
    let report = config.check();
    if !report.ok() {
        return Err(BundleError::Invalid(report.errors));
    }
    Ok(config)
}

/// Opens a bundle without checking what's in it (to load it back into the
/// app and fix it). It's still authenticated and must parse strictly.
pub fn open_unchecked(bytes: &[u8], key: &BundleKey) -> Result<Config, BundleError> {
    let (header, aad, nonce, ct) = split(bytes)?;
    if key.instance_id != header.instance_id {
        return Err(BundleError::OtherInstance(key.instance_id.clone()));
    }
    let plain = Zeroizing::new(
        XChaCha20Poly1305::new_from_slice(key.key.as_ref())
            .expect("a 32-byte key")
            .decrypt(
                <&XNonce>::try_from(nonce).expect("a 24-byte nonce"),
                Payload { msg: ct, aad },
            )
            .map_err(|_| BundleError::WrongPassword)?,
    );
    let config: Config = serde_json::from_slice(&plain).map_err(|e| {
        BundleError::Invalid(vec![format!("Its configuration can't be read: {e}.")])
    })?;
    if config.format != CONFIG_FORMAT || config.version != CONFIG_VERSION {
        return Err(BundleError::Version);
    }
    if config.instance.id != header.instance_id {
        return Err(BundleError::NotABundle);
    }
    Ok(config)
}

#[cfg(test)]
mod tests;
