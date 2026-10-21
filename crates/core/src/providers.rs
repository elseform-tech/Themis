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
mod gemini;
mod messages;
pub mod reasoning;
mod responses;
mod wire;

pub use catalog::*;
use gemini::*;
use messages::*;
use responses::*;
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
    provider_name: &'static str,
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

struct ChatEventStream {
    response: reqwest::Response,
    buffer: Vec<u8>,
    ready: std::collections::VecDeque<Result<StreamChunk, LLMError>>,
    calls: std::collections::BTreeMap<usize, ToolCall>,
    reasoning: reasoning::StreamedReasoning,
    messages_api: bool,
    responses_api: bool,
    gemini_api: bool,
    done: bool,
    finished_choice: bool,
    trace: Option<crate::context_metrics::RequestTrace>,
}

impl ChatEventStream {
    fn new(response: reqwest::Response, protocol: Protocol) -> Self {
        Self {
            response,
            buffer: Vec::new(),
            ready: std::collections::VecDeque::new(),
            calls: std::collections::BTreeMap::new(),
            reasoning: reasoning::StreamedReasoning::default(),
            messages_api: protocol == Protocol::Messages,
            responses_api: protocol == Protocol::Responses,
            gemini_api: protocol == Protocol::Gemini,
            done: false,
            finished_choice: false,
            trace: None,
        }
    }

    async fn next_chunk(&mut self) -> Option<Result<StreamChunk, LLMError>> {
        loop {
            if let Some(item) = self.ready.pop_front() {
                return Some(item);
            }
            if self.done {
                return None;
            }
            if let Some(end) = self.buffer.iter().position(|byte| *byte == b'\n') {
                let line = self.buffer.drain(..=end).collect();
                if let Err(error) = self.process_line(line) {
                    self.fail(error);
                }
                continue;
            }
            match self.response.chunk().await {
                Ok(Some(bytes)) => {
                    note_stream_progress();
                    self.buffer.extend_from_slice(&bytes);
                    if self.buffer.len() > 4 * 1024 * 1024 {
                        self.fail(LLMError::Generic(
                            "Provider stream event exceeded size limit".to_owned(),
                        ));
                    }
                }
                Ok(None) if self.finished_choice => {
                    self.done = true;
                    if let Some(trace) = &mut self.trace {
                        trace.complete();
                    }
                }
                Ok(None) => self.fail(LLMError::Generic(
                    "Provider stream ended before completion".to_owned(),
                )),
                Err(error) => self.fail(transport_error("stream interrupted", error)),
            }
        }
    }

    fn process_line(&mut self, line: Vec<u8>) -> Result<(), LLMError> {
        let line = std::str::from_utf8(&line)
            .map_err(|error| LLMError::Generic(format!("Invalid UTF-8 stream: {error}")))?
            .trim();
        let Some(data) = line.strip_prefix("data:").map(str::trim) else {
            return Ok(());
        };
        if data == "[DONE]" {
            if let Some(trace) = &mut self.trace {
                trace.complete();
            }
            self.done = true;
            return Ok(());
        }
        self.process_payload(data)
    }

