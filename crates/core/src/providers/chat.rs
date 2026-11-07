//! The SDK chat, completion and embedding traits, over one request or one stream.

use super::*;

impl CompatibleProvider {
    /// One non-streaming chat request with this provider's model, settings and key.
    /// Claude refused a replayed thinking block because the conversation before it changed
    /// (context maintenance rewrote older messages mid-run). The request is valid without its
    /// thinking blocks, so it is sent once more with none; logs metadata only.
    fn thinking_unbound(&self, error: &LLMError) -> bool {
        let unbound = self.model.starts_with("claude-")
            && error
                .to_string()
                .contains("bound to a different conversation");
        if unbound {
            crate::diagnostics::emit(
                "provider",
                "thinking_unbound_retry",
                "warn",
                serde_json::json!({"model": self.model}),
            );
        }
        unbound
    }

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
        trace.provider(self.provider_id());
        trace.output_limit(
            sampling
                .and_then(|s| s.max_tokens)
                .or_else(|| OUTPUT_LIMIT.try_with(|limit| *limit).ok())
                .map(|limit| bounded_output_limit(&self.model, limit)),
        );
        let send = || async {
            let response = self
                .chat_request(messages, tools, false, sampling)?
                .send()
                .await
                .map_err(|err| transport_error("chat request failed", err))?;
            ensure_success(response, &self.provider_name).await
        };
        let response = match send().await {
            Err(error) if self.thinking_unbound(&error) => {
                reasoning::REPLAY_REASONING.scope(false, send()).await?
            }
            result => result?,
        };
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
        // summarizer for other models until their disable capability is verified. A summary
        // model on another provider is called with that provider (and its key).
        let summary_provider = COMPACTION_REQUEST.try_with(|_| ()).ok().map(|()| {
            let model = summary_model(&self.model);
            match &self.summary_provider {
                Some(other) if other.model == model => Self::clone(other),
                _ => Self {
                    model,
                    reasoning_effort: None,
                    summary_provider: None,
                    ..self.clone()
                },
            }
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
        trace.provider(self.provider_id());
        trace.output_limit(
            OUTPUT_LIMIT
                .try_with(|limit| bounded_output_limit(&self.model, *limit))
                .ok(),
        );
        let protocol = self.protocol();
        let send = || async {
            let response = self
                .chat_request(messages, tools, true, None)?
                .send()
                .await
                .map_err(|error| transport_error("stream request failed", error))?;
            ensure_success(response, &self.provider_name).await
        };
        let response = match send().await {
            Err(error) if self.thinking_unbound(&error) => {
                reasoning::REPLAY_REASONING.scope(false, send()).await?
            }
            result => result?,
        };
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
            model: self.wire_model(),
            messages: to_wire_messages(std::slice::from_ref(&message))?
                .into_iter()
                .map(chat_message_value)
                .collect::<Result<_, _>>()?,
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
        let response = ensure_success(response, &self.provider_name).await?;
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
            "model": self.wire_model(),
            "input": input,
        });
        let response = self
            .post(Protocol::ChatCompletions, "embeddings")
            .json(&body)
            .send()
            .await
            .map_err(|err| transport_error("embedding request failed", err))?;
        let response = ensure_success(response, &self.provider_name).await?;
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
