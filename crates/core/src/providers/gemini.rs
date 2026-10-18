//! Google Gemini `generateContent`: Gemini models on OpenCode Go.
//!
//! History goes out as `contents` (roles `user` and `model`), tools as function
//! declarations with their JSON Schema, and replies come back normalized to the Chat
//! Completions shape the rest of the provider reads.
//!
//! Gemini signs its reasoning: the first function call of each step carries a
//! `thoughtSignature`, and a request whose history has a function call without one is
//! refused. The signature is kept with the reasoning (see `reasoning`) and sent back on
//! that call; when it is gone (replay off, or history from before a restart) Google's
//! documented placeholder goes back instead.

use super::*;
use serde_json::{json, Value};
use std::sync::atomic::{AtomicU64, Ordering};

/// Google's documented stand-in for a signature that is no longer available.
const SKIP_SIGNATURE: &str = "skip_thought_signature_validator";

/// Numbers tool calls that come without an ID.
static CALL_COUNTER: AtomicU64 = AtomicU64::new(0);

impl CompatibleProvider {
    /// A `generateContent` request: output cap, sampling and thinking level go in
    /// `generationConfig`. Summaries think as little as Gemini allows.
    pub(super) fn gemini_request(
        &self,
        messages: &[ChatMessage],
        tools: Option<&[Tool]>,
        stream: bool,
        sampling: Option<&SamplingOverrides>,
    ) -> Result<reqwest::RequestBuilder, LLMError> {
        let mut body = gemini_body(messages, tools)?;
        let config = &mut body["generationConfig"];
        let limit = sampling
            .and_then(|sampling| sampling.max_tokens)
            .or_else(|| OUTPUT_LIMIT.try_with(|limit| *limit).ok());
        if let Some(limit) = limit {
            config["maxOutputTokens"] = bounded_output_limit(&self.model, limit).into();
        }
        if let Some(temperature) = sampling.and_then(|sampling| sampling.temperature) {
            config["temperature"] = temperature.into();
        }
        if let Some(top_p) = sampling.and_then(|sampling| sampling.top_p) {
            config["topP"] = top_p.into();
        }
        let level = if COMPACTION_REQUEST.try_with(|_| ()).is_ok() {
            Some("low")
        } else {
            self.reasoning_effort
                .as_deref()
                .or(thinking_default(&self.model).effort)
        };
        if let Some(level) = level {
            config["thinkingConfig"] = json!({"thinkingLevel": level});
        }
        let path = gemini_path(&self.model, stream)?;
        Ok(self.post(Protocol::Gemini, &path).json(&body))
    }
}

/// The request path for `model`: streamed as server-sent events, or one reply.
pub(super) fn gemini_path(model: &str, stream: bool) -> Result<String, LLMError> {
    // The model ID becomes part of the URL path.
    if model.is_empty()
        || !model
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '-' | '_'))
    {
        return Err(LLMError::invalid_request(format!(
            "Model ID '{model}' cannot be sent to Gemini"
        )));
    }
    Ok(if stream {
        format!("models/{model}:streamGenerateContent?alt=sse")
    } else {
        format!("models/{model}:generateContent")
    })
}

/// The `generateContent` body for `messages` and `tools`; generation settings are added by
/// the caller under `generationConfig`.
pub(super) fn gemini_body(
    messages: &[ChatMessage],
    tools: Option<&[Tool]>,
) -> Result<Value, LLMError> {
    // A function response names its function; tool results carry the call's ID.
    let mut names = HashMap::new();
    for message in messages {
        if let MessageType::ToolUse(calls) | MessageType::ToolResult(calls) = &message.message_type
        {
            for call in calls {
                names.insert(call.id.clone(), call.function.name.clone());
            }
        }
    }
    let mut system = Vec::new();
    let mut contents: Vec<Value> = Vec::new();
    for message in to_wire_messages(messages)? {
        let (role, parts) = if message.role == "system" {
            system.push(message.content.unwrap_or_default());
            continue;
        } else if let Some(id) = message.tool_call_id {
            let name = names.get(&id).cloned().unwrap_or_default();
            let result = message.content.unwrap_or_default();
            (
                "user",
                vec![
                    json!({"functionResponse": {"id": id, "name": name, "response": {"result": result}}}),
                ],
            )
        } else if message.role == "assistant" {
            ("model", model_parts(message.content, message.tool_calls)?)
        } else {
            let text = message.content.unwrap_or_default();
            ("user", vec![json!({"text": text})])
        };
        if parts.is_empty() {
            continue;
        }
        // Consecutive turns of one role (parallel tool results) form one content.
        match contents.last_mut() {
            Some(last) if last["role"] == role => {
                if let Some(known) = last["parts"].as_array_mut() {
                    known.extend(parts);
                }
            }
            _ => contents.push(json!({"role": role, "parts": parts})),
        }
    }
    let mut body = json!({"contents": contents, "generationConfig": {}});
    if !system.is_empty() {
        body["systemInstruction"] = json!({"parts": [{"text": system.join("\n\n")}]});
    }
    if let Some(tools) = tools.filter(|tools| !tools.is_empty()) {
        let declarations: Vec<Value> = tools
            .iter()
            .map(|tool| json!({"name": tool.function.name, "description": tool.function.description, "parametersJsonSchema": tool.function.parameters}))
            .collect();
        body["tools"] = json!([{"functionDeclarations": declarations}]);
    }
    Ok(body)
}

