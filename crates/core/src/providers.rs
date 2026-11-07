//! LLM provider registry (OpenCode Go first).
//!
//! [`ProviderKind`] enumerates the supported backends, [`ProviderConfig`] carries
//! per-thread credentials, and [`resolve`] turns a config into the uniform
//! `Arc<dyn LLMProvider>` handle that `AgentBuilder::llm` (and therefore the
//! ReAct/CodeAct executors) consumes.
//!
//! # Go wiring
//!
//! Go serves its models over four wire protocols: Chat Completions, Responses,
//! Messages and Gemini, each at its own endpoint. The catalog names each model's
//! protocol and endpoint (see `catalog`); all four use [`CompatibleProvider`] with Go
//! session headers.
//!
//! # Session-header spike (verdict)
//!
//! Go wants a stable per-conversation `x-opencode-session` header for routing and
//! prompt caching. Investigated against the autoagents 0.4 source vendored in
//! `~/.cargo/registry`:
//!
//! - `OpenAIProviderConfig::custom_headers()` is a *static* trait hook
//!   (`fn custom_headers() -> Option<Vec<(String, String)>>`, no `&self`), invoked
//!   as `T::custom_headers()` inside each request builder. It cannot vary per
//!   instance or per conversation.
//! - Worse, in autoagents-llm 0.4.0 the whole `OpenAICompatibleProvider` seam is
//!   `pub(crate)` (`providers::openai_compatible`), so external crates cannot use
//!   it at all. There is no per-request header mechanism in the
//!   0.4 public API.
//!
//! Verdict: **no SDK mechanism exists; themis-core implements its own provider.**
//! [`CompatibleProvider`] carries per-instance headers, so per-conversation session
//! IDs are supported *today* by constructing the provider with an explicit session
//! (`CompatibleProvider::go(.., Some(session_id))`). [`resolve`] falls back to the
//! `THEMIS_GO_SESSION` environment variable, then to [`GO_DEFAULT_SESSION`] —
//! functional, with degraded prompt-cache routing when the fallback is shared.
//! Remaining options, in preference order: (1) keep the in-tree provider (done);
//! (2) contribute per-instance headers / a public compatible-provider seam upstream;
//! (3) keep the static env fallback only. Option (1) already covers (3).
//!
//! Note: autoagents-llm 0.4.0 depends on reqwest 0.13 while themis-core uses
//! reqwest 0.12, so HTTP errors are mapped to `LLMError` by message rather than by
//! `From` conversion (both versions coexist in the dependency tree).

use futures_util::{stream, Stream};
mod catalog;
mod chat;
mod gemini;
mod messages;
pub mod reasoning;
mod request;
mod responses;
mod streaming;
mod wire;

pub use catalog::*;
use gemini::*;
use messages::*;
use request::{ensure_success, transport_error};
use responses::*;
#[cfg(test)]
use streaming::streamed_call_index;
use streaming::ChatEventStream;
use wire::*;

use std::collections::HashMap;
use std::fmt;
use std::pin::Pin;
use std::str::FromStr;
use std::sync::Arc;
use std::time::Duration;

use anyhow::{anyhow, bail, Context, Result};
use autoagents::llm::{
    chat::{
        ChatMessage, ChatProvider, ChatResponse, ChatRole, MessageType, SamplingOverrides,
        StreamChunk, StructuredOutputFormat, Tool, Usage,
    },
    completion::{CompletionProvider, CompletionRequest, CompletionResponse},
    embedding::EmbeddingProvider,
    error::LLMError,
    models::ModelsProvider,
    LLMProvider, ToolCall,
};
use reqwest::Client;
use serde::{Deserialize, Serialize};

// The SDK sampling overrides do not carry request purpose; keep this task-local
// so internal summary settings cannot leak into concurrent answer requests.
tokio::task_local! { pub(crate) static COMPACTION_REQUEST: (); }

// Least thinking only (no summary-model swap, unlike COMPACTION_REQUEST).
tokio::task_local! { static LEAST_THINKING: (); }

/// Runs `future` with the least thinking each model allows (thinking off, or its lowest
/// effort), as checkpoint summaries get, but on the model asked for: for short extraction jobs
/// such as the memory pass, where thinking only adds time.
pub async fn with_least_thinking<F: std::future::Future>(future: F) -> F::Output {
    LEAST_THINKING.scope((), future).await
}

/// A summary or a short extraction: thinking as low as the model allows.
pub(crate) fn least_thinking() -> bool {
    COMPACTION_REQUEST.try_with(|_| ()).is_ok() || LEAST_THINKING.try_with(|_| ()).is_ok()
}
// When provider bytes last arrived for the current answer request (see `note_stream_progress`).
tokio::task_local! { pub(crate) static STREAM_PROGRESS: Arc<std::sync::Mutex<tokio::time::Instant>>; }