    fn process_payload(&mut self, data: &str) -> Result<(), LLMError> {
        let value: serde_json::Value = serde_json::from_str(data)
            .map_err(|error| LLMError::Generic(format!("Invalid stream event: {error}")))?;
        if let Some(trace) = &mut self.trace {
            trace.usage(&value["usage"]);
            trace.usage(&value["message"]["usage"]);
            trace.usage(&value["response"]["usage"]);
        }
        if value["type"] == "message_stop" || value["type"] == "response.completed" {
            self.done = true;
            if let Some(trace) = &mut self.trace {
                trace.complete();
            }
        }
        if let Some(error) = value.get("error") {
            return Err(LLMError::Generic(format!("Provider stream error: {error}")));
        }
        if self.responses_api
            && value["type"] == "response.incomplete"
            && value["response"]["incomplete_details"]["reason"] == "max_output_tokens"
        {
            // The same cut-off as a non-streamed reply, so it gets the cut-off retries.
            return Err(LLMError::Generic(
                "Provider response was truncated".to_owned(),
            ));
        }
        if self.responses_api
            && matches!(
                value["type"].as_str(),
                Some("response.failed" | "response.incomplete")
            )
        {
            return Err(LLMError::Generic(
                "Provider response failed or was incomplete".to_owned(),
            ));
        }
        if self.responses_api && value["type"] == "response.function_call_arguments.done" {
            let index = value["output_index"].as_u64().unwrap_or(0) as usize;
            if let Some(call) = self.calls.get_mut(&index) {
                if let Some(args) = value["arguments"]
                    .as_str()
                    .or_else(|| value["item"]["arguments"].as_str())
                {
                    call.function.arguments = args.to_owned();
                }
            }
            return Ok(());
        }
        if self.gemini_api {
            if let Some(trace) = &mut self.trace {
                trace.usage(&gemini_usage(&value["usageMetadata"]));
            }
        }
        let value = if self.gemini_api {
            gemini_stream_event(value, self.calls.len())?
        } else if self.responses_api {
            responses_stream_event(value)
        } else if self.messages_api {
            messages_stream_event(value)
        } else {
            value
        };
        for choice in value["choices"].as_array().into_iter().flatten() {
            self.process_choice(choice)?;
        }
        Ok(())
    }

    fn process_choice(&mut self, choice: &serde_json::Value) -> Result<(), LLMError> {
        let delta = &choice["delta"];
        if let Some(text) = delta["content"].as_str().filter(|text| !text.is_empty()) {
            self.ready.push_back(Ok(StreamChunk::Text(text.to_owned())));
        }
        self.reasoning.take(delta);
        for call in delta["tool_calls"].as_array().into_iter().flatten() {
            self.merge_tool_call(call);
        }
        if let Some(reason) = choice["finish_reason"].as_str() {
            self.finish_choice(reason)?;
        }
        Ok(())
    }

    fn merge_tool_call(&mut self, call: &serde_json::Value) {
        let index = call["index"].as_u64().unwrap_or(0) as usize;
        let entry = self.calls.entry(index).or_insert_with(|| ToolCall {
            id: String::new(),
            call_type: "function".to_owned(),
            function: autoagents::llm::FunctionCall {
                name: String::new(),
                arguments: String::new(),
            },
        });
        if let Some(id) = call["id"].as_str() {
            entry.id.push_str(id);
        }
        if let Some(name) = call["function"]["name"].as_str() {
            entry.function.name.push_str(name);
        }
        if let Some(args) = call["function"]["arguments"].as_str() {
            entry.function.arguments.push_str(args);
        }
    }

    fn finish_choice(&mut self, reason: &str) -> Result<(), LLMError> {
        if reason == "length" {
            return Err(LLMError::Generic(
                "Provider response was truncated".to_owned(),
            ));
        }
        let calls: Vec<ToolCall> = self.calls.values().cloned().collect();
        self.reasoning.finish(&calls);
        for (index, mut tool_call) in std::mem::take(&mut self.calls) {
            if self.messages_api && tool_call.function.arguments.is_empty() {
                tool_call.function.arguments = "{}".to_owned();
            }
            self.ready
                .push_back(Ok(StreamChunk::ToolUseComplete { index, tool_call }));
        }
        self.ready.push_back(Ok(StreamChunk::Done {
            stop_reason: reason.to_owned(),
        }));
        self.finished_choice = true;
        Ok(())
    }

    fn fail(&mut self, error: LLMError) {
        self.ready.push_back(Err(error));
        self.done = true;
    }
}

impl CompatibleProvider {
    /// Builds the provider preconfigured for OpenCode Go.
    ///
    /// `base_url` overrides [`GO_BASE_URL`] (primarily a test hook);
    /// `session_id` overrides the `THEMIS_GO_SESSION` / [`GO_DEFAULT_SESSION`]
    /// fallback chain for this instance.
    pub fn go(
        api_key: String,
        model: String,
        base_url: Option<String>,
        session_id: Option<String>,
    ) -> Self {
        let session = session_id
            .filter(|id| !id.trim().is_empty())
            .unwrap_or_else(go_session_id);
        Self {
            provider_name: GO_PROVIDER_NAME,
            api_key,
            base_override: base_url.is_some(),
            base_url: normalize_base(base_url.as_deref().unwrap_or(GO_BASE_URL)),
            model,
            extra_headers: vec![(GO_SESSION_HEADER.to_owned(), session)],
            client: http_client(),
            reasoning_effort: None,
        }
    }

