//! Secrets (API keys, tokens) live in the OS keychain, never in the Chummy
//! folder: Keychain on macOS, Credential Manager on Windows, and the Secret
//! Service (GNOME Keyring, KWallet) on Linux.
//!
//! Code that needs secrets takes `&dyn SecretStore`, so tests can use
//! [`MemorySecrets`].

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use keyring_core::api::CredentialStore;

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
/// with `key` as the account name: a generic password with that service and
/// account in the login keychain on macOS, a generic credential named
/// `key.service` on Windows, and an item with `service` and `username`
/// attributes in the Secret Service on Linux. These are the names keyring 3
/// used, so secrets saved by earlier versions are still found.
pub struct Keychain {
    service: String,
    /// The platform's store, or why it couldn't be opened.
    store: Result<Arc<CredentialStore>, String>,
}

impl Keychain {
    pub fn new(service: impl Into<String>) -> Self {
        Self {
            service: service.into(),
            store: platform_store().map_err(|e| e.to_string()),
        }
    }

    fn entry(&self, key: &str) -> Result<keyring_core::Entry, SecretError> {
        let store = self
            .store
            .as_ref()
            .map_err(|e| SecretError::Unavailable(e.clone()))?;
        store.build(&self.service, key, None).map_err(map_err)
    }
}

#[cfg(target_os = "macos")]
fn platform_store() -> keyring_core::Result<Arc<CredentialStore>> {
    Ok(apple_native_keyring_store::keychain::Store::new()?)
}

#[cfg(windows)]
fn platform_store() -> keyring_core::Result<Arc<CredentialStore>> {
    Ok(windows_native_keyring_store::Store::new()?)
}

#[cfg(target_os = "linux")]
fn platform_store() -> keyring_core::Result<Arc<CredentialStore>> {
    Ok(dbus_secret_service_keyring_store::Store::new()?)
}

#[cfg(not(any(target_os = "macos", windows, target_os = "linux")))]
fn platform_store() -> keyring_core::Result<Arc<CredentialStore>> {
    Err(keyring_core::Error::NotSupportedByStore(
        "no system keychain on this platform".into(),
    ))
}

fn map_err(e: keyring_core::Error) -> SecretError {
    match e {
        keyring_core::Error::PlatformFailure(_)
        | keyring_core::Error::NoStorageAccess(_)
        | keyring_core::Error::NoDefaultStore => SecretError::Unavailable(e.to_string()),
        other => SecretError::Other(other.to_string()),
    }
}

impl SecretStore for Keychain {
    fn get(&self, key: &str) -> Result<Option<String>, SecretError> {
        match self.entry(key)?.get_password() {
            Ok(v) => Ok(Some(v)),
            Err(keyring_core::Error::NoEntry) => Ok(None),
            Err(e) => Err(map_err(e)),
        }
    }

    fn set(&self, key: &str, value: &str) -> Result<(), SecretError> {
        self.entry(key)?.set_password(value).map_err(map_err)
    }

    fn delete(&self, key: &str) -> Result<(), SecretError> {
        match self.entry(key)?.delete_credential() {
            Ok(()) | Err(keyring_core::Error::NoEntry) => Ok(()),
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