/// Records that the provider sent data: answer deadlines count only time without any.
pub(crate) fn note_stream_progress() {
    let _ = STREAM_PROGRESS.try_with(|last| {
        *last.lock().unwrap_or_else(|e| e.into_inner()) = tokio::time::Instant::now();
    });
}

/// Canonical provider name reported for OpenCode Go in errors and diagnostics.
///
/// This mirrors the `PROVIDER_NAME = "OpenCode Go"` value the plan assigned to a
/// `GoConfig` type; the SDK trait it was meant to implement is `pub(crate)` in
/// autoagents-llm 0.4.0 (see the module docs), so the value lives here instead.
pub const GO_PROVIDER_NAME: &str = "OpenCode Go";
/// OpenCode Go's id in the usage ledger (its key in the public models.dev catalog).
pub const GO_PROVIDER_ID: &str = "opencode-go";

/// Default Go API root (OpenAI chat-completions wire format).
pub const GO_BASE_URL: &str = "https://opencode.ai/inference/go/v1";

/// Chat-completions endpoint path appended to the base URL.
pub const CHAT_COMPLETIONS_PATH: &str = "chat/completions";

/// Default Go model when no explicit model or environment override is set.
pub const GO_DEFAULT_MODEL: &str = "muse-spark-1.3-contributor";

/// Environment variable overriding the Go model when [`ProviderConfig::model`] is `None`.
pub const GO_MODEL_ENV_VAR: &str = "THEMIS_GO_MODEL";

/// Environment variable carrying the Go per-conversation session ID.
pub const GO_SESSION_ENV_VAR: &str = "THEMIS_GO_SESSION";

/// Session header Go uses for routing and prompt caching.
pub const GO_SESSION_HEADER: &str = "x-opencode-session";

/// Fallback session ID when `THEMIS_GO_SESSION` is unset.
///
/// Requests stay functional, but conversations sharing this fallback do not get
/// per-conversation prompt-cache routing. Prefer per-thread IDs via
/// [`CompatibleProvider::go`] with an explicit session.
pub const GO_DEFAULT_SESSION: &str = "themis-default-session";

/// The `themis/<crate version>` user agent Go asks clients to send.
pub fn themis_user_agent() -> String {
    format!("themis/{}", env!("CARGO_PKG_VERSION"))
}

/// Supported LLM backend.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ProviderKind {
    /// OpenCode Go (OpenAI-compatible chat-completions endpoint).
    Go,
}

impl ProviderKind {
    /// Canonical lowercase name.
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Go => "go",
        }
    }

    /// Environment variable used as the model fallback for this kind.
    const fn model_env_var(self) -> &'static str {
        match self {
            Self::Go => GO_MODEL_ENV_VAR,
        }
    }
}

impl FromStr for ProviderKind {
    type Err = anyhow::Error;

    /// Parses a provider kind name, case-insensitively.
    ///
    /// Accepts the canonical [`ProviderKind::as_str`] names plus the `opencode-go`
    /// alias for [`ProviderKind::Go`].
    fn from_str(s: &str) -> Result<Self> {
        match s.trim().to_lowercase().as_str() {
            "go" | "opencode-go" => Ok(Self::Go),
            other => Err(anyhow!("unknown provider kind: {other}")),
        }
    }
}

impl fmt::Display for ProviderKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

/// Credentials and endpoint selection for one provider.
///
/// `base_url` overrides the kind's default endpoint; it is required for
/// a test hook. `model`
/// overrides the kind's model default (see [`GO_DEFAULT_MODEL`]).
#[derive(Debug, Clone)]
pub struct ProviderConfig {
    /// Which backend to resolve.
    pub kind: ProviderKind,
    /// OpenCode Go API key.
    pub api_key: String,
    /// Explicit model ID; wins over the `THEMIS_*_MODEL` environment fallback.
    pub model: Option<String>,
    /// Base-URL override for tests.
    pub base_url: Option<String>,
    /// Optional reasoning effort for supported reasoning models.
    pub reasoning_effort: Option<String>,
    /// Stable OpenCode routing session for this conversation.
    pub session_id: Option<String>,
    /// A provider from `providers` in runtime.jsonc instead of OpenCode Go; the model is
    /// then `<provider id>/<its model id>`.
    pub endpoint: Option<CustomEndpoint>,
    /// The provider that writes summaries when `compaction.summary_model` is on another
    /// provider than the chat's model (it needs that provider's key).
    pub summary: Option<Box<ProviderConfig>>,
}

/// An OpenAI-compatible provider from `providers` in runtime.jsonc.
#[derive(Debug, Clone)]
pub struct CustomEndpoint {
    /// The provider's `id`; models are qualified as `<id>/<model>`.
    pub id: String,
    /// Its display name, used in errors.
    pub name: String,
    /// The API root; Chat Completions requests go to `{base_url}/chat/completions`.
    pub base_url: String,
}