    /// Adds an extra header sent on every request (e.g. per-thread overrides).
    pub fn with_header(mut self, name: impl Into<String>, value: impl Into<String>) -> Self {
        self.extra_headers.push((name.into(), value.into()));
        self
    }

    /// The configured model ID.
    pub fn model(&self) -> &str {
        &self.model
    }

    /// The normalized base URL (no trailing slash).
    pub fn base_url(&self) -> &str {
        &self.base_url
    }

    /// The wire protocol for this provider's model (see [`catalog::route`]).
    fn protocol(&self) -> Protocol {
        if self.provider_name == GO_PROVIDER_NAME {
            route(&self.model).0
        } else {
            Protocol::ChatCompletions
        }
    }

    fn uses_responses(&self) -> bool {
        self.protocol() == Protocol::Responses
    }

    fn uses_messages(&self) -> bool {
        self.protocol() == Protocol::Messages
    }

    /// Where `protocol` requests go: the explicit base URL, else the model's catalog
    /// endpoint when it speaks `protocol`, else Go's default endpoint for it.
    fn endpoint(&self, protocol: Protocol) -> String {
        if self.base_override {
            return self.base_url.clone();
        }
        match route(&self.model) {
            (routed, endpoint) if routed == protocol => endpoint,
            _ => protocol.default_endpoint(),
        }
    }

    fn chat_request(
        &self,
        messages: &[ChatMessage],
        tools: Option<&[Tool]>,
        stream: bool,
        sampling: Option<&SamplingOverrides>,
    ) -> Result<reqwest::RequestBuilder, LLMError> {
        if self.protocol() == Protocol::Unsupported {
            return Err(LLMError::invalid_request(format!(
                "Model '{}' needs a protocol Themis does not support yet",
                self.model
            )));
        }
        if self.protocol() == Protocol::Gemini {
            return self.gemini_request(messages, tools, stream, sampling);
        }
        let mut body = if self.uses_responses() {
            responses_body(
                &self.model,
                messages,
                tools,
                stream,
                self.reasoning_effort.as_deref(),
            )?
        } else if self.uses_messages() {
            messages_body(&self.model, messages, tools, stream)?
        } else {
            serde_json::to_value(WireChatRequest {
                model: &self.model,
                messages: to_wire_messages(messages)?,
                stream,
                reasoning_effort: self.reasoning_effort.as_deref(),
                max_tokens: None,
                temperature: None,
                tools,
            })?
        };
        if let Ok(limit) = OUTPUT_LIMIT.try_with(|limit| *limit) {
            let key = if self.uses_responses() {
                "max_output_tokens"
            } else {
                "max_tokens"
            };
            body[key] = bounded_output_limit(&self.model, limit).into();
        }
        if stream && !self.uses_messages() && !self.uses_responses() {
            body["stream_options"] = serde_json::json!({"include_usage":true});
        }
        if COMPACTION_REQUEST.try_with(|_| ()).is_ok() {
            if self.uses_responses() {
                body["reasoning"] = serde_json::json!({"effort":"none"});
            } else {
                body["thinking"] = serde_json::json!({"type":"disabled"});
            }
        }
        if let Some(sampling) = sampling {
            if let Some(limit) = sampling.max_tokens {
                let key = if self.uses_responses() {
                    "max_output_tokens"
                } else {
                    "max_tokens"
                };
                body[key] = bounded_output_limit(&self.model, limit).into();
            }
            if let Some(temperature) = sampling.temperature {
                body["temperature"] = temperature.into();
            }
            if let Some(top_p) = sampling.top_p {
                body["top_p"] = top_p.into();
            }
        }
        // Thinking on by default for answers (summaries keep it off, above).
        if COMPACTION_REQUEST.try_with(|_| ()).is_err() {
            let thinking = thinking_default(&self.model);
            if let (None, Some(effort), false) =
                (&self.reasoning_effort, thinking.effort, thinking.switch_on)
            {
                if self.uses_responses() {
                    body["reasoning"] = serde_json::json!({"effort":effort});
                } else if !self.uses_messages() {
                    body["reasoning_effort"] = effort.into();
                }
            }
            if thinking.switch_on && self.model.starts_with("claude-") {
                // Claude rejects a fixed budget: adaptive thinking, steered by effort.
                body["thinking"] = serde_json::json!({"type":"adaptive"});
                if let Some(effort) = self.reasoning_effort.as_deref().or(thinking.effort) {
                    body["output_config"] = serde_json::json!({"effort":effort});
                }
            } else if thinking.switch_on {
                if self.uses_messages() {
                    // Messages thinking needs a budget below the output cap.
                    let cap = body["max_tokens"].as_u64().unwrap_or(16_384);
                    if cap > 2048 {
                        body["thinking"] =
                            serde_json::json!({"type":"enabled","budget_tokens":cap / 2});
                    }
                } else if !self.uses_responses() {
                    body["thinking"] = serde_json::json!({"type":"enabled"});
                }
            }
        }
        let request = if self.uses_responses() {
            self.post(Protocol::Responses, "responses")
        } else if self.uses_messages() {
            self.post(Protocol::Messages, "messages")
                .header("anthropic-version", "2023-06-01")
                .header("x-api-key", &self.api_key)
        } else {
            self.post(Protocol::ChatCompletions, CHAT_COMPLETIONS_PATH)
        };
        Ok(request.json(&body))
    }

