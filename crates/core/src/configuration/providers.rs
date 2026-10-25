//! Model providers beyond the built-in OpenCode Go (`providers`) and per-model overrides
//! (`models`). Providers are OpenAI-compatible Chat Completions endpoints defined only in the
//! user `runtime.jsonc`: a project or run override may not add one or point one elsewhere,
//! since each request carries the person's key. Model overrides may come from any layer.

use anyhow::{ensure, Result};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::BTreeMap;

/// The built-in provider's id, reserved (with its alias) so a model qualified as `go/<id>`
/// always means OpenCode Go.
pub const GO_PROVIDER_ID: &str = "go";
const RESERVED_IDS: [&str; 2] = [GO_PROVIDER_ID, "opencode-go"];

/// At most this many providers, models per provider, and model overrides.
pub const MAX_PROVIDERS: usize = 16;
pub const MAX_PROVIDER_MODELS: usize = 512;
pub const MAX_MODEL_OVERRIDES: usize = 256;

/// The reasoning efforts a model override may name; the model must still offer it.
pub const REASONING_EFFORTS: [&str; 7] =
    ["none", "minimal", "low", "medium", "high", "xhigh", "max"];

/// One OpenAI-compatible provider from the user file.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct ProviderDefinition {
    /// Short id used to qualify its models: a chat on `<id>/<model>` uses this provider.
    pub id: String,
    /// Shown in Settings › Models.
    pub name: String,
    /// The API root, such as `https://api.example.com/v1`; requests go to
    /// `{base_url}/chat/completions` and `{base_url}/models`.
    pub base_url: String,
    /// The Settings › Secrets secret holding the key, written `${NAME}`.
    pub secret: String,
    /// The models to offer. Left out, the provider's `/models` list is read instead.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub models: Option<Vec<ProviderModel>>,
}

impl ProviderDefinition {
    /// The secret's name (`NAME` of `${NAME}`); validation guarantees the form.
    pub fn secret_name(&self) -> &str {
        self.secret
            .strip_prefix("${")
            .and_then(|rest| rest.strip_suffix('}'))
            .unwrap_or(&self.secret)
    }

    /// The id a chat stores for one of its models.
    pub fn qualified(&self, model: &str) -> String {
        format!("{}/{model}", self.id)
    }
}

/// A model a provider definition lists, with what Themis cannot learn from `/models`.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct ProviderModel {
    /// The id the provider expects in requests.
    pub id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub context_tokens: Option<usize>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub output_tokens: Option<usize>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub price: Option<ModelPrice>,
}

/// US dollars per million tokens.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct ModelPrice {
    pub input_per_mtok: f64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cached_input_per_mtok: Option<f64>,
    pub output_per_mtok: f64,
}

/// Overrides for one model, keyed by its id or by `<provider>/<id>`.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(default, deny_unknown_fields)]
pub struct ModelOverride {
    /// Caps the model's context window (never raises it).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub context_tokens: Option<usize>,
    /// Caps the model's output tokens per reply (never raises it).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub output_tokens: Option<usize>,
    /// The effort a chat uses when it chose none; the model must offer it.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reasoning_effort: Option<String>,
    /// Thinking on or off, for a model whose catalog lists a thinking switch.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub thinking: Option<bool>,
}

impl ModelOverride {
    /// `self` with `fallback`'s values where `self` sets none.
    fn or(&self, fallback: &ModelOverride) -> ModelOverride {
        ModelOverride {
            context_tokens: self.context_tokens.or(fallback.context_tokens),
            output_tokens: self.output_tokens.or(fallback.output_tokens),
            reasoning_effort: self
                .reasoning_effort
                .clone()
                .or_else(|| fallback.reasoning_effort.clone()),
            thinking: self.thinking.or(fallback.thinking),
        }
    }
}

/// The override for `model` of `provider` (`go` for OpenCode Go): the qualified entry's
/// values first, then the bare id's.
pub fn model_override(
    overrides: &BTreeMap<String, ModelOverride>,
    provider: &str,
    model: &str,
) -> Option<ModelOverride> {
    let qualified = overrides.get(&format!("{provider}/{model}"));
    let bare = overrides.get(model);
    match (qualified, bare) {
        (None, None) => None,
        (Some(one), None) | (None, Some(one)) => Some(one.clone()),
        (Some(qualified), Some(bare)) => Some(qualified.or(bare)),
    }
}

