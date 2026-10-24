//! Model providers besides OpenCode Go: the OpenAI-compatible ones `providers` in the user
//! `runtime.jsonc` defines. A chat names their models as `<provider>/<model>`; their keys are
//! Settings › Secrets secrets. Status checks log metadata only, never keys or bodies.
use super::*;
use serde_json::{json, Value};
use themis_core::configuration::{provider_for, ProviderDefinition, RuntimeConfig};
use themis_core::providers::{CustomEndpoint, GoModel, ModelListStatus, ProviderConfig};

/// What a configured provider's check found.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ProviderState {
    /// Its `/models` answered with the key.
    Works,
    /// Its `/models` refused the key (401 or 403).
    Rejected,
    /// It could not be reached, or its models list failed.
    Unavailable,
    /// The secret has no value.
    Missing,
    /// The key is set and the models are declared in the file; the provider has no usable
    /// `/models` to check it with.
    Unchecked,
}

impl ProviderState {
    fn as_str(self) -> &'static str {
        match self {
            Self::Works => "works",
            Self::Rejected => "rejected",
            Self::Unavailable => "unavailable",
            Self::Missing => "missing",
            Self::Unchecked => "unchecked",
        }
    }
}

fn endpoint(provider: &ProviderDefinition) -> CustomEndpoint {
    CustomEndpoint {
        id: provider.id.clone(),
        name: provider.name.clone(),
        base_url: provider.base_url.clone(),
    }
}

impl AppState {
    /// The providers the user `runtime.jsonc` defines (none while it is missing or invalid;
    /// Settings › Permissions reports an invalid file).
    pub(super) fn user_providers(&self) -> Vec<ProviderDefinition> {
        self.runtime_configuration(None, None)
            .map(|resolved| resolved.config.providers)
            .unwrap_or_default()
    }

    /// A configured provider's models and what its check found. Declared models need no
    /// request unless `check` is set; a `/models` list is read once and kept (per base URL)
    /// until checked again.
    async fn provider_models(
        &self,
        provider: &ProviderDefinition,
        check: bool,
    ) -> (ProviderState, Vec<GoModel>) {
        let declared = provider.models.is_some();
        let cached = self
            .inner
            .provider_catalogs
            .read()
            .await
            .get(&provider.id)
            .filter(|(base, _)| *base == provider.base_url)
            .map(|(_, models)| models.clone());
        if !check {
            if declared {
                return (
                    ProviderState::Unchecked,
                    themis_core::providers::provider_models(provider, &[]),
                );
            }
            if let Some(models) = cached.clone() {
                return (ProviderState::Works, models);
            }
        }
        let key = match self
            .read_named_secret(provider.secret_name().to_owned())
            .await
        {
            Ok(Some(key)) => key,
            Ok(None) => return (ProviderState::Missing, Vec::new()),
            Err(_) => return (ProviderState::Unavailable, Vec::new()),
        };
        let started = std::time::Instant::now();
        let listed =
            themis_core::providers::refresh_provider_models(&provider.base_url, &key).await;
        let state = match &listed {
            Ok(_) => ProviderState::Works,
            Err(error)
                if error
                    .downcast_ref::<ModelListStatus>()
                    .is_some_and(|status| matches!(status.0, 401 | 403)) =>
            {
                ProviderState::Rejected
            }
            Err(_) if declared => ProviderState::Unchecked,
            Err(_) => ProviderState::Unavailable,
        };
        let models = match listed {
            Ok(ids) => {
                let models = themis_core::providers::provider_models(provider, &ids);
                if !declared {
                    self.inner.provider_catalogs.write().await.insert(
                        provider.id.clone(),
                        (provider.base_url.clone(), models.clone()),
                    );
                }
                models
            }
            Err(_) if declared && state != ProviderState::Rejected => {
                themis_core::providers::provider_models(provider, &[])
            }
            Err(_) if state == ProviderState::Unavailable => cached.unwrap_or_default(),
            Err(_) => Vec::new(),
        };
        let _ = self.inner.diagnostics.append(
            "providers",
            "provider_checked",
            if matches!(state, ProviderState::Works | ProviderState::Unchecked) {
                "info"
            } else {
                "warn"
            },
            json!({"provider": provider.id, "result": state.as_str(), "models": models.len(),
                   "duration_ms": started.elapsed().as_millis() as u64}),
        );
        (state, models)
    }