    /// POST builder for `{endpoint}/{path}` with auth, Themis user agent, and extras.
    fn post(&self, protocol: Protocol, path: &str) -> reqwest::RequestBuilder {
        let request = self
            .client
            .post(format!("{}/{}", self.endpoint(protocol), path))
            .header("User-Agent", themis_user_agent());
        let mut request = if protocol == Protocol::Gemini {
            request.header("x-goog-api-key", &self.api_key)
        } else {
            request.bearer_auth(&self.api_key)
        };
        for (name, value) in &self.extra_headers {
            request = request.header(name.as_str(), value.as_str());
        }
        request
    }
}

/// Maps a reqwest transport failure to `LLMError` by message.
///
/// (Needed because themis-core's reqwest 0.12 error type differs from the reqwest
/// 0.13 error the SDK's `From` impl targets.)
fn transport_error(context: &str, err: reqwest::Error) -> LLMError {
    let category = if err.is_timeout() {
        "request timed out: "
    } else if err.is_connect() {
        "connection failed: "
    } else {
        ""
    };
    LLMError::HttpError(format!("{category}{context}: {err}"))
}

/// Maps non-2xx responses to typed `LLMError`s (auth / rate-limit / generic).
async fn ensure_success(
    response: reqwest::Response,
    provider_name: &str,
) -> Result<reqwest::Response, LLMError> {
    if response.status().is_success() {
        return Ok(response);
    }
    let status = response.status().as_u16();
    let retry_after = response
        .headers()
        .get(reqwest::header::RETRY_AFTER)
        .and_then(|value| value.to_str().ok())
        .and_then(|value| {
            let value = value.trim();
            value
                .parse::<u64>()
                .ok()
                .map(Duration::from_secs)
                .or_else(|| {
                    httpdate::parse_http_date(value)
                        .ok()?
                        .duration_since(std::time::SystemTime::now())
                        .ok()
                })
        });
    let body = response.text().await.unwrap_or_default();
    let message = format!("{provider_name} request failed with HTTP {status}");
    match status {
        401 | 403 => Err(LLMError::AuthError {
            message,
            status_code: Some(status),
            response_body: Some(body.into()),
        }),
        429 => Err(LLMError::RateLimitError {
            status_code: status,
            message,
            response_body: body.into(),
            retry_after,
            provider_code: None,
        }),
        _ => Err(LLMError::HttpStatusError {
            status_code: status,
            message,
            response_body: body.into(),
            retry_after,
            provider_code: None,
        }),
    }
}