impl ProviderConfig {
    /// Creates a config with no model or base-URL override.
    pub fn new(kind: ProviderKind, api_key: impl Into<String>) -> Self {
        Self {
            kind,
            api_key: api_key.into(),
            model: None,
            base_url: None,
            reasoning_effort: None,
            session_id: None,
            endpoint: None,
            summary: None,
        }
    }

    /// Sets the explicit model ID.
    pub fn with_model(mut self, model: impl Into<String>) -> Self {
        self.model = Some(model.into());
        self
    }

    /// Sets the base-URL override.
    pub fn with_base_url(mut self, base_url: impl Into<String>) -> Self {
        self.base_url = Some(base_url.into());
        self
    }
}

/// HTTP client for provider requests, including streamed answers and summaries (five minutes).
fn http_client() -> Client {
    http_client_with(Duration::from_secs(300))
}

/// No total timeout: a model streaming a long reply keeps its connection as long as data
/// flows. `read` bounds silence between reads (the answer deadlines bound the rest), and
/// connecting has its own short limit.
fn http_client_with(read: Duration) -> Client {
    Client::builder()
        .connect_timeout(Duration::from_secs(30))
        .read_timeout(read)
        .build()
        .unwrap_or_else(|_| Client::new())
}

/// Strips trailing slashes so endpoint paths can be joined with `/`.
fn normalize_base(base_url: &str) -> String {
    base_url.trim_end_matches('/').to_owned()
}

/// Explicit [`ProviderConfig::model`] wins; otherwise the kind's `THEMIS_*_MODEL`
/// environment variable; otherwise `None` (caller applies its default or errors).
fn effective_model(config: &ProviderConfig) -> Option<String> {
    if let Some(model) = config.model.clone().filter(|m| !m.trim().is_empty()) {
        return Some(model);
    }
    std::env::var(config.kind.model_env_var())
        .ok()
        .filter(|m| !m.trim().is_empty())
}

/// Non-empty override or `None`.
fn effective_base_url(config: &ProviderConfig) -> Option<String> {
    config
        .base_url
        .clone()
        .filter(|u| !u.trim().is_empty())
        .map(|u| normalize_base(&u))
}

/// `THEMIS_GO_SESSION` or the documented shared fallback.
fn go_session_id() -> String {
    std::env::var(GO_SESSION_ENV_VAR)
        .ok()
        .filter(|s| !s.trim().is_empty())
        .unwrap_or_else(|| GO_DEFAULT_SESSION.to_owned())
}

/// OpenAI chat-completions wire-format provider.
///
/// This is the themis-core stand-in for the SDK's `pub(crate)`
/// `OpenAICompatibleProvider` seam: a minimal `LLMProvider` speaking
/// `{base}/chat/completions`, `{base}/embeddings`, and `{base}/models` with
/// per-instance headers. [`CompatibleProvider::go`] preconfigures it for OpenCode
/// Go.
///
/// Per-conversation Go session IDs (see the module-level spike note) are supported
/// by passing an explicit `session_id`; `None` falls back to `THEMIS_GO_SESSION`
/// and then [`GO_DEFAULT_SESSION`].
#[derive(Clone)]
pub struct CompatibleProvider {
    provider_name: Arc<str>,
    /// `Some(id)` for a provider from `providers`: Chat Completions only, the model sent
    /// without its `<id>/` prefix, and no OpenCode-specific fields or headers.
    custom: Option<String>,
    /// Writes summaries when the configured summary model is on another provider.
    summary_provider: Option<Arc<CompatibleProvider>>,
    api_key: String,
    base_url: String,
    /// `base_url` was set explicitly (a test server): every protocol is served from it.
    base_override: bool,
    model: String,
    reasoning_effort: Option<String>,
    extra_headers: Vec<(String, String)>,
    client: Client,
}

tokio::task_local! { pub static OUTPUT_LIMIT: u32; }

// `list_models` intentionally uses the SDK default ("not supported"): model
// listing is served by [`refresh_go_models`], and a custom `ModelListResponse`
// cannot be built without a chrono dependency for its entry timestamps.
#[autoagents::async_trait]
impl ModelsProvider for CompatibleProvider {}

impl LLMProvider for CompatibleProvider {}

/// Rejects empty API keys for backends that require one.
fn require_api_key(config: &ProviderConfig) -> Result<()> {
    if config.api_key.trim().is_empty() {
        bail!("{} requires a non-empty API key", config.kind.as_str());
    }
    Ok(())
}

