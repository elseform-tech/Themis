//! API-key storage behind a trait.
//!
//! On macOS credentials live in Keychain; Go also accepts its launch environment.
//! Values never cross the bridge back to the frontend.

use std::collections::HashMap;
use std::sync::Mutex;

#[cfg(target_os = "macos")]
pub type ProductionSecretStore = KeychainStore;
#[cfg(not(target_os = "macos"))]
pub type ProductionSecretStore = SessionStore;

#[cfg(target_os = "macos")]
const KEYCHAIN_SERVICE: &str = "ai.themis.desktop";

#[cfg(target_os = "macos")]
pub struct KeychainStore;

#[cfg(target_os = "macos")]
impl KeychainStore {
    pub fn new() -> Self {
        Self
    }

    fn keychain_get(provider: &str) -> Option<String> {
        match security_framework::passwords::get_generic_password(KEYCHAIN_SERVICE, provider) {
            Ok(bytes) => String::from_utf8(bytes)
                .ok()
                .filter(|value| !value.trim().is_empty()),
            Err(error) if error.code() == security_framework_sys::base::errSecItemNotFound => None,
            Err(error) => {
                eprintln!("themis: keychain read failed for {provider}: {error}");
                None
            }
        }
    }
}

#[cfg(target_os = "macos")]
impl Default for KeychainStore {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(target_os = "macos")]
impl SecretStore for KeychainStore {
    fn has(&self, provider: &str) -> bool {
        self.get(provider).is_some()
    }

    fn get(&self, provider: &str) -> Option<String> {
        Self::keychain_get(provider).or_else(|| {
            (provider == "go")
                .then(|| std::env::var("OPENCODE_KEY").ok())
                .flatten()
                .filter(|value| !value.trim().is_empty())
        })
    }

    fn set(&self, provider: &str, value: &str) -> Result<(), String> {
        security_framework::passwords::set_generic_password(
            KEYCHAIN_SERVICE,
            provider,
            value.as_bytes(),
        )
        .map_err(|error| format!("could not save {provider} key in Keychain: {error}"))
    }

    fn clear(&self, provider: &str) -> Result<(), String> {
        match security_framework::passwords::delete_generic_password(KEYCHAIN_SERVICE, provider) {
            Ok(()) => Ok(()),
            Err(error) if error.code() == security_framework_sys::base::errSecItemNotFound => {
                Ok(())
            }
            Err(error) => Err(format!(
                "could not forget {provider} key in Keychain: {error}"
            )),
        }
    }
}

/// Stores one API key per provider name (`go`, `openai`, `anthropic`, `custom`).
pub trait SecretStore: Send + Sync {
    /// Returns true when a non-empty secret is stored for `provider`.
    fn has(&self, provider: &str) -> bool;
    /// Returns the stored secret, or `None` when missing.
    fn get(&self, provider: &str) -> Option<String>;
    /// Stores `value` for `provider`, overwriting any previous secret.
    fn set(&self, provider: &str, value: &str) -> Result<(), String>;
    /// Deletes the secret for `provider`; missing secrets are a no-op success.
    fn clear(&self, provider: &str) -> Result<(), String>;
}

/// Session-only credentials with an OpenCode Go environment fallback.
#[derive(Default)]
pub struct SessionStore {
    keys: MemoryStore,
}

impl SessionStore {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }
}

impl SecretStore for SessionStore {
    fn has(&self, provider: &str) -> bool {
        self.get(provider).is_some()
    }
    fn get(&self, provider: &str) -> Option<String> {
        self.keys
            .get(provider)
            .or_else(|| {
                (provider == "go")
                    .then(|| std::env::var("OPENCODE_KEY").ok())
                    .flatten()
            })
            .filter(|key| !key.trim().is_empty())
    }
    fn set(&self, provider: &str, value: &str) -> Result<(), String> {
        self.keys.set(provider, value)
    }
    fn clear(&self, provider: &str) -> Result<(), String> {
        self.keys.clear(provider)
    }
}

/// In-memory [`SecretStore`] for tests.
#[derive(Debug, Default)]
pub struct MemoryStore {
    inner: Mutex<HashMap<String, String>>,
}

impl MemoryStore {
    /// Creates an empty store.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }
}

impl SecretStore for MemoryStore {
    fn has(&self, provider: &str) -> bool {
        self.get(provider).is_some_and(|value| !value.is_empty())
    }

    fn get(&self, provider: &str) -> Option<String> {
        self.inner
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .get(provider)
            .cloned()
    }

    fn set(&self, provider: &str, value: &str) -> Result<(), String> {
        self.inner
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .insert(provider.to_owned(), value.to_owned());
        Ok(())
    }

    fn clear(&self, provider: &str) -> Result<(), String> {
        self.inner
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .remove(provider);
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn memory_store_roundtrips() {
        let store = MemoryStore::new();
        assert!(!store.has("go"));
        assert!(store.get("go").is_none());
        // Clearing a missing secret is a no-op success.
        assert!(store.clear("go").is_ok());

        assert!(store.set("go", "secret-1").is_ok());
        assert!(store.has("go"));
        assert_eq!(store.get("go").as_deref(), Some("secret-1"));

        // Overwrite wins; providers are independent.
        assert!(store.set("go", "secret-2").is_ok());
        assert_eq!(store.get("go").as_deref(), Some("secret-2"));
        assert!(!store.has("openai"));

        assert!(store.clear("go").is_ok());
        assert!(!store.has("go"));
        assert!(store.get("go").is_none());
    }
}

#[cfg(all(test, target_os = "macos"))]
mod keychain_tests {
    use super::*;
    #[test]
    fn entered_keys_survive_new_store_and_can_be_forgotten() {
        let provider = format!("synthetic-test-{}", uuid::Uuid::new_v4());
        let first = KeychainStore::new();
        first.set(&provider, "synthetic-test-key").unwrap();
        assert_eq!(
            KeychainStore::new().get(&provider).as_deref(),
            Some("synthetic-test-key")
        );
        first.clear(&provider).unwrap();
        assert!(KeychainStore::new().get(&provider).is_none());
    }
}