impl CompatibleProvider {
    /// One non-streaming chat request with this provider's model, settings and key.
    async fn chat_once(
        &self,
        messages: &[ChatMessage],
        tools: Option<&[Tool]>,
        sampling: Option<&SamplingOverrides>,
    ) -> Result<Box<dyn ChatResponse>, LLMError> {
        if self.api_key.is_empty() {
            return Err(LLMError::missing_api_key(format!(
                "Missing {} API key",
                self.provider_name
            )));
        }
        let mut trace = crate::context_metrics::RequestTrace::begin(
            &self.model,
            messages,
            tools,
            COMPACTION_REQUEST.try_with(|_| ()).is_ok(),
        );
        trace.output_limit(
            sampling
                .and_then(|s| s.max_tokens)
                .or_else(|| OUTPUT_LIMIT.try_with(|limit| *limit).ok())
                .map(|limit| bounded_output_limit(&self.model, limit)),
        );
        let response = self
            .chat_request(messages, tools, false, sampling)?
            .send()
            .await
            .map_err(|err| transport_error("chat request failed", err))?;
        let response = ensure_success(response, self.provider_name).await?;
        let raw = response
            .text()
            .await
            .map_err(|err| transport_error("failed to read chat response", err))?;
        if let Ok(raw_usage) = serde_json::from_str::<serde_json::Value>(&raw) {
            trace.usage(&raw_usage["usage"]);
            trace.usage(&gemini_usage(&raw_usage["usageMetadata"]));
        }
        let parsed: WireChatResponse = if self.protocol() == Protocol::Gemini {
            gemini_response(serde_json::from_str(&raw)?)?
        } else if self.uses_responses() {
            responses_response(serde_json::from_str(&raw)?)?
        } else if self.uses_messages() {
            messages_response(serde_json::from_str(&raw)?)?
        } else {
            serde_json::from_str(&raw).map_err(|err| LLMError::ResponseFormatError {
                message: format!(
                    "Failed to decode {} API response: {err}",
                    self.provider_name
                ),
                raw_response: raw,
            })?
        };
        let choice =
            parsed
                .choices
                .into_iter()
                .next()
                .ok_or_else(|| LLMError::ResponseFormatError {
                    message: format!("{} returned no choices", self.provider_name),
                    raw_response: String::new(),
                })?;
        let truncated = choice.finish_reason.as_deref() == Some("length");
        if truncated || COMPACTION_REQUEST.try_with(|_| ()).is_ok() {
            let usage = parsed.usage.as_ref();
            crate::diagnostics::emit(
                "provider",
                if truncated {
                    "output_truncated"
                } else {
                    "summary_usage"
                },
                if truncated { "warn" } else { "info" },
                serde_json::json!({
                    "model": self.model,
                    "output_limit_tokens": sampling.and_then(|sampling| sampling.max_tokens),
                    "completion_tokens": usage.map(|usage| usage.completion_tokens),
                    "reasoning_tokens": usage.and_then(|usage| usage.completion_tokens_details.as_ref()).and_then(|details| details.reasoning_tokens),
                    "public_text_chars": choice.message.content.as_ref().map_or(0, |text| text.chars().count()),
                }),
            );
        }
        if truncated {
            return Err(LLMError::Generic("Provider response was truncated".into()));
        }
        trace.complete();
        reasoning::remember(
            choice.message.tool_calls.as_deref().unwrap_or_default(),
            choice.message.content.as_deref().unwrap_or_default(),
            reasoning::from_message(&choice.message),
        );
        Ok(Box::new(CompatibleChatResponse {
            text: choice.message.content,
            tool_calls: choice.message.tool_calls,
            usage: parsed.usage,
        }))
    }
}

/// The provider refused the model itself (gone, unlisted or unpaid), not this one request.
fn summary_model_unavailable(error: &LLMError) -> bool {
    matches!(
        error,
        LLMError::HttpStatusError {
            status_code: 402 | 404 | 410,
            ..
        }
    )
}

#[autoagents::async_trait]
impl ChatProvider for CompatibleProvider {
    async fn chat_with_tools(
        &self,
        messages: &[ChatMessage],
        tools: Option<&[Tool]>,
        json_schema: Option<StructuredOutputFormat>,
    ) -> Result<Box<dyn ChatResponse>, LLMError> {
        self.chat_with_tools_and_sampling(messages, tools, json_schema, None)
            .await
    }

