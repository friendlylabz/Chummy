//! chummyd's bundle: its whole configuration ([`Config`]) in one file,
//! encrypted with a password. The Chummy app makes it ([`seal`]); chummyd
//! opens it when it starts ([`open`]) and runs exactly that until it stops.
//!
//! On disk:
//!
//! ```text
//! "CHUMMYD\x01"              8 bytes
//! header length              u32, little-endian (at most 4 KiB)
//! header                     JSON: format, version, instance id, Argon2id settings and salt
//! nonce                      24 bytes
//! ciphertext                 XChaCha20-Poly1305 of the configuration's JSON
//! ```
//!
//! The key is Argon2id of the password with the header's settings; the
//! associated data is everything before the nonce, so the header can't be
//! changed either. Nothing in it is ever written to disk unencrypted.

mod config;
mod password;

use argon2::{Algorithm, Argon2, Params, Version};
use base64::Engine as _;
use base64::engine::general_purpose::STANDARD as B64;
use chacha20poly1305::aead::{Aead, KeyInit, Payload};
use chacha20poly1305::{XChaCha20Poly1305, XNonce};
use serde::{Deserialize, Serialize};
use zeroize::Zeroizing;

pub use config::{
    BundleAgent, CONFIG_FORMAT, CONFIG_VERSION, Channels, Config, Instance, Limits, Logging,
    MAX_RUNS, MAX_UPLOAD_MB, MatrixChannel, Report, TerminalChannel, valid_id, valid_matrix_id,
};
pub use password::{generate_password, password_problem};

const MAGIC: &[u8; 8] = b"CHUMMYD\x01";
pub const BUNDLE_FORMAT: &str = "chummyd.bundle";
pub const BUNDLE_VERSION: u32 = 1;
/// The file name ending the app gives bundles.
pub const EXTENSION: &str = "chummyd";
/// Largest bundle opened.
pub const MAX_BUNDLE_BYTES: usize = 64 * 1024 * 1024;
const MAX_HEADER_BYTES: usize = 4096;

/// Argon2id settings for new bundles: 64 MiB and 3 passes (about a second
/// on a small computer, like a Raspberry Pi).
const M_COST: u32 = 64 * 1024;
const T_COST: u32 = 3;
const P_COST: u32 = 1;

#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum BundleError {
    #[error("This isn't a chummyd bundle.")]
    NotABundle,
    #[error("This bundle is from a newer or different version of Chummy.")]
    Version,
    #[error("The password is wrong, or the bundle was changed.")]
    WrongPassword,
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
    pub salt: String,
}

pub(crate) fn random<const N: usize>() -> Result<[u8; N], String> {
    let mut b = [0u8; N];
    getrandom::fill(&mut b).map_err(|e| format!("no randomness: {e}"))?;
    Ok(b)
}

fn key_for(password: &str, kdf: &Kdf) -> Result<Zeroizing<[u8; 32]>, BundleError> {
    // Refuse settings that would take forever or use up memory.
    if kdf.name != "argon2id"
        || !(8 * 1024..=1024 * 1024).contains(&kdf.memory_kib)
        || !(1..=10).contains(&kdf.iterations)
        || !(1..=8).contains(&kdf.lanes)
    {
        return Err(BundleError::Version);
    }
    let salt = B64.decode(&kdf.salt).map_err(|_| BundleError::NotABundle)?;
    if salt.len() < 16 {
        return Err(BundleError::NotABundle);
    }
    let params = Params::new(kdf.memory_kib, kdf.iterations, kdf.lanes, Some(32))
        .map_err(|e| BundleError::Other(format!("bad key settings: {e}")))?;
    let mut out = Zeroizing::new([0u8; 32]);
    Argon2::new(Algorithm::Argon2id, Version::V0x13, params)
        .hash_password_into(password.as_bytes(), &salt, out.as_mut())
        .map_err(|e| BundleError::Other(format!("couldn't derive a key: {e}")))?;
    Ok(out)
}

/// Checks `config` and seals it with `password` (which must be strong: see
/// [`password_problem`]). Refused if the checks find errors.
pub fn seal(config: &Config, password: &str) -> Result<Vec<u8>, BundleError> {
    if let Some(problem) = password_problem(password) {
        return Err(BundleError::Other(problem));
    }
    let report = config.check();
    if !report.ok() {
        return Err(BundleError::Invalid(report.errors));
    }
    let header = Header {
        format: BUNDLE_FORMAT.into(),
        version: BUNDLE_VERSION,
        instance_id: config.instance.id.clone(),
        kdf: Kdf {
            name: "argon2id".into(),
            memory_kib: M_COST,
            iterations: T_COST,
            lanes: P_COST,
            salt: B64.encode(random::<16>().map_err(BundleError::Other)?),
        },
    };
    let key = key_for(password, &header.kdf)?;
    let header_json = serde_json::to_vec(&header).map_err(|e| BundleError::Other(e.to_string()))?;
    let mut out = Vec::with_capacity(64 * 1024);
    out.extend_from_slice(MAGIC);
    out.extend_from_slice(&(header_json.len() as u32).to_le_bytes());
    out.extend_from_slice(&header_json);
    let nonce = random::<24>().map_err(BundleError::Other)?;
    let plain =
        Zeroizing::new(serde_json::to_vec(config).map_err(|e| BundleError::Other(e.to_string()))?);
    let ct = XChaCha20Poly1305::new_from_slice(key.as_ref())
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
    if header.version != BUNDLE_VERSION {
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

/// Opens a bundle with its password, and checks what's in it. A bundle
/// whose configuration has errors isn't opened.
pub fn open(bytes: &[u8], password: &str) -> Result<Config, BundleError> {
    let config = open_unchecked(bytes, password)?;
    let report = config.check();
    if !report.ok() {
        return Err(BundleError::Invalid(report.errors));
    }
    Ok(config)
}

/// Opens a bundle without checking what's in it (to load it back into the
/// app and fix it). It's still authenticated and must parse strictly.
pub fn open_unchecked(bytes: &[u8], password: &str) -> Result<Config, BundleError> {
    let (header, aad, nonce, ct) = split(bytes)?;
    let key = key_for(password, &header.kdf)?;
    let plain = Zeroizing::new(
        XChaCha20Poly1305::new_from_slice(key.as_ref())
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
    if config.format != CONFIG_FORMAT || !(1..=CONFIG_VERSION).contains(&config.version) {
        return Err(BundleError::Version);
    }
    if config.instance.id != header.instance_id {
        return Err(BundleError::NotABundle);
    }
    Ok(config)
}

#[cfg(test)]
mod tests;
