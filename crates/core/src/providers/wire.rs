use super::*;

/// OpenAI chat-completions message (owned subset sufficient for text + tools).
#[derive(Serialize)]
pub(super) struct WireMessage {
    pub(super) role: &'static str,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(super) content: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(super) tool_calls: Option<Vec<ToolCall>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(super) tool_call_id: Option<String>,
}

/// OpenAI chat-completions request body.
#[derive(Serialize)]
pub(super) struct WireChatRequest<'a> {
    pub(super) model: &'a str,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(super) reasoning_effort: Option<&'a str>,
    pub(super) messages: Vec<WireMessage>,
    pub(super) stream: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(super) max_tokens: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(super) temperature: Option<f32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(super) tools: Option<&'a [Tool]>,
}

/// OpenAI chat-completions response body (subset).
#[derive(Deserialize)]
pub(super) struct WireChatResponse {
    #[serde(default)]
    pub(super) choices: Vec<WireChoice>,
    #[serde(default)]
    pub(super) usage: Option<Usage>,
}

/// Single chat choice of [`WireChatResponse`].
#[derive(Deserialize)]
pub(super) struct WireChoice {
    pub(super) message: WireChoiceMessage,
}

/// Assistant message of [`WireChoice`].
#[derive(Deserialize)]
pub(super) struct WireChoiceMessage {
    #[serde(default)]
    pub(super) content: Option<String>,
    #[serde(default)]
    pub(super) tool_calls: Option<Vec<ToolCall>>,
}

/// Converts SDK messages to the OpenAI wire shape.
///
/// Text becomes a plain message; `ToolUse` becomes an assistant message carrying
/// tool calls; each `ToolResult` entry becomes a `tool`-role message. Image and
/// PDF inputs are rejected: this provider is text-first for the coding agent.
pub(super) fn to_wire_messages(messages: &[ChatMessage]) -> Result<Vec<WireMessage>, LLMError> {
    let mut wire = Vec::with_capacity(messages.len());
    for message in messages {
        match &message.message_type {
            MessageType::Text => {
                let role = match &message.role {
                    ChatRole::System => "system",
                    ChatRole::Assistant => "assistant",
                    ChatRole::Tool => "user",
                    ChatRole::User => "user",
                };
                wire.push(WireMessage {
                    role,
                    content: Some(message.content.clone()),
                    tool_calls: None,
                    tool_call_id: None,
                });
            }
            MessageType::ToolUse(calls) => wire.push(WireMessage {
                role: "assistant",
                content: if message.content.is_empty() {
                    None
                } else {
                    Some(message.content.clone())
                },
                tool_calls: Some(calls.clone()),
                tool_call_id: None,
            }),
            MessageType::ToolResult(results) => {
                for result in results {
                    wire.push(WireMessage {
                        role: "tool",
                        content: Some(result.function.arguments.clone()),
                        tool_calls: None,
                        tool_call_id: Some(result.id.clone()),
                    });
                }
            }
            MessageType::Image(_) | MessageType::Pdf(_) | MessageType::ImageURL(_) => {
                return Err(LLMError::invalid_request(
                    "CompatibleProvider supports text and tool messages only".to_string(),
                ));
            }
        }
    }
    Ok(wire)
}

pub(super) fn responses_response(value: serde_json::Value) -> Result<WireChatResponse, LLMError> {
    if matches!(value["status"].as_str(), Some("failed" | "incomplete")) {
        return Err(LLMError::Generic(
            "Provider response failed or was incomplete".to_owned(),
        ));
    }
    let output = value["output"]
        .as_array()
        .ok_or_else(|| LLMError::Generic("Missing Responses output".to_owned()))?;
    let mut text = String::new();
    let mut calls = Vec::new();
    for item in output {
        if item["type"] == "function_call" {
            calls.push(serde_json::json!({"id":item["call_id"],"type":"function","function":{"name":item["name"],"arguments":item["arguments"]}}));
        }
        for content in item["content"].as_array().into_iter().flatten() {
            if let Some(part) = content["text"].as_str() {
                text.push_str(part);
            }
        }
    }
    if text.is_empty() && calls.is_empty() {
        return Err(LLMError::Generic(
            "Provider returned no text or tools".to_owned(),
        ));
    }
    Ok(serde_json::from_value(
        serde_json::json!({"choices":[{"message":{"content":text,"tool_calls":calls}}],"usage":value["usage"]}),
    )?)
}

pub(super) fn responses_stream_event(value: serde_json::Value) -> serde_json::Value {
    use serde_json::json;
    let index = &value["output_index"];
    match value["type"].as_str().unwrap_or_default() {
        "response.output_text.delta" => json!({"choices":[{"delta":{"content":value["delta"]}}]}),
        "response.output_item.added" if value["item"]["type"] == "function_call" => {
            json!({"choices":[{"delta":{"tool_calls":[{"index":index,"id":value["item"]["call_id"],"function":{"name":value["item"]["name"],"arguments":value["item"]["arguments"]}}]}}]})
        }
        "response.function_call_arguments.delta" => {
            json!({"choices":[{"delta":{"tool_calls":[{"index":index,"function":{"arguments":value["delta"]}}]}}]})
        }
        "response.completed" => json!({"choices":[{"delta":{},"finish_reason":"stop"}]}),
        _ => serde_json::Value::Null,
    }
}

/// Go's model families use three distinct wire protocols (docs/go/#endpoints).
pub(super) fn go_chat_path(model: &str) -> &'static str {
    if model.starts_with("muse-") || model.starts_with("gpt-") || model.starts_with("grok-") {
        "responses"
    } else if model.starts_with("qwen") || model.starts_with("minimax-") {
        "messages"
    } else {
        CHAT_COMPLETIONS_PATH
    }
}

