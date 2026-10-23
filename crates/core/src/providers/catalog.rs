//! The OpenCode Go model catalog: which models exist, how each is called, its limits and
//! thinking options.
//!
//! Go serves its model families over four wire protocols at different endpoints. The
//! public models.dev catalog names each model's protocol (as the AI SDK package that speaks
//! it) and endpoint, so a new model is called correctly without an app update. Names only
//! decide the protocol when the catalog cannot be loaded.

use super::*;

tokio::task_local! { pub(super) static MODEL_LIMITS: HashMap<String, (Option<usize>, Option<usize>)>; }
tokio::task_local! { pub(super) static MODEL_THINKING: HashMap<String, (bool, Vec<String>)>; }
tokio::task_local! { static MODEL_ROUTES: HashMap<String, (Protocol, Option<String>)>; }

/// Root of Go's per-protocol endpoints (docs: opencode.ai/docs/go/#endpoints).
const GO_INFERENCE_ROOT: &str = "https://opencode.ai/inference/go";

/// The wire protocol a Go model is called with.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Protocol {
    /// OpenAI Chat Completions, Go's default.
    #[default]
    ChatCompletions,
    /// OpenAI Responses.
    Responses,
    /// Anthropic Messages.
    Messages,
    /// Google Gemini `generateContent`.
    Gemini,
    /// A protocol Themis does not speak yet; the model cannot be used.
    Unsupported,
}

impl Protocol {
    /// The protocol for a catalog AI SDK package (`None`: the provider's default).
    fn from_package(package: &str) -> Self {
        match package {
            "@ai-sdk/openai-compatible" => Self::ChatCompletions,
            "@ai-sdk/openai" => Self::Responses,
            "@ai-sdk/anthropic" => Self::Messages,
            "@ai-sdk/google" => Self::Gemini,
            _ => Self::Unsupported,
        }
    }

    /// The protocol a model name suggests, used only when the catalog is not loaded.
    pub(crate) fn guess(model: &str) -> Self {
        if ["muse-", "gpt-", "grok-"]
            .iter()
            .any(|prefix| model.starts_with(prefix))
        {
            Self::Responses
        } else if ["qwen", "minimax-", "claude-"]
            .iter()
            .any(|prefix| model.starts_with(prefix))
        {
            Self::Messages
        } else if model.starts_with("gemini-") {
            Self::Gemini
        } else {
            Self::ChatCompletions
        }
    }

    /// Go's documented endpoint for this protocol.
    pub(crate) fn default_endpoint(self) -> String {
        let path = match self {
            Self::ChatCompletions | Self::Responses | Self::Unsupported => "openai/v1",
            Self::Messages => "anthropic/v1",
            Self::Gemini => "google/v1beta",
        };
        format!("{GO_INFERENCE_ROOT}/{path}")
    }
}

/// A catalog endpoint is used only when it is an HTTPS OpenCode URL: the catalog comes from
/// the network, and the request it names carries the API key.
pub(super) fn trusted_endpoint(url: &str) -> Option<String> {
    let url = url.trim().trim_end_matches('/');
    let rest = url.strip_prefix("https://opencode.ai/")?;
    let safe = rest
        .split('/')
        .all(|part| !part.is_empty() && part != ".." && part != ".")
        && rest
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '/' | '-' | '_' | '.'));
    safe.then(|| url.to_owned())
}

/// How `model` is called: its protocol and endpoint from the loaded catalog, else the
/// protocol its name suggests at Go's default endpoint for it.
pub(crate) fn route(model: &str) -> (Protocol, String) {
    let known = MODEL_ROUTES
        .try_with(|routes| routes.get(model).cloned())
        .ok()
        .flatten();
    let protocol = known
        .as_ref()
        .map_or_else(|| Protocol::guess(model), |(protocol, _)| *protocol);
    let endpoint = known
        .and_then(|(_, endpoint)| endpoint)
        .unwrap_or_else(|| protocol.default_endpoint());
    (protocol, endpoint)
}

/// Output tokens a reply may use from the start when the model has room: a thinking model's
/// reasoning plus a whole file in one tool call does not fit a cap sized for the memory budget.
const LONG_REPLY_TOKENS: usize = 32_768;

