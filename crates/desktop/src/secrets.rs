//! API-key storage behind a trait.
//!
//! Production credentials live in the current user's OS credential store;
//! Go also accepts its launch environment.
//! Values never cross the bridge back to the frontend.

use std::collections::HashMap;
use std::sync::Mutex;

fn credential_with_fallback(
    stored: Result<Option<String>, String>,
    fallback: Option<String>,
) -> Result<Option<String>, String> {
    match stored {
        Ok(Some(value)) => Ok(Some(value)),
        other => match fallback.filter(|value| !value.trim().is_empty()) {
            Some(value) => Ok(Some(value)),
            None => other,
        },
    }
}

/// The OpenCode Go key's name in Settings › Keys (and its environment variable).
pub const GO_KEY_NAME: &str = "OPENCODE_KEY";
/// Where the OpenCode Go key is kept: the Settings › Keys entry `OPENCODE_KEY`, so MCP
/// servers can reference it as `${OPENCODE_KEY}` too.
pub const GO_ENTRY: &str = "key:OPENCODE_KEY";
/// Where the key was kept before it joined Settings › Keys (moved on first read).
pub const LEGACY_GO_ENTRY: &str = "go";

fn go_environment_key(provider: &str) -> Option<String> {
    (provider == GO_ENTRY)
        .then(|| std::env::var(GO_KEY_NAME).ok())
        .flatten()
}

#[cfg(target_os = "macos")]
pub type ProductionSecretStore = KeychainStore;
#[cfg(not(target_os = "macos"))]
pub type ProductionSecretStore = NativeStore;

#[cfg(target_os = "macos")]
const KEYCHAIN_SERVICE: &str = "ai.themis.desktop";

#[cfg(not(target_os = "macos"))]
const CREDENTIAL_SERVICE: &str = "ai.themis.desktop";

/// Shared validation for desktop and terminal credential entry; errors omit values.
pub fn validate_provider_key(value: &str) -> Result<(), String> {
    if value.is_empty() || value.len() > 4096 || !value.bytes().all(|byte| byte.is_ascii_graphic())
    {
        return Err(
            "API key must contain 1–4096 printable ASCII characters without whitespace".to_owned(),
        );
    }
    Ok(())
}

/// Windows Credential Manager or the Linux Secret Service for the current user.
#[cfg(not(target_os = "macos"))]
pub struct NativeStore;

#[cfg(not(target_os = "macos"))]
impl NativeStore {
    pub fn new() -> Self {
        Self
    }

    fn entry(provider: &str) -> Result<keyring::Entry, String> {
        keyring::Entry::new(CREDENTIAL_SERVICE, provider).map_err(|err| err.to_string())
    }
}

#[cfg(not(target_os = "macos"))]
impl Default for NativeStore {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(not(target_os = "macos"))]
impl SecretStore for NativeStore {
    fn has(&self, provider: &str) -> bool {
        self.get(provider).is_some()
    }

    fn get(&self, provider: &str) -> Option<String> {
        self.try_get(provider).ok().flatten()
    }

    fn try_get(&self, provider: &str) -> Result<Option<String>, String> {
        let stored = Self::entry(provider)
            .map_err(|_| "Could not open the OS credential store".to_owned())
            .and_then(|entry| match entry.get_password() {
                Ok(value) => Ok(Some(value).filter(|value| !value.trim().is_empty())),
                Err(keyring::Error::NoEntry) => Ok(None),
                Err(_) => Err("OS credential lookup failed; unlock the credential store and allow ThemisCode access".to_owned()),
            });
        credential_with_fallback(stored, go_environment_key(provider))
    }

    fn set(&self, provider: &str, value: &str) -> Result<(), String> {
        Self::entry(provider)?.set_password(value).map_err(|err| {
            format!("could not save {provider} key in the OS credential store: {err}")
        })
    }