pub(super) fn messages_body(
    model: &str,
    messages: &[ChatMessage],
    tools: Option<&[Tool]>,
    streaming: bool,
) -> Result<serde_json::Value, LLMError> {
    use serde_json::json;
    let mut system = Vec::new();
    let mut input = Vec::new();
    for message in to_wire_messages(messages)? {
        if message.role == "system" {
            system.push(message.content.unwrap_or_default());
            continue;
        }
        let mut content = Vec::new();
        if let Some(id) = message.tool_call_id {
            content.push(json!({"type": "tool_result", "tool_use_id": id, "content": message.content.unwrap_or_default()}));
        } else {
            if let Some(text) = message.content.filter(|text| !text.is_empty()) {
                content.push(json!({"type": "text", "text": text}));
            }
            for call in message.tool_calls.into_iter().flatten() {
                let arguments: serde_json::Value = serde_json::from_str(&call.function.arguments)?;
                if !arguments.is_object() {
                    return Err(LLMError::invalid_request("Tool input must be an object"));
                }
                content.push(json!({"type": "tool_use", "id": call.id, "name": call.function.name, "input": arguments}));
            }
        }
        input.push(json!({"role": if message.role == "assistant" { "assistant" } else { "user" }, "content": content}));
    }
    let mut body =
        json!({"model": model, "messages": input, "max_tokens": 16384, "stream": streaming});
    if !system.is_empty() {
        body["system"] = json!(system.join("\n\n"));
    }
    if let Some(tools) = tools.filter(|tools| !tools.is_empty()) {
        body["tools"] = json!(tools.iter().map(|tool| json!({"name": tool.function.name, "description": tool.function.description, "input_schema": tool.function.parameters})).collect::<Vec<_>>());
    }
    Ok(body)
}

pub(super) fn messages_response(value: serde_json::Value) -> Result<WireChatResponse, LLMError> {
    use serde_json::json;
    if let Some(error) = value.get("error") {
        return Err(LLMError::Generic(format!("Provider error: {error}")));
    }
    if value["stop_reason"] == "max_tokens" {
        return Err(LLMError::Generic("Provider response was truncated".into()));
    }
    let content = value["content"]
        .as_array()
        .ok_or_else(|| LLMError::Generic("Messages response missing content".into()))?;
    let mut text = String::new();
    let mut calls = Vec::new();
    for block in content {
        match block["type"].as_str() {
            Some("text") => text.push_str(
                block["text"]
                    .as_str()
                    .ok_or_else(|| LLMError::Generic("Invalid text block".into()))?,
            ),
            Some("tool_use") => {
                if !block["input"].is_object() {
                    return Err(LLMError::Generic("Invalid tool input".into()));
                }
                calls.push(serde_json::from_value(json!({"id": block["id"], "type": "function", "function": {"name": block["name"], "arguments": block["input"].to_string()}}))?);
            }
            _ => {}
        }
    }
    if text.is_empty() && calls.is_empty() {
        return Err(LLMError::Generic(
            "Provider returned no text or tools".into(),
        ));
    }
    Ok(WireChatResponse {
        choices: vec![WireChoice {
            message: WireChoiceMessage {
                content: Some(text),
                tool_calls: Some(calls),
            },
        }],
        usage: None,
    })
}

/// Normalize Messages events into the existing text/tool stream accumulator.
pub(super) fn messages_stream_event(value: serde_json::Value) -> serde_json::Value {
    use serde_json::json;
    let index = &value["index"];
    match value["type"].as_str() {
        Some("content_block_start") if value["content_block"]["type"] == "tool_use" => {
            let block = &value["content_block"];
            json!({"choices": [{"delta": {"tool_calls": [{"index": index, "id": block["id"], "function": {"name": block["name"], "arguments": if block["input"].as_object().is_some_and(|input| !input.is_empty()) { block["input"].to_string() } else { String::new() }}}]}}]})
        }
        Some("content_block_start") if value["content_block"]["type"] == "text" => {
            json!({"choices": [{"delta": {"content": value["content_block"]["text"]}}]})
        }
        Some("content_block_delta") if value["delta"]["type"] == "text_delta" => {
            json!({"choices": [{"delta": {"content": value["delta"]["text"]}}]})
        }
        Some("content_block_delta") if value["delta"]["type"] == "input_json_delta" => {
            json!({"choices": [{"delta": {"tool_calls": [{"index": index, "function": {"arguments": value["delta"]["partial_json"]}}]}}]})
        }
        Some("message_delta") if value["delta"]["stop_reason"].is_string() => {
            json!({"choices": [{"finish_reason": if value["delta"]["stop_reason"] == "max_tokens" { json!("length") } else { value["delta"]["stop_reason"].clone() }}]})
        }
        _ => value,
    }
}

/// `ChatResponse` implementation returned by [`CompatibleProvider`].
#[derive(Debug)]
pub(super) struct CompatibleChatResponse {
    pub(super) text: Option<String>,
    pub(super) tool_calls: Option<Vec<ToolCall>>,
    pub(super) usage: Option<Usage>,
}

impl ChatResponse for CompatibleChatResponse {
    fn text(&self) -> Option<String> {
        self.text.clone()
    }

    fn tool_calls(&self) -> Option<Vec<ToolCall>> {
        self.tool_calls.clone()
    }

    fn usage(&self) -> Option<Usage> {
        self.usage.clone()
    }
}

impl fmt::Display for CompatibleChatResponse {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if let Some(text) = &self.text {
            write!(f, "{text}")?;
        }
        if let Some(calls) = &self.tool_calls {
            for call in calls {
                write!(f, "{call}")?;
            }
        }
        Ok(())
    }
}
