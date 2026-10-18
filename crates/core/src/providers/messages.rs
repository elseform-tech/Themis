//! Anthropic Messages: Claude, Qwen and MiniMax on OpenCode Go.

use super::*;

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
            // A thinking model's blocks go first, unchanged (signatures must round-trip).
            let kept = match message.tool_calls.as_deref() {
                Some(calls) => super::reasoning::replay(calls),
                // Claude binds a thinking block to the exact conversation before it, and a
                // later message's history is shorter, so its answers go back without it.
                None if message.role == "assistant" && !model.starts_with("claude-") => message
                    .content
                    .as_deref()
                    .and_then(super::reasoning::replay_answer),
                None => None,
            };
            if let Some(kept) = kept {
                content.extend(kept.blocks);
            }
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
    let mut thinking_blocks = Vec::new();
    for block in content {
        match block["type"].as_str() {
            Some("thinking" | "redacted_thinking") => thinking_blocks.push(block.clone()),
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
            finish_reason: None,
            message: WireChoiceMessage {
                content: Some(text),
                tool_calls: Some(calls),
                reasoning_content: None,
                reasoning: None,
                reasoning_details: Vec::new(),
                thinking_blocks,
                reasoning_items: Vec::new(),
                thought_signature: None,
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
        Some("content_block_start")
            if matches!(
                value["content_block"]["type"].as_str(),
                Some("thinking" | "redacted_thinking")
            ) =>
        {
            json!({"choices": [{"delta": {"thinking_block": value["content_block"]}}]})
        }
        Some("content_block_delta") if value["delta"]["type"] == "thinking_delta" => {
            json!({"choices": [{"delta": {"thinking_delta": value["delta"]["thinking"]}}]})
        }
        Some("content_block_delta") if value["delta"]["type"] == "signature_delta" => {
            json!({"choices": [{"delta": {"signature_delta": value["delta"]["signature"]}}]})
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