    async fn chat_with_tools_and_sampling(
        &self,
        messages: &[ChatMessage],
        tools: Option<&[Tool]>,
        _json_schema: Option<StructuredOutputFormat>,
        sampling: Option<&SamplingOverrides>,
    ) -> Result<Box<dyn ChatResponse>, LLMError> {
        // ponytail: keep only verified off controls; use the known no-thinking Go
        // summarizer for other models until their disable capability is verified.
        let summary_provider = COMPACTION_REQUEST.try_with(|_| ()).ok().map(|()| Self {
            model: summary_model(&self.model).to_owned(),
            reasoning_effort: None,
            ..self.clone()
        });
        let Some(summary) = summary_provider else {
            return self.chat_once(messages, tools, sampling).await;
        };
        match summary.chat_once(messages, tools, sampling).await {
            // Go can keep listing the routed summarizer while its endpoint is gone. Write the
            // summary with the chat's own model then, as when Go retires the summarizer.
            Err(error)
                if summary.model != self.model
                    && crate::runtime::compaction_config().summary_model.is_none()
                    && summary_model_unavailable(&error) =>
            {
                crate::diagnostics::emit(
                    "provider",
                    "summary_model_unavailable",
                    "warn",
                    serde_json::json!({
                        "model": summary.model,
                        "fallback_model": self.model,
                        "status": error.http_status_code(),
                    }),
                );
                Self {
                    model: self.model.clone(),
                    ..summary
                }
                .chat_once(messages, tools, sampling)
                .await
            }
            result => result,
        }
    }

    async fn chat_stream_with_tools(
        &self,
        messages: &[ChatMessage],
        tools: Option<&[Tool]>,
        _json_schema: Option<StructuredOutputFormat>,
    ) -> Result<Pin<Box<dyn Stream<Item = Result<StreamChunk, LLMError>> + Send>>, LLMError> {
        let mut trace =
            crate::context_metrics::RequestTrace::begin(&self.model, messages, tools, false);
        trace.output_limit(
            OUTPUT_LIMIT
                .try_with(|limit| bounded_output_limit(&self.model, *limit))
                .ok(),
        );
        let protocol = self.protocol();
        let response = self
            .chat_request(messages, tools, true, None)?
            .send()
            .await
            .map_err(|error| transport_error("stream request failed", error))?;
        let response = ensure_success(response, self.provider_name).await?;
        // Some compatible endpoints return a complete JSON response despite stream=true.
        if !response
            .headers()
            .get(reqwest::header::CONTENT_TYPE)
            .and_then(|value| value.to_str().ok())
            .unwrap_or("")
            .contains("text/event-stream")
        {
            let value: serde_json::Value = response
                .json()
                .await
                .map_err(|error| transport_error("invalid chat response", error))?;
            trace.usage(&value["usage"]);
            let parsed: WireChatResponse = if protocol == Protocol::Gemini {
                trace.usage(&gemini_usage(&value["usageMetadata"]));
                gemini_response(value)?
            } else if protocol == Protocol::Responses {
                responses_response(value)?
            } else if protocol == Protocol::Messages {
                messages_response(value)?
            } else {
                serde_json::from_value(value)?
            };
            let choice = parsed
                .choices
                .into_iter()
                .next()
                .ok_or_else(|| LLMError::Generic("Provider returned no choices".to_owned()))?;
            if choice.finish_reason.as_deref() == Some("length") {
                return Err(LLMError::Generic("Provider response was truncated".into()));
            }
            reasoning::remember(
                choice.message.tool_calls.as_deref().unwrap_or_default(),
                choice.message.content.as_deref().unwrap_or_default(),
                reasoning::from_message(&choice.message),
            );
            let mut chunks = Vec::new();
            if let Some(text) = choice.message.content {
                chunks.push(Ok(StreamChunk::Text(text)));
            }
            for (index, tool_call) in choice
                .message
                .tool_calls
                .unwrap_or_default()
                .into_iter()
                .enumerate()
            {
                chunks.push(Ok(StreamChunk::ToolUseComplete { index, tool_call }));
            }
            trace.complete();
            return Ok(Box::pin(stream::iter(chunks)));
        }
        let mut state = ChatEventStream::new(response, protocol);
        state.trace = Some(trace);
        Ok(Box::pin(stream::unfold(state, |mut state| async move {
            let item = state.next_chunk().await?;
            Some((item, state))
        })))
    }