/// The output cap for answer requests. The memory budget is this app's compaction threshold,
/// not the model's window, so when the catalog shows a window well beyond it, a reply may use
/// up to `LONG_REPLY_TOKENS` (never more than the model's own limit), but at most a third of
/// the budget: a single reply larger than that could never fit a later checkpoint. Otherwise
/// one eighth of the budget, at most 8,192, as before.
pub(crate) fn answer_output_cap(model: &str, budget: usize) -> u32 {
    let base = (budget / 8).clamp(1, 8192);
    MODEL_LIMITS
        .try_with(|limits| limits.get(model).copied())
        .ok()
        .flatten()
        .and_then(|(context, output)| {
            let (context, output) = (context?, output?);
            (context >= budget + LONG_REPLY_TOKENS)
                .then(|| base.max(LONG_REPLY_TOKENS.min(output).min(budget / 3)))
        })
        .unwrap_or(base) as u32
}

pub async fn with_catalog_limits<F: std::future::Future>(
    models: &[GoModel],
    future: F,
) -> F::Output {
    if models.is_empty() {
        return future.await;
    }
    MODEL_LIMITS
        .scope(
            models
                .iter()
                .map(|m| (m.id.clone(), (m.context_tokens, m.output_tokens)))
                .collect(),
            MODEL_THINKING.scope(
                models
                    .iter()
                    .map(|m| (m.id.clone(), (m.thinking_toggle, m.effort_levels.clone())))
                    .collect(),
                MODEL_ROUTES.scope(
                    models
                        .iter()
                        .map(|m| (m.id.clone(), (m.protocol(), m.endpoint.clone())))
                        .collect(),
                    future,
                ),
            ),
        )
        .await
}

/// How a request turns thinking on when the person chose no effort: the catalog's switch
/// when the model has one, else a default effort (`medium`, else `high`). Claude takes
/// adaptive thinking with an effort. Models not in the loaded catalog keep their default.
#[derive(Debug, Default, PartialEq)]
pub(crate) struct ThinkingDefault {
    pub(crate) switch_on: bool,
    pub(crate) effort: Option<&'static str>,
}

pub(crate) fn thinking_default(model: &str) -> ThinkingDefault {
    MODEL_THINKING
        .try_with(|models| {
            let Some((toggle, efforts)) = models.get(model) else {
                return ThinkingDefault::default();
            };
            let effort = ["medium", "high"]
                .into_iter()
                .find(|level| efforts.iter().any(|known| known == level));
            ThinkingDefault {
                switch_on: *toggle,
                effort,
            }
        })
        .unwrap_or_default()
}

/// The lowest reasoning effort a summary may ask of a Responses model: `none` when the catalog
/// lists it or does not describe the model, else its lowest of `minimal` and `low` (some
/// models, such as muse-spark, reject `none`).
pub(crate) fn summary_effort(model: &str) -> &'static str {
    MODEL_THINKING
        .try_with(|models| {
            let Some((_, efforts)) = models.get(model).filter(|(_, efforts)| !efforts.is_empty())
            else {
                return "none";
            };
            ["none", "minimal", "low"]
                .into_iter()
                .find(|level| efforts.iter().any(|known| known == level))
                .unwrap_or("none")
        })
        .unwrap_or("none")
}

/// The no-thinking Go model that writes summaries for models whose thinking cannot be
/// turned off reliably.
const SUMMARY_FALLBACK_MODEL: &str = "longcat-2.5-preview-free";

pub(crate) fn summary_model(model: &str) -> &str {
    // A loaded catalog without the fallback model means Go retired it: summarize with the
    // model itself rather than call a model that no longer exists.
    let fallback_listed = MODEL_ROUTES
        .try_with(|routes| {
            routes
                .get(SUMMARY_FALLBACK_MODEL)
                .is_some_and(|(protocol, _)| *protocol != Protocol::Unsupported)
        })
        .unwrap_or(true);
    if let Some(configured) = crate::runtime::compaction_config().summary_model {
        configured.as_str()
    } else if model.starts_with("longcat-2.5-")
        || matches!(model, "gpt-5.6-luna" | "gpt-6-luna")
        || !fallback_listed
    {
        model
    } else {
        SUMMARY_FALLBACK_MODEL
    }
}

pub(crate) fn summary_context_limit(model: &str) -> Option<usize> {
    MODEL_LIMITS
        .try_with(|limits| {
            limits
                .get(summary_model(model))
                .and_then(|limit| limit.0)
                .unwrap_or(24_000)
        })
        .ok()
}

pub(crate) fn bounded_output_limit(model: &str, limit: u32) -> u32 {
    MODEL_LIMITS
        .try_with(|limits| {
            limits
                .get(model)
                .and_then(|limit| limit.1)
                .map_or(limit, |cap| limit.min(cap.min(u32::MAX as usize) as u32))
        })
        .unwrap_or(limit)
}

/// Single entry of an OpenAI-shaped `/models` response.
#[derive(Deserialize)]
struct WireModelEntry {
    id: String,
}