    /// Settings › Models: each provider from the user `runtime.jsonc` with its secret, what
    /// its check found and how many models it offers. Never key values.
    pub async fn list_model_providers(&self) -> Result<Value, String> {
        let mut listed = Vec::new();
        for provider in self.user_providers() {
            let (state, models) = self.provider_models(&provider, true).await;
            listed.push(json!({
                "id": provider.id,
                "name": provider.name,
                "secret": provider.secret_name(),
                "status": state.as_str(),
                "models": models.len(),
            }));
        }
        Ok(Value::Array(listed))
    }

    /// The OpenCode Go catalog followed by every configured provider's models. Without any
    /// configured provider, a Go failure is the error; with them, Go's models are left out.
    pub async fn list_go_models(&self) -> Result<Vec<GoModel>, String> {
        let providers = self.user_providers();
        let mut models = match self.refresh_go_catalog().await {
            Ok(models) => models,
            Err(error) if providers.is_empty() => return Err(error),
            Err(_) => Vec::new(),
        };
        for provider in &providers {
            models.extend(self.provider_models(provider, false).await.1);
        }
        Ok(models)
    }

    /// The models a run on `config` may use, with its `models` overrides applied: the loaded
    /// Go catalog plus the models of the configured providers that `wanted` names.
    pub(super) async fn run_catalog(
        &self,
        config: &RuntimeConfig,
        wanted: &[&str],
    ) -> Vec<GoModel> {
        let mut models = self.inner.go_catalog.read().await.clone();
        for provider in &config.providers {
            if wanted
                .iter()
                .any(|model| provider_for(std::slice::from_ref(provider), model).is_some())
            {
                models.extend(self.provider_models(provider, false).await.1);
            }
        }
        themis_core::providers::apply_model_overrides(&models, &config.models)
    }

    /// Whether `model` belongs to a provider `config` defines.
    pub(super) fn is_configured_model(config: &RuntimeConfig, model: &str) -> bool {
        provider_for(&config.providers, model).is_some()
    }

    /// The provider settings for a chat on `model`, with its key: a configured provider's
    /// endpoint and secret, else OpenCode Go. When `compaction.summary_model` is on another
    /// provider, that provider (and key) writes the summaries.
    pub(super) async fn provider_config_for(
        &self,
        config: &RuntimeConfig,
        model: &str,
        session: &str,
    ) -> Result<ProviderConfig, String> {
        let mut provider = self.single_provider_config(config, model, session).await?;
        if let Some(summary) = &config.compaction.summary_model {
            let chat_provider = provider_for(&config.providers, model).map(|(p, _)| &p.id);
            let summary_provider = provider_for(&config.providers, summary).map(|(p, _)| &p.id);
            if chat_provider != summary_provider {
                provider.summary = Some(Box::new(
                    self.single_provider_config(config, summary, session)
                        .await?,
                ));
            }
        }
        Ok(provider)
    }

    async fn single_provider_config(
        &self,
        config: &RuntimeConfig,
        model: &str,
        session: &str,
    ) -> Result<ProviderConfig, String> {
        let kind = themis_core::providers::ProviderKind::Go;
        if let Some((provider, _)) = provider_for(&config.providers, model) {
            let secret = provider.secret_name();
            let key = self
                .read_named_secret(secret.to_owned())
                .await?
                .ok_or_else(|| {
                    format!(
                        "no key for {}: set the secret {secret} in Settings › Secrets, then retry",
                        provider.name
                    )
                })?;
            let mut settings = ProviderConfig::new(kind, key).with_model(model);
            settings.endpoint = Some(endpoint(provider));
            return Ok(settings);
        }
        let mut settings = ProviderConfig::new(kind, self.api_key_for(ProviderKind::Go).await?);
        settings.session_id = Some(session.to_owned());
        if !model.trim().is_empty() {
            settings = settings.with_model(model);
        }
        if let Some(url) = self
            .inner
            .go_base_url_override
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .clone()
        {
            settings = settings.with_base_url(url);
        }
        Ok(settings)
    }
}
