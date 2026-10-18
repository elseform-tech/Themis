//! The OpenCode Go model catalog: which models exist, their limits and thinking options.

use super::*;

tokio::task_local! { pub(super) static MODEL_LIMITS: HashMap<String, (Option<usize>, Option<usize>)>; }
tokio::task_local! { pub(super) static MODEL_THINKING: HashMap<String, (bool, Vec<String>)>; }

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
                future,
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

pub(crate) fn summary_model(model: &str) -> &str {
    if let Some(configured) = crate::runtime::compaction_config().summary_model {
        configured.as_str()
    } else if model.starts_with("longcat-2.5-") || matches!(model, "gpt-5.6-luna" | "gpt-6-luna") {
        model
    } else {
        "longcat-2.5-preview-free"
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

/// Live Go model with effort choices from the OpenCode Go catalog.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
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
}

#[derive(Deserialize)]
pub(super) struct CatalogRoot {
    #[serde(rename = "opencode-go")]
    go: CatalogProvider,
}

#[derive(Deserialize)]
struct CatalogProvider {
    models: HashMap<String, CatalogModel>,
}

#[derive(Deserialize)]
struct CatalogModel {
    #[serde(default)]
    reasoning_options: Vec<CatalogReasoningOption>,
    #[serde(default)]
    limit: CatalogLimits,
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
            GoModel {
                id,
                effort_levels,
                context_tokens: limits.and_then(|l| l.context),
                output_tokens: limits.and_then(|l| l.output),
                thinking_toggle,
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
        Err(_) => ids
            .into_iter()
            .map(|id| GoModel {
                id,
                effort_levels: Vec::new(),
                context_tokens: None,
                output_tokens: None,
                thinking_toggle: false,
            })
            .collect(),
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