/// OpenAI-shaped `{data: [{id, ...}]}` models response.
#[derive(Deserialize)]
struct WireModelList {
    data: Vec<WireModelEntry>,
}

/// Live Go model with its protocol and effort choices from the OpenCode Go catalog.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct GoModel {
    pub id: String,
    pub effort_levels: Vec<String>,
    #[serde(default)]
    pub context_tokens: Option<usize>,
    #[serde(default)]
    pub output_tokens: Option<usize>,
    /// The catalog lists a thinking on/off switch for this model.
    #[serde(default)]
    pub thinking_toggle: bool,
    /// The wire protocol the catalog names; `None` when the catalog does not describe the
    /// model, so its name decides.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub protocol: Option<Protocol>,
    /// The catalog's endpoint for the model; `None` uses Go's default for its protocol.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub endpoint: Option<String>,
    /// Why the model cannot be used, when Themis does not speak its protocol.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub unavailable_reason: Option<String>,
}

impl GoModel {
    /// A model known only by its ID.
    pub(super) fn listed(id: String) -> Self {
        Self {
            id,
            ..Self::default()
        }
    }

    /// The protocol the model is called with.
    pub fn protocol(&self) -> Protocol {
        self.protocol.unwrap_or_else(|| Protocol::guess(&self.id))
    }
}

#[derive(Deserialize)]
pub(super) struct CatalogRoot {
    #[serde(rename = "opencode-go")]
    go: CatalogProvider,
}

#[derive(Deserialize)]
struct CatalogProvider {
    /// The provider's default AI SDK package and endpoint.
    #[serde(flatten)]
    access: CatalogAccess,
    models: HashMap<String, CatalogModel>,
}

#[derive(Deserialize)]
struct CatalogModel {
    #[serde(default)]
    reasoning_options: Vec<CatalogReasoningOption>,
    #[serde(default)]
    limit: CatalogLimits,
    /// Per-model overrides of the provider's package and endpoint.
    #[serde(default)]
    provider: CatalogAccess,
}

/// How a catalog entry is reached: the AI SDK package (naming the protocol) and endpoint.
#[derive(Default, Deserialize)]
struct CatalogAccess {
    npm: Option<String>,
    api: Option<String>,
}

#[derive(Default, Deserialize)]
struct CatalogLimits {
    context: Option<usize>,
    output: Option<usize>,
}

#[derive(Deserialize)]
struct CatalogReasoningOption {
    #[serde(rename = "type")]
    kind: String,
    #[serde(default)]
    values: Vec<String>,
}

pub(super) fn join_go_catalog(ids: Vec<String>, catalog: CatalogRoot) -> Vec<GoModel> {
    ids.into_iter()
        .map(|id| {
            let effort_levels = catalog
                .go
                .models
                .get(&id)
                .into_iter()
                .flat_map(|model| &model.reasoning_options)
                .filter(|option| option.kind == "effort")
                .flat_map(|option| option.values.iter().cloned())
                .collect();
            let limits = catalog.go.models.get(&id).map(|model| &model.limit);
            let thinking_toggle = catalog.go.models.get(&id).is_some_and(|model| {
                model
                    .reasoning_options
                    .iter()
                    .any(|option| option.kind == "toggle")
            });
            let access = catalog.go.models.get(&id).map(|model| &model.provider);
            let package = access.and_then(|access| access.npm.as_deref()).or(catalog
                .go
                .access
                .npm
                .as_deref());
            // A model the account lists but the catalog does not describe yet keeps `None`.
            let protocol = catalog
                .go
                .models
                .contains_key(&id)
                .then(|| package.map_or(Protocol::ChatCompletions, Protocol::from_package));
            // The provider's endpoint serves the provider's package: a model that names
            // another package without an endpoint gets Go's default for its protocol.
            let provider_endpoint = catalog
                .go
                .access
                .api
                .as_deref()
                .filter(|_| access.and_then(|access| access.npm.as_deref()).is_none());
            let endpoint = access
                .and_then(|access| access.api.as_deref())
                .or(provider_endpoint)
                .filter(|_| protocol.is_some_and(|known| known != Protocol::Unsupported))
                .and_then(trusted_endpoint);
            let unavailable_reason = (protocol == Some(Protocol::Unsupported)).then(|| {
                format!(
                    "Needs {}, which Themis does not support yet",
                    package.unwrap_or("an unknown protocol")
                )
            });
            GoModel {
                id,
                effort_levels,
                context_tokens: limits.and_then(|l| l.context),
                output_tokens: limits.and_then(|l| l.output),
                thinking_toggle,
                protocol,
                endpoint,
                unavailable_reason,
            }
        })
        .collect()
}

