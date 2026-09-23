//! API-key storage behind a trait.
//!
//! Credentials exist only in process memory or the launch environment.
//! Values never cross the bridge back to the frontend.

use std::collections::HashMap;
use std::sync::Mutex;

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

#[cfg(test)]
mod session_tests {
    use super::*;
    #[test]
    fn entered_keys_do_not_survive_a_new_session() {
        let first = SessionStore::new();
        first.set("openai", "synthetic-session-key").unwrap();
        assert!(first.has("openai"));
        assert!(!SessionStore::new().has("openai"));
        first.clear("openai").unwrap();
        assert!(!first.has("openai"));
    }
}