/// Resolves the provider for the agent runtime: OpenCode Go, or a provider from
/// `providers` when the config names its endpoint.
pub async fn resolve(config: &ProviderConfig) -> Result<Arc<dyn LLMProvider>> {
    Ok(Arc::new(compatible(config)?))
}

/// One request with no tools (Themis's own background requests: the memory pass, goal
/// seeding): the reply's text and the provider's (prompt, completion) token counts when it
/// reported them.
pub async fn ask_once(
    config: &ProviderConfig,
    system: &str,
    user: &str,
) -> Result<(String, Option<(u32, u32)>)> {
    use autoagents::llm::chat::{ChatMessage, ChatRole, MessageType};
    let llm = resolve(config).await?;
    let messages = [
        ChatMessage {
            role: ChatRole::System,
            message_type: MessageType::Text,
            content: system.to_owned(),
        },
        ChatMessage {
            role: ChatRole::User,
            message_type: MessageType::Text,
            content: user.to_owned(),
        },
    ];
    let reply = llm.chat(&messages, None).await?;
    let usage = reply
        .usage()
        .map(|usage| (usage.prompt_tokens, usage.completion_tokens));
    Ok((reply.text().unwrap_or_default(), usage))
}

/// Whether a failed request was refused for its rate (HTTP 429), anywhere in its error chain.
pub fn is_rate_limited(error: &anyhow::Error) -> bool {
    error.chain().any(|cause| {
        matches!(
            cause.downcast_ref::<LLMError>(),
            Some(LLMError::RateLimitError { .. })
        )
    })
}

fn compatible(config: &ProviderConfig) -> Result<CompatibleProvider> {
    require_api_key(config)?;
    let model = effective_model(config).unwrap_or_else(|| GO_DEFAULT_MODEL.to_owned());
    let mut provider = match &config.endpoint {
        Some(endpoint) => CompatibleProvider::custom(endpoint, config.api_key.clone(), model),
        None => CompatibleProvider::go(
            config.api_key.clone(),
            model,
            effective_base_url(config),
            config.session_id.clone(),
        ),
    };
    provider.reasoning_effort = config.reasoning_effort.clone();
    if let Some(summary) = &config.summary {
        provider.summary_provider = Some(Arc::new(compatible(summary)?));
    }
    Ok(provider)
}

/// Entry points for the `fuzz/` cargo-fuzz targets; not a stable API.
#[cfg(feature = "fuzzing")]
#[doc(hidden)]
pub mod fuzzing {
    use super::*;

    /// Request bodies (one per wire protocol) for a history whose tool call
    /// carries `raw` model arguments.
    pub fn outgoing_bodies(raw: &str) -> Vec<serde_json::Value> {
        let call = |arguments: &str| autoagents::llm::ToolCall {
            id: "fuzz".into(),
            call_type: "function".into(),
            function: autoagents::llm::FunctionCall {
                name: "shell".into(),
                arguments: arguments.into(),
            },
        };
        let messages = [
            ChatMessage {
                role: ChatRole::Assistant,
                message_type: MessageType::ToolUse(vec![call(raw)]),
                content: String::new(),
            },
            ChatMessage {
                role: ChatRole::Tool,
                message_type: MessageType::ToolResult(vec![call("result")]),
                content: String::new(),
            },
        ];
        [
            "mimo-v2.6-flash",
            "muse-spark-1.3-contributor",
            "claude-haiku-4-5",
            "gemini-4-argon-contributor",
        ]
        .into_iter()
        .map(|model| {
            let provider = CompatibleProvider::go("fuzz".into(), model.into(), None, None);
            let request = provider
                .chat_request(&messages, None, true, None)
                .expect("request")
                .build()
                .expect("build");
            serde_json::from_slice(
                request
                    .body()
                    .and_then(|body| body.as_bytes())
                    .unwrap_or_default(),
            )
            .expect("request body is JSON")
        })
        .collect()
    }

    /// Feeds one provider JSON value through every response and stream converter.
    pub fn convert_provider_json(value: serde_json::Value) {
        let _ = responses::responses_response(value.clone());
        let _ = messages::messages_response(value.clone());
        let _ = gemini::gemini_response(value.clone());
        let _ = gemini::gemini_usage(&value);
        let mut kept = reasoning::StreamedReasoning::default();
        for event in [
            responses::responses_stream_event(value.clone()),
            messages::messages_stream_event(value.clone()),
            gemini::gemini_stream_event(value.clone(), 0).unwrap_or_default(),
            value,
        ] {
            for choice in event["choices"].as_array().into_iter().flatten() {
                kept.take(&choice["delta"]);
            }
        }
    }
}

#[cfg(test)]
// The env-serializing mutex is deliberately held across awaits: process env is
// global, so the guard must span the whole test. Test-only, single lock, no
// deadlock risk.
#[allow(clippy::await_holding_lock)]
mod tests;