/// The configured provider a stored model id names (`<id>/<model>`), with the provider's own
/// model id; `None` for an OpenCode Go model.
pub fn provider_for<'a, 'm>(
    providers: &'a [ProviderDefinition],
    model: &'m str,
) -> Option<(&'a ProviderDefinition, &'m str)> {
    let (id, rest) = model.split_once('/')?;
    providers
        .iter()
        .find(|provider| provider.id == id)
        .map(|provider| (provider, rest))
}

/// Project and run layers may not add providers or change where one sends requests.
pub(super) fn check_provider_restriction(layer: &Value, origin: &str) -> Result<()> {
    ensure!(
        layer.get("providers").is_none(),
        "providers may only be set in the user runtime.jsonc, not the {origin} configuration"
    );
    Ok(())
}

fn printable(text: &str, max: usize) -> bool {
    !text.trim().is_empty() && text.len() <= max && !text.chars().any(char::is_control)
}

fn valid_id(id: &str) -> bool {
    (1..=32).contains(&id.len())
        && id.starts_with(|c: char| c.is_ascii_lowercase() || c.is_ascii_digit())
        && id
            .chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || matches!(c, '-' | '_'))
}

/// A model id as a provider names it: printable, no spaces, at most 256 bytes.
pub fn valid_model_id(id: &str) -> bool {
    !id.is_empty()
        && id.len() <= 256
        && id
            .chars()
            .all(|c| c.is_ascii_graphic() || (!c.is_ascii() && !c.is_control()))
}

fn validate_base_url(id: &str, url: &str) -> Result<()> {
    let parsed = reqwest::Url::parse(url)
        .ok()
        .filter(|_| url.len() <= 1024)
        .ok_or_else(|| {
            anyhow::anyhow!(
                "providers.{id}.base_url must be an http or https URL of at most 1,024 bytes"
            )
        })?;
    let host = parsed.host_str().unwrap_or_default();
    let loopback = matches!(host, "localhost" | "127.0.0.1" | "[::1]" | "::1");
    ensure!(
        parsed.scheme() == "https" || (parsed.scheme() == "http" && loopback),
        "providers.{id}.base_url must use https (http only for localhost)"
    );
    ensure!(!host.is_empty(), "providers.{id}.base_url must name a host");
    ensure!(
        parsed.username().is_empty() && parsed.password().is_none(),
        "providers.{id}.base_url must not contain credentials"
    );
    ensure!(
        parsed.query().is_none() && parsed.fragment().is_none(),
        "providers.{id}.base_url must not have a query or fragment"
    );
    Ok(())
}

fn validate_tokens(field: &str, value: Option<usize>, low: usize, high: usize) -> Result<()> {
    if let Some(value) = value {
        ensure!(
            (low..=high).contains(&value),
            "{field} must be {low}..{high}"
        );
    }
    Ok(())
}

const CONTEXT_RANGE: (usize, usize) = (1_000, 10_000_000);
const OUTPUT_RANGE: (usize, usize) = (256, 1_000_000);

fn validate_price(field: &str, price: &ModelPrice) -> Result<()> {
    for (name, value) in [
        ("input_per_mtok", Some(price.input_per_mtok)),
        ("cached_input_per_mtok", price.cached_input_per_mtok),
        ("output_per_mtok", Some(price.output_per_mtok)),
    ] {
        if let Some(value) = value {
            ensure!(
                value.is_finite() && (0.0..=10_000.0).contains(&value),
                "{field}.price.{name} must be 0..10000 dollars per million tokens"
            );
        }
    }
    Ok(())
}