    fn model(&self) -> &str {
        &self.model
    }
}

#[autoagents::async_trait]
impl CompletionProvider for CompatibleProvider {
    async fn complete(
        &self,
        request: &CompletionRequest,
        _json_schema: Option<StructuredOutputFormat>,
    ) -> Result<CompletionResponse, LLMError> {
        // Completions speak Chat Completions only; never send another protocol's model there.
        if matches!(self.protocol(), Protocol::Gemini | Protocol::Unsupported) {
            return Err(LLMError::invalid_request(format!(
                "Model '{}' cannot be used for plain completions",
                self.model
            )));
        }
        if self.api_key.is_empty() {
            return Err(LLMError::missing_api_key(format!(
                "Missing {} API key",
                self.provider_name
            )));
        }
        let message = ChatMessage {
            role: ChatRole::User,
            message_type: MessageType::Text,
            content: request.prompt.clone(),
        };
        let body = WireChatRequest {
            reasoning_effort: self.reasoning_effort.as_deref(),
            model: &self.model,
            messages: to_wire_messages(std::slice::from_ref(&message))?,
            stream: false,
            max_tokens: request.max_tokens,
            temperature: request.temperature,
            tools: None,
        };
        let response = self
            .post(Protocol::ChatCompletions, CHAT_COMPLETIONS_PATH)
            .json(&body)
            .send()
            .await
            .map_err(|err| transport_error("completion request failed", err))?;
        let response = ensure_success(response, self.provider_name).await?;
        let raw = response
            .text()
            .await
            .map_err(|err| transport_error("failed to read completion response", err))?;
        let parsed: WireChatResponse =
            serde_json::from_str(&raw).map_err(|err| LLMError::ResponseFormatError {
                message: format!(
                    "Failed to decode {} API response: {err}",
                    self.provider_name
                ),
                raw_response: raw,
            })?;
        let text = parsed
            .choices
            .into_iter()
            .next()
            .and_then(|choice| choice.message.content)
            .unwrap_or_default();
        Ok(CompletionResponse { text })
    }
}

/// Single entry of an OpenAI `/embeddings` response.
#[derive(Deserialize)]
struct WireEmbeddingData {
    embedding: Vec<f32>,
}

/// OpenAI `/embeddings` response body (subset).
#[derive(Deserialize)]
struct WireEmbeddingResponse {
    data: Vec<WireEmbeddingData>,
}

#[autoagents::async_trait]
impl EmbeddingProvider for CompatibleProvider {
    async fn embed(&self, input: Vec<String>) -> Result<Vec<Vec<f32>>, LLMError> {
        if self.api_key.is_empty() {
            return Err(LLMError::missing_api_key(format!(
                "Missing {} API key",
                self.provider_name
            )));
        }
        let body = serde_json::json!({
            "model": self.model,
            "input": input,
        });
        let response = self
            .post(Protocol::ChatCompletions, "embeddings")
            .json(&body)
            .send()
            .await
            .map_err(|err| transport_error("embedding request failed", err))?;
        let response = ensure_success(response, self.provider_name).await?;
        let raw = response
            .text()
            .await
            .map_err(|err| transport_error("failed to read embedding response", err))?;
        let parsed: WireEmbeddingResponse =
            serde_json::from_str(&raw).map_err(|err| LLMError::ResponseFormatError {
                message: format!(
                    "Failed to decode {} API response: {err}",
                    self.provider_name
                ),
                raw_response: raw,
            })?;
        Ok(parsed
            .data
            .into_iter()
            .map(|entry| entry.embedding)
            .collect())
    }
}

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

/// Resolves the OpenCode Go backend for the agent runtime.
pub async fn resolve(config: &ProviderConfig) -> Result<Arc<dyn LLMProvider>> {
    require_api_key(config)?;
    let model = effective_model(config).unwrap_or_else(|| GO_DEFAULT_MODEL.to_owned());
    let mut provider = CompatibleProvider::go(
        config.api_key.clone(),
        model,
        effective_base_url(config),
        config.session_id.clone(),
    );
    provider.reasoning_effort = config.reasoning_effort.clone();
    Ok(Arc::new(provider))
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
