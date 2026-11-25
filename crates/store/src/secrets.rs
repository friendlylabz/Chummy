//! Secrets (API keys, tokens) live in the OS keychain, never in the Chummy
//! folder: Keychain on macOS, Credential Manager on Windows, and the Secret
//! Service (GNOME Keyring, KWallet) on Linux.
//!
//! Code that needs secrets takes `&dyn SecretStore`, so tests can use
//! [`MemorySecrets`].

use std::collections::HashMap;
use std::sync::Mutex;

#[derive(Debug, thiserror::Error)]
pub enum SecretError {
    #[error(
        "The system keychain isn't available ({0}). On Linux, install and unlock a Secret \
         Service provider such as GNOME Keyring or KWallet."
    )]
    Unavailable(String),
    #[error("Keychain error: {0}")]
    Other(String),
}

pub trait SecretStore: Send + Sync {
    fn get(&self, key: &str) -> Result<Option<String>, SecretError>;
    fn set(&self, key: &str, value: &str) -> Result<(), SecretError>;
    /// Deleting a secret that doesn't exist is not an error.
    fn delete(&self, key: &str) -> Result<(), SecretError>;
}

/// The OS keychain. Entries are stored under `service` (the app identifier)
/// with `key` as the account name.
pub struct Keychain {
    service: String,
}

impl Keychain {
    pub fn new(service: impl Into<String>) -> Self {
        Self {
            service: service.into(),
        }
    }

    fn entry(&self, key: &str) -> Result<keyring::Entry, SecretError> {
        keyring::Entry::new(&self.service, key).map_err(map_err)
    }
}

fn map_err(e: keyring::Error) -> SecretError {
    match e {
        keyring::Error::PlatformFailure(_) | keyring::Error::NoStorageAccess(_) => {
            SecretError::Unavailable(e.to_string())
        }
        other => SecretError::Other(other.to_string()),
    }
}

impl SecretStore for Keychain {
    fn get(&self, key: &str) -> Result<Option<String>, SecretError> {
        match self.entry(key)?.get_password() {
            Ok(v) => Ok(Some(v)),
            Err(keyring::Error::NoEntry) => Ok(None),
            Err(e) => Err(map_err(e)),
        }
    }

    fn set(&self, key: &str, value: &str) -> Result<(), SecretError> {
        self.entry(key)?.set_password(value).map_err(map_err)
    }

    fn delete(&self, key: &str) -> Result<(), SecretError> {
        match self.entry(key)?.delete_credential() {
            Ok(()) | Err(keyring::Error::NoEntry) => Ok(()),
            Err(e) => Err(map_err(e)),
        }
    }
}

/// In-memory store for tests.
#[derive(Default)]
pub struct MemorySecrets(Mutex<HashMap<String, String>>);

impl SecretStore for MemorySecrets {
    fn get(&self, key: &str) -> Result<Option<String>, SecretError> {
        Ok(self
            .0
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .get(key)
            .cloned())
    }
    fn set(&self, key: &str, value: &str) -> Result<(), SecretError> {
        self.0
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .insert(key.into(), value.into());
        Ok(())
    }
    fn delete(&self, key: &str) -> Result<(), SecretError> {
        self.0.lock().unwrap_or_else(|e| e.into_inner()).remove(key);
        Ok(())
    }
}
