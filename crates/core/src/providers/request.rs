//! Building requests: provider setup, protocol and endpoint choice, request bodies and HTTP errors.

use super::*;

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
            provider_name: GO_PROVIDER_NAME.into(),
            custom: None,
            summary_provider: None,
            api_key,
            base_override: base_url.is_some(),
            base_url: normalize_base(base_url.as_deref().unwrap_or(GO_BASE_URL)),
            model,
            extra_headers: vec![(GO_SESSION_HEADER.to_owned(), session)],
            client: http_client(),
            reasoning_effort: None,
        }
    }

    /// Builds the provider for an OpenAI-compatible endpoint from `providers`: `model` is the
    /// qualified `<id>/<model>`, sent as `<model>`.
    pub fn custom(endpoint: &CustomEndpoint, api_key: String, model: String) -> Self {
        Self {
            provider_name: endpoint.name.as_str().into(),
            custom: Some(endpoint.id.clone()),
            summary_provider: None,
            api_key,
            base_override: true,
            base_url: normalize_base(&endpoint.base_url),
            model,
            extra_headers: Vec::new(),
            client: http_client(),
            reasoning_effort: None,
        }
    }

    /// The model id the provider expects: a configured provider's without its `<id>/`.
    pub(super) fn wire_model(&self) -> &str {
        self.custom
            .as_deref()
            .and_then(|id| self.model.strip_prefix(id))
            .and_then(|rest| rest.strip_prefix('/'))
            .unwrap_or(&self.model)
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

    /// The stable provider id the usage ledger records: `opencode-go` for Go, else the
    /// configured provider's `id` from runtime.jsonc.
    pub fn provider_id(&self) -> &str {
        self.custom.as_deref().unwrap_or(GO_PROVIDER_ID)
    }

    /// The wire protocol for this provider's model (see [`catalog::route`]).
    pub(super) fn protocol(&self) -> Protocol {
        if self.custom.is_none() {
            route(&self.model).0
        } else {
            Protocol::ChatCompletions
        }
    }

    pub(super) fn uses_responses(&self) -> bool {
        self.protocol() == Protocol::Responses
    }

    pub(super) fn uses_messages(&self) -> bool {
        self.protocol() == Protocol::Messages
    }

    /// Where `protocol` requests go: the explicit base URL, else the model's catalog
    /// endpoint when it speaks `protocol`, else Go's default endpoint for it.
    pub(super) fn endpoint(&self, protocol: Protocol) -> String {
        if self.base_override {
            return self.base_url.clone();
        }
        match route(&self.model) {
            (routed, endpoint) if routed == protocol => endpoint,
            _ => protocol.default_endpoint(),
        }
    }

    pub(super) fn chat_request(
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
        if has_images(messages) && !accepts_images(&self.model) {
            return Err(LLMError::invalid_request(format!(
                "Model '{}' cannot read images; screenshots go only to models that do",
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
                model: self.wire_model(),
                messages: to_wire_messages(messages)?
                    .into_iter()
                    .map(chat_message_value)
                    .collect::<Result<_, _>>()?,
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
        // A configured provider gets no OpenCode thinking field it may not know.
        if least_thinking() && self.custom.is_none() {
            if self.uses_responses() {
                body["reasoning"] = serde_json::json!({"effort":summary_effort(&self.model)});
            } else {
                match summary_thinking(&self.model) {
                    SummaryThinking::Switch => {
                        body["thinking"] = serde_json::json!({"type":"disabled"});
                    }
                    SummaryThinking::Effort(effort) => body["reasoning_effort"] = effort.into(),
                    SummaryThinking::Nothing => {}
                }
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
        if !least_thinking() {
            let thinking = thinking_default(&self.model);
            if let (None, Some(effort), false) = (
                &self.reasoning_effort,
                thinking.effort.as_deref(),
                thinking.switch_on,
            ) {
                if self.uses_responses() {
                    body["reasoning"] = serde_json::json!({"effort":effort});
                } else if !self.uses_messages() {
                    body["reasoning_effort"] = effort.into();
                }
            }
            if thinking.switch_off && !self.uses_responses() {
                // The person turned this model's thinking switch off (`models` override).
                body["thinking"] = serde_json::json!({"type":"disabled"});
            } else if thinking.switch_on && self.model.starts_with("claude-") {
                // Claude rejects a fixed budget: adaptive thinking, steered by effort.
                body["thinking"] = serde_json::json!({"type":"adaptive"});
                if let Some(effort) = self
                    .reasoning_effort
                    .as_deref()
                    .or(thinking.effort.as_deref())
                {
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
    pub(super) fn post(&self, protocol: Protocol, path: &str) -> reqwest::RequestBuilder {
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
pub(super) fn transport_error(context: &str, err: reqwest::Error) -> LLMError {
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
pub(super) async fn ensure_success(
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