pub(super) fn validate(
    providers: &[ProviderDefinition],
    models: &BTreeMap<String, ModelOverride>,
) -> Result<()> {
    ensure!(
        providers.len() <= MAX_PROVIDERS,
        "providers holds at most {MAX_PROVIDERS} providers"
    );
    let mut ids = std::collections::BTreeSet::new();
    for provider in providers {
        let id = provider.id.as_str();
        ensure!(
            valid_id(id),
            "providers: id '{id}' must be 1–32 lowercase letters, digits, - or _"
        );
        ensure!(
            !RESERVED_IDS.contains(&id),
            "providers: id '{id}' is reserved for OpenCode Go"
        );
        ensure!(ids.insert(id), "providers: id '{id}' is used twice");
        ensure!(
            printable(&provider.name, 64),
            "providers.{id}.name must be 1–64 printable characters"
        );
        validate_base_url(id, &provider.base_url)?;
        let secret = crate::plugins::keys::header_template(&provider.secret)
            .ok()
            .filter(|(prefix, _)| prefix.is_empty());
        ensure!(
            secret.is_some(),
            "providers.{id}.secret must name a Settings › Secrets secret as ${{NAME}}; keys never go in this file"
        );
        if let Some(list) = &provider.models {
            ensure!(
                list.len() <= MAX_PROVIDER_MODELS,
                "providers.{id}.models holds at most {MAX_PROVIDER_MODELS} models"
            );
            let mut seen = std::collections::BTreeSet::new();
            for model in list {
                let field = format!("providers.{id}.models.{}", model.id);
                ensure!(
                    valid_model_id(&model.id),
                    "providers.{id}.models: id '{}' must be 1–256 printable characters without spaces",
                    model.id
                );
                ensure!(seen.insert(&model.id), "{field} is listed twice");
                validate_tokens(
                    &format!("{field}.context_tokens"),
                    model.context_tokens,
                    CONTEXT_RANGE.0,
                    CONTEXT_RANGE.1,
                )?;
                validate_tokens(
                    &format!("{field}.output_tokens"),
                    model.output_tokens,
                    OUTPUT_RANGE.0,
                    OUTPUT_RANGE.1,
                )?;
                if let Some(price) = &model.price {
                    validate_price(&field, price)?;
                }
            }
        }
    }
    ensure!(
        models.len() <= MAX_MODEL_OVERRIDES,
        "models holds at most {MAX_MODEL_OVERRIDES} overrides"
    );
    for (key, entry) in models {
        ensure!(
            valid_model_id(key) && key.len() <= 300,
            "models: '{key}' must be a model id, optionally as <provider>/<id>"
        );
        check_model_prefix(providers, "models", key, true)?;
        validate_tokens(
            &format!("models.{key}.context_tokens"),
            entry.context_tokens,
            CONTEXT_RANGE.0,
            CONTEXT_RANGE.1,
        )?;
        validate_tokens(
            &format!("models.{key}.output_tokens"),
            entry.output_tokens,
            OUTPUT_RANGE.0,
            OUTPUT_RANGE.1,
        )?;
        if let Some(effort) = &entry.reasoning_effort {
            ensure!(
                REASONING_EFFORTS.contains(&effort.as_str()),
                "models.{key}.reasoning_effort must be one of {}",
                REASONING_EFFORTS.join(", ")
            );
        }
    }
    Ok(())
}

/// A model id with a `/` names a provider before it: one from `providers` (or, for a
/// `models` key, `go` for OpenCode Go). A bare id is an OpenCode Go model, so a prefix
/// that names no provider is a mistake, not a Go model.
pub(super) fn check_model_prefix(
    providers: &[ProviderDefinition],
    field: &str,
    model: &str,
    go_allowed: bool,
) -> Result<()> {
    let Some((prefix, _)) = model.split_once('/') else {
        return Ok(());
    };
    let known = providers.iter().any(|provider| provider.id == prefix)
        || (go_allowed && prefix == GO_PROVIDER_ID);
    ensure!(
        known,
        "{field}: '{model}' names provider '{prefix}', which is not in providers{}",
        if go_allowed {
            " (use go/<id> or the bare id for OpenCode Go)"
        } else {
            " (use the bare id for OpenCode Go)"
        }
    );
    Ok(())
}

