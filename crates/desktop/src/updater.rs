//! In-app update checks (`tauri-plugin-updater`).
//!
//! The updater is live only when `tauri.conf.json` carries a `plugins.updater`
//! section with both a non-empty `endpoints` array and a non-empty `pubkey`
//! (see `crates/desktop/UPDATER_NOTES.md` for the maintainer setup). Without
//! either, [`check_for_updates`](crate::commands::check_for_updates) reports
//! [`UpdateState::Disabled`](crate::types::UpdateState) without touching the
//! network. No signing keys are needed to build or run the app itself.

use serde_json::Value;

use crate::types::UpdateStatus;

/// Message returned when the updater has no usable configuration.
pub const UPDATER_DISABLED_MESSAGE: &str =
    "updates not configured: set plugins.updater endpoints and pubkey (see UPDATER_NOTES.md)";

/// Why an updater config is (un)usable (internal classification for logging).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UpdaterDisabledReason {
    /// `plugins.updater` section absent.
    MissingConfig,
    /// No non-empty endpoint URL configured.
    MissingEndpoints,
    /// No public key configured.
    MissingPubkey,
    /// Fully configured; a live check should proceed.
    Configured,
}

/// Returns the disabled status when `updater_config` — the `plugins.updater`
/// value from the Tauri config — lacks a usable endpoint+pubkey pair, or
/// `None` when the caller should proceed to a live update check.
///
/// Pure over the config JSON so the disabled path is testable without a
/// Tauri app handle; the command passes
/// `app.config().plugins.0.get("updater")`.
#[must_use]
pub fn disabled_status(updater_config: Option<&Value>) -> Option<UpdateStatus> {
    if disabled_reason(updater_config) == UpdaterDisabledReason::Configured {
        None
    } else {
        Some(UpdateStatus::disabled(UPDATER_DISABLED_MESSAGE))
    }
}

/// Whether the updater plugin may be registered for this config value.
///
/// The plugin panics at startup when `plugins.updater` is absent or fails to
/// deserialize, so `run()` registers it only when this returns true. Kept in
/// sync with the plugin's own `Config` by deserializing it directly.
#[must_use]
pub fn plugin_usable(updater_config: Option<&Value>) -> bool {
    updater_config.is_some_and(|value| {
        serde_json::from_value::<tauri_plugin_updater::Config>(value.clone()).is_ok()
    })
}

/// Classifies why an updater config is unusable (for logging/debugging).
#[must_use]
pub fn disabled_reason(updater_config: Option<&Value>) -> UpdaterDisabledReason {
    match updater_config {
        None => UpdaterDisabledReason::MissingConfig,
        Some(config) => {
            let has_endpoint = config
                .get("endpoints")
                .and_then(Value::as_array)
                .is_some_and(|endpoints| {
                    endpoints
                        .iter()
                        .any(|endpoint| endpoint.as_str().is_some_and(|url| !url.trim().is_empty()))
                });
            let has_pubkey = config
                .get("pubkey")
                .and_then(Value::as_str)
                .is_some_and(|pubkey| !pubkey.trim().is_empty());
            match (has_endpoint, has_pubkey) {
                (true, true) => UpdaterDisabledReason::Configured,
                (false, _) => UpdaterDisabledReason::MissingEndpoints,
                (true, false) => UpdaterDisabledReason::MissingPubkey,
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;
    use crate::types::UpdateState;

    #[test]
    fn missing_config_is_disabled() {
        let status = disabled_status(None).expect("disabled");
        assert_eq!(status.state, UpdateState::Disabled);
        assert!(status
            .message
            .expect("message")
            .contains("updates not configured"));
        assert_eq!(disabled_reason(None), UpdaterDisabledReason::MissingConfig);
    }

    #[test]
    fn partial_config_is_disabled() {
        for (config, reason) in [
            (
                json!({"endpoints": [], "pubkey": "abc"}),
                UpdaterDisabledReason::MissingEndpoints,
            ),
            (
                json!({"endpoints": [""], "pubkey": "abc"}),
                UpdaterDisabledReason::MissingEndpoints,
            ),
            (
                json!({"endpoints": ["https://r.example/latest.json"]}),
                UpdaterDisabledReason::MissingPubkey,
            ),
            (
                json!({
                    "endpoints": ["https://r.example/latest.json"],
                    "pubkey": "  ",
                }),
                UpdaterDisabledReason::MissingPubkey,
            ),
            (
                json!({"endpoints": "https://r.example/latest.json", "pubkey": "abc"}),
                UpdaterDisabledReason::MissingEndpoints,
            ),
        ] {
            let status = disabled_status(Some(&config)).expect("disabled");
            assert_eq!(status.state, UpdateState::Disabled, "{config}");
            assert_eq!(disabled_reason(Some(&config)), reason, "{config}");
        }
    }

    #[test]
    fn full_config_proceeds_to_live_check() {
        let config = json!({
            "endpoints": ["https://releases.example/{{target}}/{{arch}}/{{current_version}}"],
            "pubkey": "dW50cnVzdGVkIGNvbW1lbnQ6IHJlc3VsdA==",
        });
        assert!(disabled_status(Some(&config)).is_none());
        assert_eq!(
            disabled_reason(Some(&config)),
            UpdaterDisabledReason::Configured
        );
    }

    #[test]
    fn plugin_registers_only_for_deserializable_config() {
        // Missing section: the shipped configuration. Registering the plugin
        // here panicked at startup (null is not a Config struct).
        assert!(!plugin_usable(None));
        // Present but unusable: must not register either.
        assert!(!plugin_usable(Some(&json!({}))));
        assert!(!plugin_usable(Some(
            &json!({"endpoints": ["https://r.example/latest.json"]})
        )));
        // Fully deserializable: safe to register.
        assert!(plugin_usable(Some(&json!({
            "endpoints": ["https://r.example/latest.json"],
            "pubkey": "abc",
        }))));
    }
}