/// A model turn's parts: its text, then its function calls, with the step's signature on
/// the first call (or on the text of a final answer).
fn model_parts(text: Option<String>, calls: Option<Vec<ToolCall>>) -> Result<Vec<Value>, LLMError> {
    let calls = calls.unwrap_or_default();
    let kept = if calls.is_empty() {
        text.as_deref().and_then(reasoning::replay_answer)
    } else {
        reasoning::replay(&calls)
    };
    let signature = kept.and_then(|kept| kept.signature);
    let mut parts = Vec::new();
    if let Some(text) = text.filter(|text| !text.is_empty()) {
        let mut part = json!({"text": text});
        if let (true, Some(signature)) = (calls.is_empty(), &signature) {
            part["thoughtSignature"] = signature.as_str().into();
        }
        parts.push(part);
    }
    for (index, call) in calls.into_iter().enumerate() {
        let args: Value = serde_json::from_str(&call.function.arguments)?;
        if !args.is_object() {
            return Err(LLMError::invalid_request("Tool input must be an object"));
        }
        let mut part =
            json!({"functionCall": {"id": call.id, "name": call.function.name, "args": args}});
        if index == 0 {
            part["thoughtSignature"] = signature.as_deref().unwrap_or(SKIP_SIGNATURE).into();
        }
        parts.push(part);
    }
    Ok(parts)
}

/// Gemini's finish reason in Chat Completions terms; a reply Gemini stopped for another
/// reason (safety, a malformed call) is an error.
fn finish_reason(reason: Option<&str>) -> Result<Option<&'static str>, LLMError> {
    match reason {
        None => Ok(None),
        Some("STOP" | "FINISH_REASON_UNSPECIFIED") => Ok(Some("stop")),
        Some("MAX_TOKENS") => Ok(Some("length")),
        Some(other) => Err(LLMError::Generic(format!(
            "Gemini stopped the reply: {other}"
        ))),
    }
}

/// The text, tool calls and first signature in a candidate's parts. Thought summaries are
/// skipped: only the signature goes back.
fn read_parts(value: &Value, first_index: usize) -> (String, Vec<Value>, Option<String>) {
    let mut text = String::new();
    let mut calls = Vec::new();
    let mut signature = None;
    let parts = value["candidates"][0]["content"]["parts"].as_array();
    for part in parts.into_iter().flatten() {
        if signature.is_none() {
            signature = part["thoughtSignature"].as_str().map(str::to_owned);
        }
        if part["thought"] == true {
            continue;
        }
        if let Some(part_text) = part["text"].as_str() {
            text.push_str(part_text);
        }
        let call = &part["functionCall"];
        if call.is_object() {
            let id = call["id"].as_str().filter(|id| !id.is_empty()).map_or_else(
                || {
                    format!(
                        "gemini_call_{}",
                        CALL_COUNTER.fetch_add(1, Ordering::Relaxed)
                    )
                },
                str::to_owned,
            );
            let args = if call["args"].is_object() {
                call["args"].to_string()
            } else {
                "{}".to_owned()
            };
            calls.push(json!({"index": first_index + calls.len(), "id": id, "type": "function", "function": {"name": call["name"], "arguments": args}}));
        }
    }
    (text, calls, signature)
}

/// `usageMetadata` as Chat Completions usage. Gemini counts thinking apart from the reply.
pub(super) fn gemini_usage(metadata: &Value) -> Value {
    if !metadata.is_object() {
        return Value::Null;
    }
    let count = |key: &str| metadata[key].as_u64().unwrap_or(0);
    let thoughts = count("thoughtsTokenCount");
    let completion = count("candidatesTokenCount") + thoughts;
    json!({
        "prompt_tokens": count("promptTokenCount"),
        "completion_tokens": completion,
        "total_tokens": count("totalTokenCount").max(count("promptTokenCount") + completion),
        "completion_tokens_details": {"reasoning_tokens": thoughts},
        "prompt_tokens_details": {"cached_tokens": count("cachedContentTokenCount")},
    })
}

/// A complete `generateContent` reply, normalized.
pub(super) fn gemini_response(value: Value) -> Result<WireChatResponse, LLMError> {
    if let Some(error) = value.get("error") {
        return Err(LLMError::Generic(format!("Provider error: {error}")));
    }
    let finish = finish_reason(value["candidates"][0]["finishReason"].as_str())?;
    if finish == Some("length") {
        return Err(LLMError::Generic("Provider response was truncated".into()));
    }
    let (text, calls, signature) = read_parts(&value, 0);
    if text.is_empty() && calls.is_empty() {
        return Err(LLMError::Generic(
            "Provider returned no text or tools".into(),
        ));
    }
    let calls = calls
        .into_iter()
        .map(serde_json::from_value)
        .collect::<Result<Vec<ToolCall>, _>>()?;
    Ok(WireChatResponse {
        choices: vec![WireChoice {
            finish_reason: finish.map(str::to_owned),
            message: WireChoiceMessage {
                content: Some(text),
                tool_calls: Some(calls),
                reasoning_content: None,
                reasoning: None,
                reasoning_details: Vec::new(),
                thinking_blocks: Vec::new(),
                reasoning_items: Vec::new(),
                thought_signature: signature,
            },
        }],
        usage: serde_json::from_value(gemini_usage(&value["usageMetadata"])).ok(),
    })
}

/// One streamed `generateContent` chunk as a Chat Completions delta. Gemini sends each
/// function call whole, so calls are numbered from `first_index`, the calls seen so far.
pub(super) fn gemini_stream_event(value: Value, first_index: usize) -> Result<Value, LLMError> {
    let finish = finish_reason(value["candidates"][0]["finishReason"].as_str())?;
    let (text, calls, signature) = read_parts(&value, first_index);
    let mut delta = json!({"content": text});
    if !calls.is_empty() {
        delta["tool_calls"] = calls.into();
    }
    if let Some(signature) = signature {
        delta["thought_signature"] = signature.into();
    }
    Ok(json!({"choices": [{"delta": delta, "finish_reason": finish}]}))
}