/// The editor schema for `providers` and `models`.
pub(super) fn schema() -> (Value, Value) {
    let tokens = |low: usize, high: usize| serde_json::json!({"type":"integer","minimum":low,"maximum":high});
    let price = serde_json::json!({"type":"object","additionalProperties":false,"required":["input_per_mtok","output_per_mtok"],"properties":{
        "input_per_mtok":{"type":"number","minimum":0,"maximum":10000},
        "cached_input_per_mtok":{"type":"number","minimum":0,"maximum":10000},
        "output_per_mtok":{"type":"number","minimum":0,"maximum":10000}
    }});
    let providers = serde_json::json!({"type":"array","maxItems":MAX_PROVIDERS,"default":[],"description":"OpenAI-compatible providers besides OpenCode Go; user file only","items":{
    "type":"object","additionalProperties":false,"required":["id","name","base_url","secret"],"properties":{
        "id":{"type":"string","pattern":"^[a-z0-9][a-z0-9_-]{0,31}$"},
        "name":{"type":"string","minLength":1,"maxLength":64},
        "base_url":{"type":"string","maxLength":1024,"description":"https URL (http only for localhost)"},
        "secret":{"type":"string","pattern":"^\\$\\{[A-Za-z0-9_]{1,128}\\}$","description":"A Settings › Secrets name as ${NAME}"},
        "models":{"type":"array","maxItems":MAX_PROVIDER_MODELS,"description":"Leave out to read the provider's /models list","items":{
            "type":"object","additionalProperties":false,"required":["id"],"properties":{
                "id":{"type":"string","minLength":1,"maxLength":256},
                "context_tokens":tokens(CONTEXT_RANGE.0, CONTEXT_RANGE.1),
                "output_tokens":tokens(OUTPUT_RANGE.0, OUTPUT_RANGE.1),
                "price":price
            }}}
    }}});
    let models = serde_json::json!({"type":"object","default":{},"maxProperties":MAX_MODEL_OVERRIDES,"description":"Per-model overrides keyed by model id or <provider>/<id>","additionalProperties":{
    "type":"object","additionalProperties":false,"properties":{
        "context_tokens":tokens(CONTEXT_RANGE.0, CONTEXT_RANGE.1),
        "output_tokens":tokens(OUTPUT_RANGE.0, OUTPUT_RANGE.1),
        "reasoning_effort":{"enum":REASONING_EFFORTS},
        "thinking":{"type":"boolean"}
    }}});
    (providers, models)
}

#[cfg(test)]
mod tests {
    use super::super::resolve;
    use super::*;

