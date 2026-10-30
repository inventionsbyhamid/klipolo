//! Where the secrets live: the macOS Keychain in the app, memory in tests and on other systems. Nothing here logs or
//! prints a secret, and a store's errors never hold one.

use std::collections::HashMap;
use std::sync::{Arc, Mutex, PoisonError};

#[cfg(target_os = "macos")]
use klipolo_meta::ExposeSecret;
use klipolo_meta::SecretString;

/// A store for small secrets, one per account name, all under one service.
pub trait SecretStore: Send + Sync {
    /// The secret saved for `account`, or None when there is none.
    fn get(&self, account: &str) -> Result<Option<SecretString>, StoreError>;
    /// Saves `secret` for `account`, replacing what was there.
    fn set(&self, account: &str, secret: &SecretString) -> Result<(), StoreError>;
    /// Removes `account`'s secret; nothing to remove is fine.
    fn delete(&self, account: &str) -> Result<(), StoreError>;
}

/// A store that couldn't read, save or remove a secret. `detail` is the system's reason, for the log: it names the
/// account and the system's status, never the secret.
#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
#[error("the Keychain refused {action} {account}: {detail}")]
pub struct StoreError {
    pub action: &'static str,
    pub account: String,
    pub detail: String,
}

/// Secrets in memory, for tests and for systems without a Keychain. Its clones are copies, not views.
#[derive(Debug, Default)]
pub struct MemoryStore {
    secrets: Mutex<HashMap<String, SecretString>>,
}

impl Clone for MemoryStore {
    fn clone(&self) -> Self {
        Self {
            secrets: Mutex::new(self.lock().clone()),
        }
    }
}

impl MemoryStore {
    fn lock(&self) -> std::sync::MutexGuard<'_, HashMap<String, SecretString>> {
        self.secrets.lock().unwrap_or_else(PoisonError::into_inner)
    }
}

impl SecretStore for MemoryStore {
    fn get(&self, account: &str) -> Result<Option<SecretString>, StoreError> {
        Ok(self.lock().get(account).cloned())
    }

    fn set(&self, account: &str, secret: &SecretString) -> Result<(), StoreError> {
        self.lock().insert(account.to_owned(), secret.clone());
        Ok(())
    }

    fn delete(&self, account: &str) -> Result<(), StoreError> {
        self.lock().remove(account);
        Ok(())
    }
}

impl<S: SecretStore + ?Sized> SecretStore for Arc<S> {
    fn get(&self, account: &str) -> Result<Option<SecretString>, StoreError> {
        (**self).get(account)
    }

    fn set(&self, account: &str, secret: &SecretString) -> Result<(), StoreError> {
        (**self).set(account, secret)
    }

    fn delete(&self, account: &str) -> Result<(), StoreError> {
        (**self).delete(account)
    }
}

/// The login Keychain's generic passwords under one service (the app's is `com.klipolo.app`).
#[cfg(target_os = "macos")]
#[derive(Clone, Debug)]
pub struct KeychainStore {
    service: String,
}

#[cfg(target_os = "macos")]
impl KeychainStore {
    pub fn new(service: impl Into<String>) -> Self {
        Self {
            service: service.into(),
        }
    }
}

/// `errSecItemNotFound`: no item for that service and account.
#[cfg(target_os = "macos")]
const ITEM_NOT_FOUND: i32 = -25300;

#[cfg(target_os = "macos")]
impl SecretStore for KeychainStore {
    fn get(&self, account: &str) -> Result<Option<SecretString>, StoreError> {
        match security_framework::passwords::get_generic_password(&self.service, account) {
            Ok(bytes) => match String::from_utf8(bytes) {
                Ok(text) => Ok(Some(SecretString::from(text))),
                Err(_) => Err(StoreError {
                    action: "reading",
                    account: account.to_owned(),
                    detail: "the saved item isn't text".into(),
                }),
            },
            Err(e) if e.code() == ITEM_NOT_FOUND => Ok(None),
            Err(e) => Err(keychain_error("reading", account, e)),
        }
    }

    fn set(&self, account: &str, secret: &SecretString) -> Result<(), StoreError> {
        security_framework::passwords::set_generic_password(
            &self.service,
            account,
            secret.expose_secret().as_bytes(),
        )
        .map_err(|e| keychain_error("saving", account, e))
    }

    fn delete(&self, account: &str) -> Result<(), StoreError> {
        match security_framework::passwords::delete_generic_password(&self.service, account) {
            Ok(()) => Ok(()),
            Err(e) if e.code() == ITEM_NOT_FOUND => Ok(()),
            Err(e) => Err(keychain_error("removing", account, e)),
        }
    }
}

#[cfg(target_os = "macos")]
fn keychain_error(
    action: &'static str,
    account: &str,
    error: security_framework::base::Error,
) -> StoreError {
    StoreError {
        action,
        account: account.to_owned(),
        detail: format!("status {}: {error}", error.code()),
    }
}

#[cfg(test)]
mod tests {
    use klipolo_meta::ExposeSecret;

    use super::*;

    #[test]
    fn memory_store_keeps_replaces_and_deletes() {
        let store = MemoryStore::default();
        assert!(store.get("a").unwrap().is_none());
        store.set("a", &"one".into()).unwrap();
        store.set("a", &"two".into()).unwrap();
        assert_eq!(store.get("a").unwrap().unwrap().expose_secret(), "two");
        store.delete("a").unwrap();
        store.delete("a").unwrap();
        assert!(store.get("a").unwrap().is_none());
    }
}
