use super::*;

impl AppState {
    /// Test-only Go endpoint override for synthetic provider responses.
    pub fn set_go_base_url_override(&self, url: Option<String>) {
        *self
            .inner
            .go_base_url_override
            .lock()
            .unwrap_or_else(|error| error.into_inner()) = url;
    }

    /// Returns the current settings.
    pub async fn get_settings(&self) -> Settings {
        self.inner.settings.get().await
    }

    /// Returns the diagnostics snapshot without exposing secret values.
    pub async fn get_diagnostics(&self) -> Diagnostics {
        self.get_diagnostics_with_depth(false).await
    }

    pub async fn get_diagnostics_with_depth(&self, deep: bool) -> Diagnostics {
        let threads = self.inner.threads.read().await.keys().cloned().collect();
        Diagnostics {
            persistence: self.inner.transcript.diagnostics(deep, &threads),
            recovery_notes: self.reconcile_report().len(),
            app_version: env!("CARGO_PKG_VERSION").to_owned(),
            os: std::env::consts::OS.to_owned(),
            settings: self.inner.settings.get().await,
            recent_errors: self.recent_errors(),
        }
    }

    /// Applies a settings patch (validated) and returns the new settings.
    pub async fn update_settings(&self, patch: SettingsPatch) -> Result<Settings, String> {
        self.inner.settings.update(patch).await
    }

    /// Reports whether the Go key is available (the value never leaves the store).
    pub async fn get_secret_status(&self) -> SecretStatus {
        SecretStatus {
            go: self.inner.secrets.has("go"),
        }
    }

    /// Stores the API key for `provider`.
    pub async fn set_secret(&self, provider: String, value: String) -> Result<(), String> {
        let key = secret_key(&provider)?;
        if value.trim().is_empty() {
            return Err("secret value must not be empty".to_owned());
        }
        self.inner.secrets.set(key, &value)
    }

    /// Deletes the API key for `provider` (missing keys are a no-op success).
    pub async fn clear_secret(&self, provider: String) -> Result<(), String> {
        let key = secret_key(&provider)?;
        self.inner.secrets.clear(key)
    }

    /// Looks up the Go API key.
    pub(super) fn api_key_for(&self, provider: ProviderKind) -> Result<String, String> {
        let name = provider.as_str();
        self.inner
            .secrets
            .get(name)
            .filter(|key| !key.trim().is_empty())
            .ok_or_else(|| {
                format!("no API key stored for provider '{name}': add one in Settings, then retry")
            })
    }
}