    const USER: &str = r#"{
        "providers": [{
            "id": "local", "name": "Local server", "base_url": "http://127.0.0.1:8080/v1",
            "secret": "${LOCAL_KEY}",
            "models": [{"id": "qwen/coder", "context_tokens": 64000,
                        "price": {"input_per_mtok": 0.5, "output_per_mtok": 1.5}}]
        }],
        "models": {"glm-5.3": {"context_tokens": 100000, "thinking": false}}
    }"#;

    #[test]
    fn providers_come_from_the_user_file_with_provenance() {
        let resolved = resolve(Some(USER), None, None).unwrap();
        let provider = &resolved.config.providers[0];
        assert_eq!(provider.secret_name(), "LOCAL_KEY");
        assert_eq!(provider.qualified("qwen/coder"), "local/qwen/coder");
        assert_eq!(resolved.provenance["providers"], "user");
        assert_eq!(resolved.provenance["models.glm-5.3.context_tokens"], "user");
        let (found, model) = provider_for(&resolved.config.providers, "local/qwen/coder").unwrap();
        assert_eq!((found.id.as_str(), model), ("local", "qwen/coder"));
        assert!(provider_for(&resolved.config.providers, "glm-5.3").is_none());
        assert!(provider_for(&resolved.config.providers, "other/x").is_none());
    }

    #[test]
    fn projects_and_runs_may_not_add_providers_but_may_override_models() {
        let project = r#"{"providers":[]}"#;
        assert!(resolve(Some(USER), Some(project), None)
            .unwrap_err()
            .to_string()
            .contains("user runtime.jsonc"));
        assert!(resolve(Some(USER), None, Some(project)).is_err());
        let resolved = resolve(
            Some(USER),
            Some(r#"{"models":{"glm-5.3":{"output_tokens":4096}}}"#),
            None,
        )
        .unwrap();
        let entry = &resolved.config.models["glm-5.3"];
        assert_eq!(entry.context_tokens, Some(100000), "fields merge per layer");
        assert_eq!(entry.output_tokens, Some(4096));
        assert_eq!(
            resolved.provenance["models.glm-5.3.output_tokens"],
            "project"
        );
    }

    #[test]
    fn provider_definitions_are_validated() {
        let provider = |fields: &str| {
            format!(
                r#"{{"providers":[{{"id":"p","name":"P","base_url":"https://api.example.com/v1","secret":"${{K}}"{fields}}}]}}"#
            )
        };
        assert!(resolve(Some(&provider("")), None, None).is_ok());
        for bad in [
            provider(r#","extra":1"#),
            provider(r#","models":[{"id":"a"},{"id":"a"}]"#),
            provider(r#","models":[{"id":"has space"}]"#),
            provider(r#","models":[{"id":"a","context_tokens":10}]"#),
            provider(r#","models":[{"id":"a","price":{"input_per_mtok":-1,"output_per_mtok":1}}]"#),
            provider(r#","models":[{"id":"a","price":{"input_per_mtok":1}}]"#),
            r#"{"providers":[{"id":"go","name":"P","base_url":"https://x.dev","secret":"${K}"}]}"#.into(),
            r#"{"providers":[{"id":"Bad","name":"P","base_url":"https://x.dev","secret":"${K}"}]}"#.into(),
            r#"{"providers":[{"id":"p","name":"P","base_url":"http://x.dev","secret":"${K}"}]}"#.into(),
            r#"{"providers":[{"id":"p","name":"P","base_url":"https://u:p@x.dev","secret":"${K}"}]}"#.into(),
            r#"{"providers":[{"id":"p","name":"P","base_url":"https://x.dev?a=1","secret":"${K}"}]}"#.into(),
            r#"{"providers":[{"id":"p","name":"P","base_url":"https://x.dev","secret":"sk-literal"}]}"#.into(),
            r#"{"providers":[{"id":"p","name":"P","base_url":"https://x.dev","secret":"Bearer ${K}"}]}"#.into(),
            r#"{"providers":[{"id":"p","name":"","base_url":"https://x.dev","secret":"${K}"}]}"#.into(),
            r#"{"providers":[{"id":"p","name":"P","base_url":"https://x.dev","secret":"${K}"},{"id":"p","name":"Q","base_url":"https://y.dev","secret":"${K}"}]}"#.into(),
            r#"{"models":{"m":{"reasoning_effort":"huge"}}}"#.into(),
            r#"{"models":{"m":{"output_tokens":1}}}"#.into(),
            r#"{"models":{"m":{"other":1}}}"#.into(),
            r#"{"models":{"":{}}}"#.into(),
        ] {
            assert!(resolve(Some(&bad), None, None).is_err(), "{bad}");
        }
        let error = resolve(
            Some(r#"{"providers":[{"id":"p","name":"P","base_url":"https://x.dev","secret":"sk-live-123"}]}"#),
            None,
            None,
        )
        .unwrap_err()
        .to_string();
        assert!(!error.contains("sk-live-123"), "{error}");
    }

    #[test]
    fn qualified_overrides_win_field_by_field() {
        let overrides: BTreeMap<String, ModelOverride> = serde_json::from_str(
            r#"{"m":{"context_tokens":1000,"thinking":true},"local/m":{"context_tokens":2000}}"#,
        )
        .unwrap();
        let local = model_override(&overrides, "local", "m").unwrap();
        assert_eq!(
            (local.context_tokens, local.thinking),
            (Some(2000), Some(true))
        );
        assert_eq!(
            model_override(&overrides, GO_PROVIDER_ID, "m")
                .unwrap()
                .context_tokens,
            Some(1000)
        );
        assert!(model_override(&overrides, "local", "n").is_none());
    }

    proptest::proptest! {
        /// Any provider block from a file resolves or fails cleanly, and what resolves is
        /// valid: a reserved or repeated id, a literal key or a non-https remote URL never
        /// gets through.
        #[test]
        fn any_providers_block_resolves_or_fails_cleanly(
            id in "[a-zA-Z0-9_-]{0,6}",
            url in "(https?://)?[a-z0-9.:\\[\\]@?#/-]{0,24}",
            secret in "[$]?[{]?[A-Z_a-z0-9 -]{0,8}[}]?",
            model in "\\PC{0,12}",
            context in 0usize..20_000_000,
        ) {
            let file = serde_json::json!({"providers":[{"id":id,"name":"N","base_url":url,"secret":secret,
                "models":[{"id":model,"context_tokens":context}]}]}).to_string();
            if let Ok(resolved) = resolve(Some(&file), None, None) {
                let provider = &resolved.config.providers[0];
                proptest::prop_assert!(valid_id(&provider.id) && !RESERVED_IDS.contains(&provider.id.as_str()));
                proptest::prop_assert!(provider.secret_name().len() + 3 == provider.secret.len());
                proptest::prop_assert!(provider.base_url.starts_with("https://") || provider.base_url.starts_with("http://"));
                let models = provider.models.as_ref().unwrap();
                proptest::prop_assert!(valid_model_id(&models[0].id));
            }
        }
    }
}