    fn clear(&self, provider: &str) -> Result<(), String> {
        match Self::entry(provider)?.delete_credential() {
            Ok(()) | Err(keyring::Error::NoEntry) => Ok(()),
            Err(err) => Err(format!("could not forget {provider} key: {err}")),
        }
    }
}

#[cfg(target_os = "macos")]
pub struct KeychainStore;

#[cfg(target_os = "macos")]
impl KeychainStore {
    pub fn new() -> Self {
        Self
    }

    fn decode_lookup(result: Result<Vec<u8>, i32>) -> Result<Option<String>, String> {
        match result {
            Ok(bytes) => String::from_utf8(bytes)
                .map(|value| Some(value).filter(|value| !value.trim().is_empty()))
                .map_err(|_| "Stored credential is not valid UTF-8".to_owned()),
            Err(code) if code == security_framework_sys::base::errSecItemNotFound => Ok(None),
            Err(code) => Err(format!(
                "Credential lookup failed (Keychain status {code}); unlock Keychain and allow ThemisCode access"
            )),
        }
    }

    fn keychain_get(provider: &str) -> Result<Option<String>, String> {
        Self::decode_lookup(
            security_framework::passwords::get_generic_password(KEYCHAIN_SERVICE, provider)
                .map_err(|error| error.code()),
        )
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
        self.try_get(provider).ok().flatten()
    }

    fn try_get(&self, provider: &str) -> Result<Option<String>, String> {
        credential_with_fallback(Self::keychain_get(provider), go_environment_key(provider))
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

/// Stores the OpenCode Go API key.
pub trait SecretStore: Send + Sync {
    /// Returns true when a non-empty secret is stored for `provider`.
    fn has(&self, provider: &str) -> bool;
    /// Returns the stored secret, or `None` when missing.
    fn get(&self, provider: &str) -> Option<String>;
    /// Fallible application lookup; OS stores distinguish unavailable from absent.
    fn try_get(&self, provider: &str) -> Result<Option<String>, String> {
        Ok(self.get(provider))
    }
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
                (provider == GO_ENTRY)
                    .then(|| std::env::var(GO_KEY_NAME).ok())
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
    fn credential_fallback_preserves_errors_unless_a_valid_key_is_available() {
        assert_eq!(credential_with_fallback(Ok(None), None), Ok(None));
        assert_eq!(
            credential_with_fallback(Err("unavailable".into()), Some("  ".into())),
            Err("unavailable".into())
        );
        assert_eq!(
            credential_with_fallback(Err("unavailable".into()), Some("synthetic-env".into())),
            Ok(Some("synthetic-env".into()))
        );
        assert_eq!(
            credential_with_fallback(
                Ok(Some("synthetic-stored".into())),
                Some("synthetic-env".into())
            ),
            Ok(Some("synthetic-stored".into()))
        );
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn keychain_result_distinguishes_missing_access_failure_and_invalid_data() {
        assert_eq!(
            KeychainStore::decode_lookup(Err(security_framework_sys::base::errSecItemNotFound)),
            Ok(None)
        );
        let error = KeychainStore::decode_lookup(Err(-25308)).unwrap_err();
        assert!(error.contains("Keychain status -25308"));
        assert!(KeychainStore::decode_lookup(Ok(vec![255])).is_err());
        assert_eq!(
            KeychainStore::decode_lookup(Ok(b"synthetic-key".to_vec())),
            Ok(Some("synthetic-key".into()))
        );
    }

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

#[cfg(test)]
mod key_validation_tests {
    use super::*;
    #[test]
    fn provider_key_validation_rejects_invalid_values_without_echoing_them() {
        for value in [
            "",
            "synthetic with spaces",
            "synthetic\nkey",
            "µ-key",
            &"x".repeat(4097),
        ] {
            let error = validate_provider_key(value).unwrap_err();
            if !value.is_empty() {
                assert!(!error.contains(value));
            }
        }
        assert!(validate_provider_key("synthetic-key_123").is_ok());
    }
}
