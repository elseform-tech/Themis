use super::*;

impl AppState {
    /// Overrides the custom provider base URL for tests.
    pub fn set_custom_base_url_override(&self, url: Option<String>) {
        *self
            .inner
            .custom_base_url_override
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner()) = url;
    }

    /// Returns the current settings.
    pub async fn get_settings(&self) -> Settings {
        self.inner.settings.get().await
    }

    /// Returns the diagnostics snapshot without exposing secret values.
    pub async fn get_diagnostics(&self) -> Diagnostics {
        Diagnostics {
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

    /// Reports which providers have a key stored (values never leave the store).
    pub async fn get_secret_status(&self) -> SecretStatus {
        SecretStatus {
            go: self.inner.secrets.has("go"),
            openai: self.inner.secrets.has("openai"),
            anthropic: self.inner.secrets.has("anthropic"),
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

    /// Looks up the API key for `provider` (Ollama needs none).
    pub(super) fn api_key_for(&self, provider: ProviderKind) -> Result<String, String> {
        if provider == ProviderKind::Ollama {
            return Ok(String::new());
        }
        let name = provider.as_str();
        self.inner
            .secrets
            .get(name)
            .filter(|key| !key.trim().is_empty())
            .ok_or_else(|| {
                format!("no API key stored for provider '{name}': add one in Settings, then retry")
            })
    }

    /// Custom base URL: test override first, then the environment fallback.
    pub(super) fn custom_base_url(&self) -> Option<String> {
        if let Some(url) = self
            .inner
            .custom_base_url_override
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .clone()
        {
            return Some(url);
        }
        std::env::var(CUSTOM_BASE_URL_ENV_VAR)
            .ok()
            .filter(|url| !url.trim().is_empty())
    }
}
