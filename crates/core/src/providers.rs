//! LLM provider registry (OpenCode Go first).
//!
//! [`ProviderKind`] enumerates the supported backends, [`ProviderConfig`] carries
//! per-thread credentials, and [`resolve`] turns a config into the uniform
//! `Arc<dyn LLMProvider>` handle that `AgentBuilder::llm` (and therefore the
//! ReAct/CodeAct executors) consumes.
//!
//! # Go wiring
//!
//! Go routes GPT, Grok, and Muse through Responses; Qwen and MiniMax use
//! Messages; other catalog models use Chat Completions. All three protocols
//! use [`CompatibleProvider`] with Go session headers.
//! Custom endpoints remain Chat Completions; direct providers use named SDK backends.
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
//!   it at all. The named `OpenAI`/`Ollama` builders expose no header options and
//!   no injectable HTTP client. There is no per-request header mechanism in the
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
mod wire;

use wire::*;

use std::fmt;
use std::pin::Pin;
use std::str::FromStr;
use std::sync::Arc;
use std::time::Duration;

use anyhow::{anyhow, bail, Context, Result};
use autoagents::llm::{
    backends::{
        anthropic::Anthropic,
        ollama::Ollama,
        openai::{OpenAI, OpenAIApiMode},
    },
    builder::LLMBuilder,
    chat::{
        ChatMessage, ChatProvider, ChatResponse, ChatRole, MessageType, ReasoningEffort,
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

/// Canonical provider name reported for OpenCode Go in errors and diagnostics.
///
/// This mirrors the `PROVIDER_NAME = "OpenCode Go"` value the plan assigned to a
/// `GoConfig` type; the SDK trait it was meant to implement is `pub(crate)` in
/// autoagents-llm 0.4.0 (see the module docs), so the value lives here instead.
pub const GO_PROVIDER_NAME: &str = "OpenCode Go";

/// Default Go API root (OpenAI chat-completions wire format).
pub const GO_BASE_URL: &str = "https://opencode.ai/zen/go/v1";

/// Chat-completions endpoint path appended to the base URL.
pub const CHAT_COMPLETIONS_PATH: &str = "chat/completions";

/// Documented last-resort Go model.
///
/// [`resolve`] prefers, in order: the explicit [`ProviderConfig::model`], the
/// `THEMIS_GO_MODEL` environment variable, and only then this fallback, which
/// exists so resolution is total. Prefer real IDs from settings or
/// [`refresh_go_models`]; never treat this constant as authoritative.
pub const GO_DEFAULT_MODEL: &str = "go-default";

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

/// Supported LLM backends, in product-priority order (Go first).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ProviderKind {
    /// OpenCode Go (OpenAI-compatible chat-completions endpoint).
    Go,
    /// Direct OpenAI API.
    OpenAI,
    /// Anthropic API via the SDK's named backend.
    Anthropic,
    /// Local Ollama server.
    Ollama,
    /// Any OpenAI-compatible chat-completions endpoint.
    Custom,
}

impl ProviderKind {
    /// Canonical lowercase name (`go`, `openai`, `anthropic`, `ollama`, `custom`).
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Go => "go",
            Self::OpenAI => "openai",
            Self::Anthropic => "anthropic",
            Self::Ollama => "ollama",
            Self::Custom => "custom",
        }
    }

    /// Environment variable used as the model fallback for this kind.
    const fn model_env_var(self) -> &'static str {
        match self {
            Self::Go => GO_MODEL_ENV_VAR,
            Self::OpenAI => "THEMIS_OPENAI_MODEL",
            Self::Anthropic => "THEMIS_ANTHROPIC_MODEL",
            Self::Ollama => "THEMIS_OLLAMA_MODEL",
            Self::Custom => "THEMIS_CUSTOM_MODEL",
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
            "openai" => Ok(Self::OpenAI),
            "anthropic" => Ok(Self::Anthropic),
            "ollama" => Ok(Self::Ollama),
            "custom" => Ok(Self::Custom),
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
/// [`ProviderKind::Custom`] and primarily a test hook otherwise. `model`
/// overrides the kind's model default (see [`GO_DEFAULT_MODEL`]).
#[derive(Debug, Clone)]
pub struct ProviderConfig {
    /// Which backend to resolve.
    pub kind: ProviderKind,
    /// API key (BYOK). May be empty only for [`ProviderKind::Ollama`].
    pub api_key: String,
    /// Explicit model ID; wins over the `THEMIS_*_MODEL` environment fallback.
    pub model: Option<String>,
    /// Base-URL override (required for [`ProviderKind::Custom`]).
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

/// HTTP client for one provider (120s default timeout, matching the SDK default).
fn http_client() -> Client {
    Client::builder()
        .timeout(Duration::from_secs(120))
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
/// Go; [`CompatibleProvider::custom`] targets any compatible endpoint.
///
/// Per-conversation Go session IDs (see the module-level spike note) are supported
/// by passing an explicit `session_id`; `None` falls back to `THEMIS_GO_SESSION`
/// and then [`GO_DEFAULT_SESSION`].
pub struct CompatibleProvider {
    provider_name: &'static str,
    api_key: String,
    base_url: String,
    model: String,
    reasoning_effort: Option<String>,
    extra_headers: Vec<(String, String)>,
    client: Client,
}

struct ChatEventStream {
    response: reqwest::Response,
    buffer: Vec<u8>,
    ready: std::collections::VecDeque<Result<StreamChunk, LLMError>>,
    calls: std::collections::BTreeMap<usize, ToolCall>,
    messages_api: bool,
    responses_api: bool,
    done: bool,
}

impl ChatEventStream {
    fn new(response: reqwest::Response, messages_api: bool, responses_api: bool) -> Self {
        Self {
            response,
            buffer: Vec::new(),
            ready: std::collections::VecDeque::new(),
            calls: std::collections::BTreeMap::new(),
            messages_api,
            responses_api,
            done: false,
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
                    self.buffer.extend_from_slice(&bytes);
                    if self.buffer.len() > 4 * 1024 * 1024 {
                        self.fail(LLMError::Generic(
                            "Provider stream event exceeded size limit".to_owned(),
                        ));
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
            self.done = true;
            return Ok(());
        }
        self.process_payload(data)
    }

    fn process_payload(&mut self, data: &str) -> Result<(), LLMError> {
        let value: serde_json::Value = serde_json::from_str(data)
            .map_err(|error| LLMError::Generic(format!("Invalid stream event: {error}")))?;
        if let Some(error) = value.get("error") {
            return Err(LLMError::Generic(format!("Provider stream error: {error}")));
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
        let value = if self.responses_api {
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
        self.done = true;
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
            base_url: normalize_base(base_url.as_deref().unwrap_or(GO_BASE_URL)),
            model,
            extra_headers: vec![(GO_SESSION_HEADER.to_owned(), session)],
            client: http_client(),
            reasoning_effort: None,
        }
    }

    /// Builds the provider for a custom OpenAI-compatible endpoint.
    pub fn custom(api_key: String, model: String, base_url: String) -> Self {
        Self {
            provider_name: "Custom",
            api_key,
            base_url: normalize_base(&base_url),
            model,
            extra_headers: Vec::new(),
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

    fn uses_responses(&self) -> bool {
        self.provider_name == GO_PROVIDER_NAME && go_chat_path(&self.model) == "responses"
    }

    fn uses_messages(&self) -> bool {
        self.provider_name == GO_PROVIDER_NAME && go_chat_path(&self.model) == "messages"
    }

    fn chat_request(
        &self,
        messages: &[ChatMessage],
        tools: Option<&[Tool]>,
        stream: bool,
    ) -> Result<reqwest::RequestBuilder, LLMError> {
        if self.uses_responses() {
            let mut input = Vec::new();
            for message in to_wire_messages(messages)? {
                if let Some(id) = message.tool_call_id {
                    input.push(serde_json::json!({"type":"function_call_output", "call_id":id, "output":message.content.unwrap_or_default()}));
                } else {
                    if let Some(content) = message.content {
                        input.push(serde_json::json!({"role":message.role, "content":content}));
                    }
                    for call in message.tool_calls.unwrap_or_default() {
                        input.push(serde_json::json!({"type":"function_call", "call_id":call.id, "name":call.function.name, "arguments":call.function.arguments}));
                    }
                }
            }
            let mut body =
                serde_json::json!({"model":self.model,"input":input,"stream":stream,"store":false});
            if let Some(tools) = tools {
                body["tools"] = tools.iter().map(|tool| serde_json::json!({"type":"function","name":tool.function.name,"description":tool.function.description,"parameters":tool.function.parameters})).collect();
            }
            if let Some(effort) = &self.reasoning_effort {
                body["reasoning"] = serde_json::json!({"effort":effort});
            }
            return Ok(self.post("responses").json(&body));
        }
        if self.uses_messages() {
            return Ok(self
                .post("messages")
                .header("anthropic-version", "2023-06-01")
                .header("x-api-key", &self.api_key)
                .json(&messages_body(&self.model, messages, tools, stream)?));
        }
        Ok(self.post(CHAT_COMPLETIONS_PATH).json(&WireChatRequest {
            model: &self.model,
            messages: to_wire_messages(messages)?,
            stream,
            reasoning_effort: self.reasoning_effort.as_deref(),
            max_tokens: None,
            temperature: None,
            tools,
        }))
    }

    /// POST builder for `{base}/{path}` with auth, Themis user agent, and extras.
    fn post(&self, path: &str) -> reqwest::RequestBuilder {
        let mut request = self
            .client
            .post(format!("{}/{}", self.base_url, path))
            .bearer_auth(&self.api_key)
            .header("User-Agent", themis_user_agent());
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
    LLMError::HttpError(format!("{context}: {err}"))
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
            retry_after: None,
            provider_code: None,
        }),
        _ => Err(LLMError::HttpStatusError {
            status_code: status,
            message,
            response_body: body.into(),
            retry_after: None,
            provider_code: None,
        }),
    }
}

#[autoagents::async_trait]
impl ChatProvider for CompatibleProvider {
    async fn chat_with_tools(
        &self,
        messages: &[ChatMessage],
        tools: Option<&[Tool]>,
        _json_schema: Option<StructuredOutputFormat>,
    ) -> Result<Box<dyn ChatResponse>, LLMError> {
        if self.api_key.is_empty() {
            return Err(LLMError::missing_api_key(format!(
                "Missing {} API key",
                self.provider_name
            )));
        }
        let response = self
            .chat_request(messages, tools, false)?
            .send()
            .await
            .map_err(|err| transport_error("chat request failed", err))?;
        let response = ensure_success(response, self.provider_name).await?;
        let raw = response
            .text()
            .await
            .map_err(|err| transport_error("failed to read chat response", err))?;
        let parsed: WireChatResponse = if self.uses_responses() {
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
        Ok(Box::new(CompatibleChatResponse {
            text: choice.message.content,
            tool_calls: choice.message.tool_calls,
            usage: parsed.usage,
        }))
    }

    async fn chat_stream_with_tools(
        &self,
        messages: &[ChatMessage],
        tools: Option<&[Tool]>,
        _json_schema: Option<StructuredOutputFormat>,
    ) -> Result<Pin<Box<dyn Stream<Item = Result<StreamChunk, LLMError>> + Send>>, LLMError> {
        let messages_api = self.uses_messages();
        let responses_api = self.uses_responses();
        let response = self
            .chat_request(messages, tools, true)?
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
            let value = response
                .json()
                .await
                .map_err(|error| transport_error("invalid chat response", error))?;
            let parsed: WireChatResponse = if responses_api {
                responses_response(value)?
            } else if messages_api {
                messages_response(value)?
            } else {
                serde_json::from_value(value)?
            };
            let choice = parsed
                .choices
                .into_iter()
                .next()
                .ok_or_else(|| LLMError::Generic("Provider returned no choices".to_owned()))?;
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
            return Ok(Box::pin(stream::iter(chunks)));
        }
        let state = ChatEventStream::new(response, messages_api, responses_api);
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
            .post(CHAT_COMPLETIONS_PATH)
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
            .post("embeddings")
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

fn configure_model_and_base_url<P: LLMProvider + autoagents::llm::HasConfig>(
    mut builder: LLMBuilder<P>,
    config: &ProviderConfig,
) -> LLMBuilder<P> {
    if let Some(model) = effective_model(config) {
        builder = builder.model(model);
    }
    if let Some(base_url) = effective_base_url(config) {
        builder = builder.base_url(base_url);
    }
    builder
}

/// Resolves a [`ProviderConfig`] into the uniform agent-consumable LLM handle.
///
/// The returned `Arc<dyn LLMProvider>` is exactly the type `AgentBuilder::llm`
/// accepts, so every backend below plugs into ReAct/CodeAct agents unchanged —
/// no per-provider enum is needed because all SDK supertraits (`ChatProvider`,
/// `CompletionProvider`, `EmbeddingProvider`, `ModelsProvider`) are object-safe.
///
/// Backend mapping:
///
/// - [`ProviderKind::Go`] → [`CompatibleProvider::go`] (Go base URL by default).
/// - [`ProviderKind::OpenAI`] → the SDK's named `OpenAI` backend in
///   chat-completions mode (same wire shape as Go, so mocks are interchangeable).
/// - [`ProviderKind::Anthropic`] → the SDK's named `Anthropic` backend
///   (requires the `anthropic` autoagents feature, enabled in this build).
///   exist in this build (see the report's dependency needs).
/// - [`ProviderKind::Ollama`] → the SDK's named `Ollama` backend (local default
///   URL; API key optional).
/// - [`ProviderKind::Custom`] → [`CompatibleProvider::custom`] (base URL and
///   model required).
///
/// Model precedence is explicit config, then the kind's `THEMIS_*_MODEL`
/// environment variable, then the kind default. Resolution is pure construction
/// and performs no network I/O.
#[allow(clippy::unused_async)]
pub async fn resolve(config: &ProviderConfig) -> Result<Arc<dyn LLMProvider>> {
    let effort = match config.reasoning_effort.as_deref() {
        None => None,
        Some(value) => {
            let model = effective_model(config).unwrap_or_default();
            if !matches!(config.kind, ProviderKind::Go | ProviderKind::OpenAI)
                || !(model.starts_with("gpt-5")
                    || model.starts_with("gpt-6")
                    || model.starts_with("o3")
                    || model.starts_with("o4"))
            {
                bail!("This model manages its own reasoning; choose default effort");
            }
            Some(match value {
                "low" => ReasoningEffort::Low,
                "medium" => ReasoningEffort::Medium,
                "high" => ReasoningEffort::High,
                _ => bail!("Invalid reasoning effort"),
            })
        }
    };
    match config.kind {
        ProviderKind::Go => {
            require_api_key(config)?;
            let model = effective_model(config).unwrap_or_else(|| GO_DEFAULT_MODEL.to_owned());
            let mut provider = CompatibleProvider::go(
                config.api_key.clone(),
                model,
                effective_base_url(config),
                config.session_id.clone(),
            );
            provider.reasoning_effort = config.reasoning_effort.clone();
            Ok(Arc::new(provider) as Arc<dyn LLMProvider>)
        }
        ProviderKind::OpenAI => {
            require_api_key(config)?;
            let mut builder = LLMBuilder::<OpenAI>::new()
                .api_key(config.api_key.clone())
                .api_mode(OpenAIApiMode::ChatCompletions);
            if let Some(effort) = effort {
                builder = builder.reasoning_effort(effort);
            }
            let builder = configure_model_and_base_url(builder, config);
            let backend = builder
                .build()
                .map_err(|err| anyhow!("failed to build OpenAI backend: {err}"))?;
            Ok(backend as Arc<dyn LLMProvider>)
        }
        ProviderKind::Anthropic => {
            require_api_key(config)?;
            let builder = configure_model_and_base_url(
                LLMBuilder::<Anthropic>::new().api_key(config.api_key.clone()),
                config,
            );
            let backend = builder
                .build()
                .map_err(|err| anyhow!("failed to build Anthropic backend: {err}"))?;
            Ok(backend as Arc<dyn LLMProvider>)
        }
        ProviderKind::Ollama => {
            let mut builder = LLMBuilder::<Ollama>::new();
            if !config.api_key.trim().is_empty() {
                builder = builder.api_key(config.api_key.clone());
            }
            let builder = configure_model_and_base_url(builder, config);
            let backend = builder
                .build()
                .map_err(|err| anyhow!("failed to build Ollama backend: {err}"))?;
            Ok(backend as Arc<dyn LLMProvider>)
        }
        ProviderKind::Custom => {
            require_api_key(config)?;
            let Some(base_url) = effective_base_url(config) else {
                bail!("Custom provider requires an explicit base_url");
            };
            let Some(model) = effective_model(config) else {
                bail!("Custom provider requires an explicit model or THEMIS_CUSTOM_MODEL");
            };
            let provider = CompatibleProvider::custom(config.api_key.clone(), model, base_url);
            Ok(Arc::new(provider) as Arc<dyn LLMProvider>)
        }
    }
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

#[cfg(test)]
// The env-serializing mutex is deliberately held across awaits: process env is
// global, so the guard must span the whole test. Test-only, single lock, no
// deadlock risk.
#[allow(clippy::await_holding_lock)]
mod tests {
    use super::*;
    use std::sync::Mutex;

    use autoagents::llm::chat::FunctionTool;
    use autoagents::llm::FunctionCall;
    use serde_json::{json, Value};
    use wiremock::matchers::{method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    /// Serializes tests that touch `THEMIS_*` environment variables (`resolve`
    /// reads them, and process env is global across test threads).
    static ENV_LOCK: Mutex<()> = Mutex::new(());

    fn user_message(content: &str) -> ChatMessage {
        ChatMessage {
            role: ChatRole::User,
            message_type: MessageType::Text,
            content: content.to_owned(),
        }
    }

    fn chat_ok_body(text: &str) -> Value {
        json!({
            "id": "chatcmpl-test",
            "object": "chat.completion",
            "choices": [{
                "index": 0,
                "message": {"role": "assistant", "content": text},
                "finish_reason": "stop"
            }],
            "usage": {"prompt_tokens": 3, "completion_tokens": 5, "total_tokens": 8}
        })
    }

    fn tool_call_body() -> Value {
        json!({
            "choices": [{
                "message": {
                    "role": "assistant",
                    "content": null,
                    "tool_calls": [{
                        "id": "call_1",
                        "type": "function",
                        "function": {
                            "name": "read_file",
                            "arguments": "{\"path\": \"src/main.rs\"}"
                        }
                    }]
                }
            }]
        })
    }

    fn test_tool() -> Tool {
        Tool {
            tool_type: "function".to_owned(),
            function: FunctionTool {
                name: "read_file".to_owned(),
                description: "Reads a file".to_owned(),
                parameters: json!({"type": "object"}),
            },
        }
    }

    async fn mount_chat(mock: &MockServer, body: Value) {
        Mock::given(method("POST"))
            .and(path("/chat/completions"))
            .respond_with(ResponseTemplate::new(200).set_body_json(body))
            .mount(mock)
            .await;
    }

    #[tokio::test]
    async fn responses_reject_incomplete_streams() {
        use futures_util::StreamExt;
        for ending in ["response.failed", "response.incomplete", "response.created"] {
            let server = MockServer::start().await;
            Mock::given(method("POST"))
                .and(path("/responses"))
                .respond_with(ResponseTemplate::new(200).set_body_raw(
                    format!("data: {{\"type\":\"{ending}\"}}\n\n"),
                    "text/event-stream",
                ))
                .mount(&server)
                .await;
            let provider = CompatibleProvider::go(
                "synthetic".into(),
                "muse-spark-1.3-contributor".into(),
                Some(server.uri()),
                Some("thread-123".into()),
            );
            let mut stream = provider
                .chat_stream_with_tools(&[user_message("hi")], None, None)
                .await
                .unwrap();
            assert!(stream.next().await.unwrap().is_err(), "{ending}");
            assert_eq!(
                server.received_requests().await.unwrap()[0].headers[GO_SESSION_HEADER],
                "thread-123"
            );
        }
    }

    #[tokio::test]
    async fn muse_uses_responses_for_text_and_tool_continuations() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/zen/go/v1/responses"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "output": [
                    {"type": "message", "content": [{"type": "output_text", "text": "Reading."}]},
                    {"type": "function_call", "call_id": "call_1", "name": "read_file", "arguments": "{}"}
                ]
            })))
            .expect(2)
            .mount(&server).await;
        let mut config = ProviderConfig::new(ProviderKind::Go, "synthetic-key")
            .with_model("muse-spark-1.3-contributor")
            .with_base_url(format!("{}/zen/go/v1", server.uri()));
        config.session_id = Some("conversation-123".into());
        let provider = resolve(&config).await.unwrap();
        let response = provider
            .chat_with_tools(&[user_message("read it")], Some(&[test_tool()]), None)
            .await
            .unwrap();
        assert_eq!(response.text().as_deref(), Some("Reading."));
        let calls = response.tool_calls().unwrap();
        assert_eq!(calls[0].function.name, "read_file");
        let mut result = calls[0].clone();
        result.function.arguments = "file contents".into();
        provider
            .chat(
                &[
                    user_message("read it"),
                    ChatMessage {
                        role: ChatRole::Assistant,
                        message_type: MessageType::ToolUse(calls),
                        content: String::new(),
                    },
                    ChatMessage {
                        role: ChatRole::Tool,
                        message_type: MessageType::ToolResult(vec![result]),
                        content: String::new(),
                    },
                ],
                None,
            )
            .await
            .unwrap();
        let requests = server.received_requests().await.unwrap();
        assert!(requests
            .iter()
            .all(|request| request.headers.contains_key(GO_SESSION_HEADER)));
        assert_eq!(requests[0].headers[GO_SESSION_HEADER], "conversation-123");
        assert_eq!(requests[1].headers[GO_SESSION_HEADER], "conversation-123");
        let first: Value = requests[0].body_json().unwrap();
        assert_eq!(first["model"], "muse-spark-1.3-contributor");
        assert_eq!(first["input"][0]["content"], "read it");
        assert_eq!(first["tools"][0]["name"], "read_file");
        assert_eq!(first["store"], false);
        let second: Value = requests[1].body_json().unwrap();
        assert_eq!(second["input"][1]["type"], "function_call");
        assert_eq!(second["input"][2]["type"], "function_call_output");
        assert_eq!(second["input"][2]["call_id"], "call_1");
        assert_eq!(second["input"][2]["output"], "file contents");
    }

    #[tokio::test]
    async fn muse_streams_responses_text_and_tools() {
        use futures_util::StreamExt;
        let server = MockServer::start().await;
        let events = [
            json!({"type": "response.output_text.delta", "delta": "Reading."}),
            json!({"type": "response.output_item.added", "output_index": 0, "item": {"type": "function_call", "id": "fc_1", "call_id": "call_1", "name": "read_file", "arguments": ""}}),
            json!({"type": "response.function_call_arguments.delta", "output_index": 0, "delta": "{"}),
            json!({"type": "response.function_call_arguments.delta", "output_index": 0, "delta": "}"}),
            json!({"type": "response.function_call_arguments.done", "output_index": 0, "arguments": "{}"}),
            json!({"type": "response.completed", "response": {}}),
        ].iter().map(|event| format!("data: {event}\n\n")).collect::<String>();
        Mock::given(method("POST"))
            .and(path("/responses"))
            .respond_with(ResponseTemplate::new(200).set_body_raw(events, "text/event-stream"))
            .expect(1)
            .mount(&server)
            .await;
        let provider = resolve(
            &ProviderConfig::new(ProviderKind::Go, "synthetic-key")
                .with_model("muse-spark-1.2-contributor")
                .with_base_url(server.uri()),
        )
        .await
        .unwrap();
        let mut stream = provider
            .chat_stream_with_tools(&[user_message("read it")], Some(&[test_tool()]), None)
            .await
            .unwrap();
        let mut text = String::new();
        let mut calls = Vec::new();
        while let Some(chunk) = stream.next().await {
            match chunk.unwrap() {
                StreamChunk::Text(delta) => text.push_str(&delta),
                StreamChunk::ToolUseComplete { tool_call, .. } => calls.push(tool_call),
                _ => {}
            }
        }
        assert!(server.received_requests().await.unwrap()[0]
            .headers
            .contains_key(GO_SESSION_HEADER));
        assert_eq!(text, "Reading.");
        assert_eq!(calls.len(), 1);
        assert_eq!(calls[0].function.name, "read_file");
        let requests = server.received_requests().await.unwrap();
        assert_eq!(requests[0].body_json::<Value>().unwrap()["stream"], true);
    }

    #[tokio::test]
    async fn qwen_uses_messages_for_tools_and_streaming() {
        use futures_util::StreamExt;
        let server = MockServer::start().await;
        Mock::given(method("POST")).and(path("/zen/go/v1/messages"))
            .and(wiremock::matchers::body_partial_json(json!({"stream": false})))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "content": [{"type": "text", "text": "Reading."}, {"type": "tool_use", "id": "call_1", "name": "read_file", "input": {"path": "a"}}],
                "stop_reason": "tool_use"
            }))).expect(2).mount(&server).await;
        let events = [
            json!({"type": "content_block_start", "index": 0, "content_block": {"type": "text", "text": ""}}),
            json!({"type": "content_block_delta", "index": 0, "delta": {"type": "text_delta", "text": "Done."}}),
            json!({"type": "content_block_start", "index": 1, "content_block": {"type": "tool_use", "id": "call_2", "name": "read_file", "input": {}}}),
            json!({"type": "content_block_delta", "index": 1, "delta": {"type": "input_json_delta", "partial_json": "{}"}}),
            json!({"type": "content_block_start", "index": 2, "content_block": {"type": "tool_use", "id": "call_3", "name": "read_file", "input": {}}}),
            json!({"type": "message_delta", "delta": {"stop_reason": "tool_use"}}),
            json!({"type": "message_stop"}),
        ].iter().map(|event| format!("data: {event}\n\n")).collect::<String>();
        Mock::given(method("POST"))
            .and(path("/zen/go/v1/messages"))
            .and(wiremock::matchers::body_partial_json(
                json!({"stream": true}),
            ))
            .respond_with(ResponseTemplate::new(200).set_body_raw(events, "text/event-stream"))
            .expect(1)
            .mount(&server)
            .await;
        let provider = resolve(
            &ProviderConfig::new(ProviderKind::Go, "synthetic-key")
                .with_model("qwen3.7-max")
                .with_base_url(format!("{}/zen/go/v1", server.uri())),
        )
        .await
        .unwrap();
        let tools = [test_tool()];
        let messages = [
            ChatMessage {
                role: ChatRole::System,
                message_type: MessageType::Text,
                content: "Be helpful.".into(),
            },
            user_message("read it"),
        ];
        let response = provider
            .chat_with_tools(&messages, Some(&tools), None)
            .await
            .unwrap();
        assert_eq!(response.text().as_deref(), Some("Reading."));
        let calls = response.tool_calls().unwrap();
        assert_eq!(calls[0].function.arguments, r#"{"path":"a"}"#);
        let mut result = calls[0].clone();
        result.function.arguments = "contents".into();
        provider
            .chat(
                &[
                    user_message("read it"),
                    ChatMessage {
                        role: ChatRole::Assistant,
                        message_type: MessageType::ToolUse(calls),
                        content: String::new(),
                    },
                    ChatMessage {
                        role: ChatRole::Tool,
                        message_type: MessageType::ToolResult(vec![result]),
                        content: String::new(),
                    },
                ],
                None,
            )
            .await
            .unwrap();
        let mut stream = provider
            .chat_stream_with_tools(&messages, Some(&tools), None)
            .await
            .unwrap();
        let mut text = String::new();
        let mut calls = Vec::new();
        while let Some(chunk) = stream.next().await {
            match chunk.unwrap() {
                StreamChunk::Text(delta) => text.push_str(&delta),
                StreamChunk::ToolUseComplete { tool_call, .. } => calls.push(tool_call),
                _ => {}
            }
        }
        assert_eq!(text, "Done.");
        assert_eq!(calls[0].id, "call_2");
        assert_eq!(calls[1].function.arguments, "{}");
        assert_eq!(calls[0].function.arguments, "{}");
        let requests = server.received_requests().await.unwrap();
        assert!(requests
            .iter()
            .all(|request| request.headers.contains_key(GO_SESSION_HEADER)));
        let first: Value = requests[0].body_json().unwrap();
        assert_eq!(first["model"], "qwen3.7-max");
        assert_eq!(first["system"], "Be helpful.");
        assert_eq!(first["tools"][0]["input_schema"]["type"], "object");
        assert!(first["max_tokens"].as_u64().unwrap() > 0);
        assert_eq!(requests[0].headers["anthropic-version"], "2023-06-01");
        assert!(requests[0].headers.contains_key(GO_SESSION_HEADER));
        let second: Value = requests[1].body_json().unwrap();
        assert_eq!(second["messages"][1]["content"][0]["type"], "tool_use");
        assert_eq!(second["messages"][2]["content"][0]["tool_use_id"], "call_1");
        assert_eq!(second["messages"][2]["content"][0]["content"], "contents");
    }

    #[tokio::test]
    async fn messages_reject_truncated_or_interrupted_responses() {
        use futures_util::StreamExt;
        for (body, streaming, expected) in [
            (json!({"content": [{"type": "text", "text": "partial"}], "stop_reason": "max_tokens"}).to_string(), false, "truncated"),
            ("data: {\"type\":\"message_delta\",\"delta\":{\"stop_reason\":\"max_tokens\"}}\n\n".into(), true, "truncated"),
            ("data: {\"type\":\"content_block_delta\",\"delta\":{\"type\":\"text_delta\",\"text\":\"partial\"}}\n\n".into(), true, "ended before completion"),
        ] {
            let server = MockServer::start().await;
            Mock::given(method("POST")).and(path("/messages"))
                .respond_with(ResponseTemplate::new(200).set_body_raw(body, if streaming { "text/event-stream" } else { "application/json" }))
                .mount(&server).await;
            let provider = resolve(&ProviderConfig::new(ProviderKind::Go, "synthetic-key").with_model("minimax-m3").with_base_url(server.uri())).await.unwrap();
            if streaming {
                let chunks = provider.chat_stream_with_tools(&[user_message("hello")], None, None).await.unwrap().collect::<Vec<_>>().await;
                assert!(chunks.iter().any(|chunk| chunk.as_ref().err().is_some_and(|error| error.to_string().contains(expected))));
            } else {
                assert!(provider.chat(&[user_message("hello")], None).await.err().unwrap().to_string().contains(expected));
            }
        }
    }

    // Snapshot of GET /zen/go/v1/models, verified against docs/go on 2026-09-23.
    #[tokio::test]
    async fn every_go_catalog_model_uses_its_wire_protocol() {
        let server = MockServer::start().await;
        for (path_name, body) in [
            (
                "/responses",
                json!({"output": [{"type": "message", "content": [{"type": "output_text", "text": "ok"}]}]}),
            ),
            (
                "/messages",
                json!({"content": [{"type": "text", "text": "ok"}], "stop_reason": "end_turn"}),
            ),
            ("/chat/completions", chat_ok_body("ok")),
        ] {
            Mock::given(method("POST"))
                .and(path(path_name))
                .respond_with(ResponseTemplate::new(200).set_body_json(body))
                .mount(&server)
                .await;
        }
        for (endpoint, models) in [
            (
                "/responses",
                &[
                    "gpt-5.6-luna",
                    "gpt-6-luna",
                    "grok-4.5",
                    "grok-4.6",
                    "grok-4.7",
                    "muse-spark-1.2-contributor",
                    "muse-spark-1.3-contributor",
                ][..],
            ),
            (
                "/messages",
                &[
                    "minimax-m3",
                    "minimax-m2.7",
                    "minimax-m2.5",
                    "qwen3.7-max",
                    "qwen3.8-max",
                    "qwen3.8-flash",
                    "qwen3.7-plus",
                    "qwen3.6-plus",
                    "qwen3.5-plus",
                ][..],
            ),
            (
                "/chat/completions",
                &[
                    "kimi-k3",
                    "kimi-k2.7-code",
                    "kimi-k2.6",
                    "longcat-2.0",
                    "kimi-k2.5",
                    "glm-5.2",
                    "glm-5.3-flash",
                    "glm-5.3",
                    "glm-5.1",
                    "glm-5",
                    "deepseek-v4-pro",
                    "deepseek-v4-flash",
                    "deepseek-flash",
                    "deepseek-v4.1-flash",
                    "deepseek-v4-flash-vision-exp",
                    "mimo-v2-pro",
                    "mimo-v2-omni",
                    "mimo-v2.6-pro",
                    "mimo-v2.6-flash",
                    "space-bunny-free",
                    "mimo-v2.5-pro",
                    "mimo-v2.5",
                    "hy4-preview",
                    "hy3",
                    "hy3-preview",
                    "omen-alpha",
                ][..],
            ),
        ] {
            for model in models {
                let provider = resolve(
                    &ProviderConfig::new(ProviderKind::Go, "synthetic-key")
                        .with_model(*model)
                        .with_base_url(server.uri()),
                )
                .await
                .unwrap();
                let reply = provider.chat(&[user_message("hello")], None).await.unwrap();
                assert_eq!(reply.text().as_deref(), Some("ok"), "{model}");
                let requests = server.received_requests().await.unwrap();
                let request = requests.last().unwrap();
                assert_eq!(request.url.path(), endpoint, "{model}");
                assert!(
                    !request.headers[GO_SESSION_HEADER]
                        .to_str()
                        .unwrap()
                        .is_empty(),
                    "{model}"
                );
                assert_eq!(request.body_json::<Value>().unwrap()["model"], *model);
            }
        }
        assert_eq!(server.received_requests().await.unwrap().len(), 42);
    }

    #[test]
    fn kind_from_str_is_case_insensitive() {
        for (input, expected) in [
            ("go", ProviderKind::Go),
            ("Go", ProviderKind::Go),
            ("GO", ProviderKind::Go),
            ("opencode-go", ProviderKind::Go),
            ("OpenAI", ProviderKind::OpenAI),
            ("OPENAI", ProviderKind::OpenAI),
            ("Anthropic", ProviderKind::Anthropic),
            ("OLLAMA", ProviderKind::Ollama),
            ("Custom", ProviderKind::Custom),
            ("  custom  ", ProviderKind::Custom),
        ] {
            assert_eq!(input.parse::<ProviderKind>().unwrap(), expected, "{input}");
        }
    }

    #[test]
    fn kind_from_str_rejects_unknown() {
        for input in ["", "gpt", "azure-openai", "openrouter", "go2"] {
            assert!(input.parse::<ProviderKind>().is_err(), "{input}");
        }
    }

    #[test]
    fn kind_display_roundtrips() {
        for kind in [
            ProviderKind::Go,
            ProviderKind::OpenAI,
            ProviderKind::Anthropic,
            ProviderKind::Ollama,
            ProviderKind::Custom,
        ] {
            assert_eq!(kind.to_string().parse::<ProviderKind>().unwrap(), kind);
        }
        assert_eq!(ProviderKind::Go.to_string(), "go");
        assert_eq!(ProviderKind::OpenAI.to_string(), "openai");
    }

    #[tokio::test]
    async fn streaming_keeps_public_text_and_assembles_tool_arguments() {
        use futures_util::StreamExt;
        let server = MockServer::start().await;
        let events = [
            json!({"choices":[{"delta":{"content":"I’ll read ","reasoning_content":"private"},"finish_reason":null}]}),
            json!({"choices":[{"delta":{"content":"the file.","tool_calls":[{"index":0,"id":"call1","function":{"name":"read_file","arguments":"{\"file_path\":"}}]},"finish_reason":null}]}),
            json!({"choices":[{"delta":{"tool_calls":[{"index":0,"function":{"arguments":"\"check.txt\"}"}}]},"finish_reason":"tool_calls"}]}),
        ];
        let body = events
            .iter()
            .map(|event| format!("data: {event}\r\n\r\n"))
            .collect::<String>()
            + "data: [DONE]\n\n";
        Mock::given(method("POST"))
            .and(path("/chat/completions"))
            .respond_with(ResponseTemplate::new(200).set_body_raw(body, "text/event-stream"))
            .mount(&server)
            .await;
        let provider =
            CompatibleProvider::go("test".into(), "mock".into(), Some(server.uri()), None);
        let mut stream = provider
            .chat_stream_with_tools(&[user_message("read")], None, None)
            .await
            .unwrap();
        let mut text = String::new();
        let mut calls = Vec::new();
        while let Some(chunk) = stream.next().await {
            match chunk.unwrap() {
                StreamChunk::Text(delta) => text.push_str(&delta),
                StreamChunk::ToolUseComplete { tool_call, .. } => calls.push(tool_call),
                _ => {}
            }
        }
        assert_eq!(text, "I’ll read the file.");
        assert_eq!(calls.len(), 1);
        assert_eq!(calls[0].function.arguments, "{\"file_path\":\"check.txt\"}");
        assert_eq!(
            server.received_requests().await.unwrap()[0]
                .body_json::<Value>()
                .unwrap()["stream"],
            true
        );
    }

    #[tokio::test]
    async fn reasoning_effort_reaches_wire_and_rejects_unsupported_models() {
        let server = MockServer::start().await;
        Mock::given(method("POST")).and(path("/responses"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({"output": [{"type": "message", "content": [{"type": "output_text", "text": "done"}]}]})))
            .mount(&server).await;
        let mut config = ProviderConfig::new(ProviderKind::Go, "synthetic-test-key")
            .with_model("gpt-5.6-luna")
            .with_base_url(server.uri());
        config.reasoning_effort = Some("high".to_owned());
        resolve(&config)
            .await
            .unwrap()
            .chat(&[user_message("hello")], None)
            .await
            .unwrap();
        let requests = server.received_requests().await.unwrap();
        let body: Value = requests[0].body_json().unwrap();
        assert_eq!(body["reasoning"]["effort"], "high");
        config.model = Some("minimax-m2.5".to_owned());
        assert!(resolve(&config).await.is_err());
        config.model = Some("gpt-5.6-luna".to_owned());
        config.reasoning_effort = Some("invalid".to_owned());
        assert!(resolve(&config).await.is_err());
    }

    #[tokio::test]
    async fn go_prefers_explicit_model_over_env() {
        let _guard = ENV_LOCK.lock().unwrap();
        std::env::set_var(GO_MODEL_ENV_VAR, "env-model");
        let server = MockServer::start().await;
        mount_chat(&server, chat_ok_body("hi")).await;

        let config = ProviderConfig::new(ProviderKind::Go, "key")
            .with_model("explicit-model")
            .with_base_url(server.uri());
        let handle: Arc<dyn LLMProvider> = resolve(&config).await.unwrap();
        handle.chat(&[user_message("hello")], None).await.unwrap();

        let requests = server.received_requests().await.unwrap();
        assert_eq!(requests.len(), 1);
        let body: Value = requests[0].body_json().unwrap();
        assert_eq!(body["model"], "explicit-model");
        std::env::remove_var(GO_MODEL_ENV_VAR);
    }

    #[tokio::test]
    async fn go_model_falls_back_to_env_then_default() {
        let _guard = ENV_LOCK.lock().unwrap();
        let server = MockServer::start().await;
        mount_chat(&server, chat_ok_body("hi")).await;

        std::env::set_var(GO_MODEL_ENV_VAR, "env-model");
        let config = ProviderConfig::new(ProviderKind::Go, "key").with_base_url(server.uri());
        let handle = resolve(&config).await.unwrap();
        handle.chat(&[user_message("hello")], None).await.unwrap();
        let requests = server.received_requests().await.unwrap();
        let body: Value = requests[0].body_json().unwrap();
        assert_eq!(body["model"], "env-model");

        std::env::remove_var(GO_MODEL_ENV_VAR);
        let handle = resolve(&config).await.unwrap();
        handle.chat(&[user_message("hello")], None).await.unwrap();
        let requests = server.received_requests().await.unwrap();
        let body: Value = requests[1].body_json().unwrap();
        assert_eq!(body["model"], GO_DEFAULT_MODEL);
    }

    #[tokio::test]
    async fn go_sends_themis_user_agent_and_session() {
        let _guard = ENV_LOCK.lock().unwrap();
        std::env::set_var(GO_SESSION_ENV_VAR, "thread-42");
        std::env::remove_var(GO_MODEL_ENV_VAR);
        let server = MockServer::start().await;
        mount_chat(&server, chat_ok_body("hi")).await;

        let config = ProviderConfig::new(ProviderKind::Go, "key")
            .with_model("m")
            .with_base_url(server.uri());
        let handle = resolve(&config).await.unwrap();
        let response = handle.chat(&[user_message("hello")], None).await.unwrap();
        assert_eq!(response.text().as_deref(), Some("hi"));

        let requests = server.received_requests().await.unwrap();
        assert_eq!(requests.len(), 1);
        let headers = &requests[0].headers;
        assert_eq!(
            headers.get("user-agent").unwrap().to_str().unwrap(),
            themis_user_agent()
        );
        assert_eq!(
            headers.get(GO_SESSION_HEADER).unwrap().to_str().unwrap(),
            "thread-42"
        );
        std::env::remove_var(GO_SESSION_ENV_VAR);
    }

    #[tokio::test]
    async fn go_session_defaults_when_env_absent() {
        let _guard = ENV_LOCK.lock().unwrap();
        std::env::remove_var(GO_SESSION_ENV_VAR);
        std::env::remove_var(GO_MODEL_ENV_VAR);
        let server = MockServer::start().await;
        mount_chat(&server, chat_ok_body("hi")).await;

        let config = ProviderConfig::new(ProviderKind::Go, "key")
            .with_model("m")
            .with_base_url(server.uri());
        resolve(&config)
            .await
            .unwrap()
            .chat(&[user_message("hello")], None)
            .await
            .unwrap();

        let requests = server.received_requests().await.unwrap();
        assert_eq!(
            requests[0]
                .headers
                .get(GO_SESSION_HEADER)
                .unwrap()
                .to_str()
                .unwrap(),
            GO_DEFAULT_SESSION
        );
    }

    #[tokio::test]
    async fn go_per_instance_session_beats_env() {
        let _guard = ENV_LOCK.lock().unwrap();
        std::env::set_var(GO_SESSION_ENV_VAR, "env-session");
        let server = MockServer::start().await;
        mount_chat(&server, chat_ok_body("hi")).await;

        // Per-conversation IDs pass through the constructor (spike option 1).
        let provider = CompatibleProvider::go(
            "key".to_owned(),
            "m".to_owned(),
            Some(server.uri()),
            Some("thread-7".to_owned()),
        );
        provider.chat(&[user_message("hello")], None).await.unwrap();

        let requests = server.received_requests().await.unwrap();
        assert_eq!(
            requests[0]
                .headers
                .get(GO_SESSION_HEADER)
                .unwrap()
                .to_str()
                .unwrap(),
            "thread-7"
        );
        std::env::remove_var(GO_SESSION_ENV_VAR);
    }

    #[tokio::test]
    async fn go_tool_call_roundtrip_through_dyn_handle() {
        let _guard = ENV_LOCK.lock().unwrap();
        std::env::remove_var(GO_MODEL_ENV_VAR);
        std::env::remove_var(GO_SESSION_ENV_VAR);
        let server = MockServer::start().await;
        mount_chat(&server, tool_call_body()).await;

        // Explicit `Arc<dyn LLMProvider>` annotation: this is the exact handle
        // type `AgentBuilder::llm` accepts, and `chat_with_tools` returning
        // parsed tool calls is the ReAct loop's core LLM primitive.
        let config = ProviderConfig::new(ProviderKind::Go, "key")
            .with_model("m")
            .with_base_url(server.uri());
        let handle: Arc<dyn LLMProvider> = resolve(&config).await.unwrap();
        let tools = [test_tool()];
        let response = handle
            .chat_with_tools(&[user_message("read it")], Some(&tools), None)
            .await
            .unwrap();
        let calls = response.tool_calls().unwrap();
        assert_eq!(calls.len(), 1);
        assert_eq!(calls[0].id, "call_1");
        assert_eq!(calls[0].function.name, "read_file");
        assert_eq!(calls[0].function.arguments, "{\"path\": \"src/main.rs\"}");

        // The tool schema must have reached the wire.
        let requests = server.received_requests().await.unwrap();
        let body: Value = requests[0].body_json().unwrap();
        assert_eq!(body["tools"][0]["function"]["name"], "read_file");
    }

    #[tokio::test]
    async fn tool_use_and_result_messages_reach_wire() {
        let _guard = ENV_LOCK.lock().unwrap();
        std::env::remove_var(GO_MODEL_ENV_VAR);
        std::env::remove_var(GO_SESSION_ENV_VAR);
        let server = MockServer::start().await;
        mount_chat(&server, chat_ok_body("done")).await;

        let config = ProviderConfig::new(ProviderKind::Go, "key")
            .with_model("m")
            .with_base_url(server.uri());
        let handle = resolve(&config).await.unwrap();
        let call = ToolCall {
            id: "call_1".to_owned(),
            call_type: "function".to_owned(),
            function: FunctionCall {
                name: "read_file".to_owned(),
                arguments: "{\"path\": \"a\"}".to_owned(),
            },
        };
        let result = ToolCall {
            id: "call_1".to_owned(),
            call_type: "function".to_owned(),
            function: FunctionCall {
                name: "read_file".to_owned(),
                arguments: "file contents".to_owned(),
            },
        };
        let messages = [
            user_message("read it"),
            ChatMessage {
                role: ChatRole::Assistant,
                message_type: MessageType::ToolUse(vec![call]),
                content: String::new(),
            },
            ChatMessage {
                role: ChatRole::Tool,
                message_type: MessageType::ToolResult(vec![result]),
                content: String::new(),
            },
        ];
        handle.chat(&messages, None).await.unwrap();

        let requests = server.received_requests().await.unwrap();
        let body: Value = requests[0].body_json().unwrap();
        let sent = body["messages"].as_array().unwrap();
        assert_eq!(sent.len(), 3);
        assert_eq!(sent[1]["role"], "assistant");
        assert_eq!(sent[1]["tool_calls"][0]["id"], "call_1");
        assert_eq!(sent[2]["role"], "tool");
        assert_eq!(sent[2]["tool_call_id"], "call_1");
        assert_eq!(sent[2]["content"], "file contents");
    }

    /// Same scripted assertion against every OpenAI-shaped backend: Go
    /// (in-tree provider), direct OpenAI (named SDK backend), and Custom URL.
    #[tokio::test]
    async fn provider_matrix_openai_shaped() {
        let _guard = ENV_LOCK.lock().unwrap();
        for var in [
            GO_MODEL_ENV_VAR,
            "THEMIS_OPENAI_MODEL",
            "THEMIS_CUSTOM_MODEL",
            GO_SESSION_ENV_VAR,
        ] {
            std::env::remove_var(var);
        }
        for kind in [ProviderKind::Go, ProviderKind::OpenAI, ProviderKind::Custom] {
            let server = MockServer::start().await;
            mount_chat(&server, chat_ok_body("matrix-hi")).await;

            let config = ProviderConfig::new(kind, "key")
                .with_model("matrix-model")
                .with_base_url(server.uri());
            let handle: Arc<dyn LLMProvider> = resolve(&config).await.unwrap();
            let response = handle.chat(&[user_message("ping")], None).await.unwrap();
            assert_eq!(response.text().as_deref(), Some("matrix-hi"), "{kind}");

            let requests = server.received_requests().await.unwrap();
            assert_eq!(requests.len(), 1, "{kind}");
            let body: Value = requests[0].body_json().unwrap();
            assert_eq!(body["model"], "matrix-model", "{kind}");
            assert_eq!(body["messages"][0]["content"], "ping", "{kind}");
        }
    }

    /// Ollama leg of the matrix: different wire shape (`/api/chat`), same
    /// scripted assertion through the same `resolve()` entry point.
    #[tokio::test]
    async fn provider_matrix_ollama_shaped() {
        let _guard = ENV_LOCK.lock().unwrap();
        std::env::remove_var("THEMIS_OLLAMA_MODEL");
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/api/chat"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "model": "llama-test",
                "created_at": "2024-08-04T08:52:19.385406455-07:00",
                "message": {"role": "assistant", "content": "matrix-hi"},
                "done": true
            })))
            .mount(&server)
            .await;

        let config = ProviderConfig::new(ProviderKind::Ollama, "")
            .with_model("llama-test")
            .with_base_url(server.uri());
        let handle: Arc<dyn LLMProvider> = resolve(&config).await.unwrap();
        let response = handle.chat(&[user_message("ping")], None).await.unwrap();
        assert_eq!(response.text().as_deref(), Some("matrix-hi"));

        let requests = server.received_requests().await.unwrap();
        assert_eq!(requests.len(), 1);
    }

    #[tokio::test]
    async fn refresh_go_models_success() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/models"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "object": "list",
                "data": [
                    {"id": "model-a", "object": "model"},
                    {"id": "model-b", "created": 1}
                ]
            })))
            .mount(&server)
            .await;

        let models = refresh_go_models(&server.uri(), "key").await.unwrap();
        assert_eq!(models, vec!["model-a".to_owned(), "model-b".to_owned()]);
        assert!(server.received_requests().await.unwrap()[0]
            .headers
            .contains_key(GO_SESSION_HEADER));

        let requests = server.received_requests().await.unwrap();
        assert_eq!(
            requests[0]
                .headers
                .get("user-agent")
                .unwrap()
                .to_str()
                .unwrap(),
            themis_user_agent()
        );
    }

    #[tokio::test]
    async fn refresh_go_models_malformed() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/models"))
            .respond_with(ResponseTemplate::new(200).set_body_string("not json{"))
            .mount(&server)
            .await;

        let err = refresh_go_models(&server.uri(), "key").await.unwrap_err();
        assert!(err.to_string().contains("failed to parse"), "{err}");
    }

    #[tokio::test]
    async fn refresh_go_models_http_error() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/models"))
            .respond_with(ResponseTemplate::new(401).set_body_string("bad key"))
            .mount(&server)
            .await;

        let err = refresh_go_models(&server.uri(), "key").await.unwrap_err();
        assert!(err.to_string().contains("401"), "{err}");
        assert!(err.to_string().contains("bad key"), "{err}");
    }

    #[tokio::test]
    async fn completion_and_embedding_roundtrip() {
        let _guard = ENV_LOCK.lock().unwrap();
        std::env::remove_var(GO_MODEL_ENV_VAR);
        std::env::remove_var(GO_SESSION_ENV_VAR);
        let server = MockServer::start().await;
        mount_chat(&server, chat_ok_body("completed")).await;
        Mock::given(method("POST"))
            .and(path("/embeddings"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "data": [{"embedding": [0.1, 0.2], "index": 0}]
            })))
            .mount(&server)
            .await;

        let config = ProviderConfig::new(ProviderKind::Go, "key")
            .with_model("m")
            .with_base_url(server.uri());
        let handle: Arc<dyn LLMProvider> = resolve(&config).await.unwrap();

        let completion = handle
            .complete(
                &CompletionRequest {
                    prompt: "say it".to_owned(),
                    max_tokens: Some(8),
                    temperature: Some(0.5),
                },
                None,
            )
            .await
            .unwrap();
        assert_eq!(completion.text, "completed");

        let embeddings = handle.embed(vec!["hello".to_owned()]).await.unwrap();
        assert_eq!(embeddings, vec![vec![0.1_f32, 0.2_f32]]);
    }

    #[tokio::test]
    async fn anthropic_resolves_without_network() {
        let config = ProviderConfig::new(ProviderKind::Anthropic, "key").with_model("m");
        let handle = resolve(&config).await.expect("resolve must succeed");
        assert_eq!(handle.model(), "m");
    }

    #[tokio::test]
    async fn custom_requires_base_url_and_model() {
        let _guard = ENV_LOCK.lock().unwrap();
        std::env::remove_var("THEMIS_CUSTOM_MODEL");

        let missing_url = ProviderConfig::new(ProviderKind::Custom, "key").with_model("m");
        assert!(resolve(&missing_url).await.is_err());

        let missing_model =
            ProviderConfig::new(ProviderKind::Custom, "key").with_base_url("http://x");
        assert!(resolve(&missing_model).await.is_err());
    }

    #[tokio::test]
    async fn empty_key_rejected_except_ollama() {
        let _guard = ENV_LOCK.lock().unwrap();
        for var in [
            GO_MODEL_ENV_VAR,
            "THEMIS_OPENAI_MODEL",
            "THEMIS_OLLAMA_MODEL",
            "THEMIS_CUSTOM_MODEL",
        ] {
            std::env::remove_var(var);
        }
        for kind in [ProviderKind::Go, ProviderKind::OpenAI, ProviderKind::Custom] {
            let config = ProviderConfig::new(kind, "")
                .with_model("m")
                .with_base_url("http://localhost:9");
            assert!(resolve(&config).await.is_err(), "{kind}");
        }
        // Ollama is local-first: no key required.
        let ollama =
            ProviderConfig::new(ProviderKind::Ollama, "").with_base_url("http://localhost:9");
        assert!(resolve(&ollama).await.is_ok());
    }
}