/// Joins account-available Go IDs with OpenCode's public Go capability catalog.
pub async fn refresh_go_catalog(base_url: &str, api_key: &str) -> Result<Vec<GoModel>> {
    let ids = refresh_go_models(base_url, api_key).await?;
    let catalog = async {
        http_client()
            .get("https://models.dev/api.json")
            .header("User-Agent", themis_user_agent())
            .send()
            .await?
            .error_for_status()?
            .json::<CatalogRoot>()
            .await
    }
    .await;
    Ok(match catalog {
        Ok(catalog) => join_go_catalog(ids, catalog),
        Err(_) => ids.into_iter().map(GoModel::listed).collect(),
    })
}

/// Refreshes the Go model table: `GET {base_url}/models`, parsed as OpenAI-shaped
/// `{data: [{id, ...}]}`, returning the model IDs.
///
/// `base_url` is the API root (e.g. [`GO_BASE_URL`]); the Themis user agent is
/// sent. Errors on transport failure, non-2xx status (with status code and a body
/// excerpt), and malformed payloads.
pub async fn refresh_go_models(base_url: &str, api_key: &str) -> Result<Vec<String>> {
    let url = format!("{}/models", base_url.trim_end_matches('/'));
    let response = http_client()
        .get(&url)
        .bearer_auth(api_key)
        .header("User-Agent", themis_user_agent())
        .header(GO_SESSION_HEADER, go_session_id())
        .send()
        .await
        .with_context(|| format!("Go models request to {url} failed"))?;
    let status = response.status();
    let body = response
        .text()
        .await
        .context("failed to read Go models response")?;
    if !status.is_success() {
        let excerpt: String = body.chars().take(500).collect();
        bail!("Go models request failed with HTTP {status}: {excerpt}");
    }
    let parsed: WireModelList = serde_json::from_str(&body)
        .with_context(|| format!("failed to parse Go models response: {body}"))?;
    Ok(parsed.data.into_iter().map(|entry| entry.id).collect())
}

/// What OpenCode Go says about an API key.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "snake_case")]
pub enum GoKeyCheck {
    /// Go accepted the key.
    Works,
    /// Go refused the key (wrong, revoked or expired).
    Rejected,
    /// Go could not be reached or answered with a server error.
    Unavailable,
}

/// Checks `api_key` with Go without generating anything (the model list does not check
/// keys): a chat request with no messages is refused only after the key is checked, with
/// 401 or 403 for a key Go does not accept.
pub async fn check_go_key(base_url: &str, api_key: &str) -> GoKeyCheck {
    let url = format!("{}/{CHAT_COMPLETIONS_PATH}", base_url.trim_end_matches('/'));
    let sent = http_client()
        .post(&url)
        .bearer_auth(api_key)
        .header("User-Agent", themis_user_agent())
        .header(GO_SESSION_HEADER, go_session_id())
        .timeout(std::time::Duration::from_secs(15))
        .json(&serde_json::json!({"model": GO_DEFAULT_MODEL, "messages": []}))
        .send()
        .await;
    match sent.map(|response| response.status()) {
        Ok(status) if matches!(status.as_u16(), 401 | 403) => GoKeyCheck::Rejected,
        Ok(status) if status.is_server_error() => GoKeyCheck::Unavailable,
        Ok(_) => GoKeyCheck::Works,
        Err(_) => GoKeyCheck::Unavailable,
    }
}

#[cfg(test)]
mod key_check_tests {
    use super::*;
    use wiremock::matchers::{header, method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    async fn go_answering(status: u16) -> MockServer {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/chat/completions"))
            .and(header("authorization", "Bearer sk-test"))
            .respond_with(ResponseTemplate::new(status))
            .mount(&server)
            .await;
        server
    }

    #[tokio::test]
    async fn a_key_is_checked_without_generating_anything() {
        for (status, expected) in [
            (400, GoKeyCheck::Works),
            (200, GoKeyCheck::Works),
            (401, GoKeyCheck::Rejected),
            (403, GoKeyCheck::Rejected),
            (502, GoKeyCheck::Unavailable),
        ] {
            let go = go_answering(status).await;
            assert_eq!(
                check_go_key(&go.uri(), "sk-test").await,
                expected,
                "{status}"
            );
            let sent = &go.received_requests().await.unwrap()[0];
            let body: serde_json::Value = serde_json::from_slice(&sent.body).unwrap();
            assert_eq!(
                body["messages"],
                serde_json::json!([]),
                "nothing to generate"
            );
        }
        // Nothing listening: unavailable, not rejected.
        assert_eq!(
            check_go_key("http://127.0.0.1:9", "sk-test").await,
            GoKeyCheck::Unavailable
        );
    }
}
