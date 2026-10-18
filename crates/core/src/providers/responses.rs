//! OpenAI Responses: GPT, Grok and Muse on OpenCode Go.

use super::*;

pub(super) fn responses_response(value: serde_json::Value) -> Result<WireChatResponse, LLMError> {
    if value["status"] == "incomplete"
        && value["incomplete_details"]["reason"] == "max_output_tokens"
    {
        return Err(LLMError::Generic("Provider response was truncated".into()));
    }
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
    let reasoning_items: Vec<_> = output
        .iter()
        .filter(|item| item["type"] == "reasoning")
        .cloned()
        .collect();
    for item in output.iter().filter(|item| item["type"] != "reasoning") {
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
        serde_json::json!({"choices":[{"message":{"content":text,"tool_calls":calls,"reasoning_items":reasoning_items}}],"usage":value["usage"]}),
    )?)
}

pub(super) fn responses_stream_event(value: serde_json::Value) -> serde_json::Value {
    use serde_json::json;
    let index = &value["output_index"];
    match value["type"].as_str().unwrap_or_default() {
        "response.output_text.delta" => json!({"choices":[{"delta":{"content":value["delta"]}}]}),
        "response.output_item.done" if value["item"]["type"] == "reasoning" => {
            json!({"choices":[{"delta":{"reasoning_item":value["item"]}}]})
        }
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

/// The Responses request body: history as input items, each tool-call set after its own
/// reasoning, and tools as functions.
pub(super) fn responses_body(
    model: &str,
    messages: &[ChatMessage],
    tools: Option<&[Tool]>,
    stream: bool,
    effort: Option<&str>,
) -> Result<serde_json::Value, LLMError> {
    let mut input = Vec::new();
    for message in to_wire_messages(messages)? {
        if let Some(id) = message.tool_call_id {
            input.push(serde_json::json!({"type":"function_call_output", "call_id":id, "output":message.content.unwrap_or_default()}));
        } else {
            if message.role == "assistant" && message.tool_calls.is_none() {
                // An earlier final answer goes back after its own reasoning.
                if let Some(kept) = message
                    .content
                    .as_deref()
                    .and_then(reasoning::replay_answer)
                {
                    input.extend(kept.items);
                }
            }
            if let Some(content) = message.content {
                input.push(serde_json::json!({"role":message.role, "content":content}));
            }
            let calls = message.tool_calls.unwrap_or_default();
            if let Some(kept) = reasoning::replay(&calls) {
                input.extend(kept.items);
            }
            for call in calls {
                input.push(serde_json::json!({"type":"function_call", "call_id":call.id, "name":call.function.name, "arguments":call.function.arguments}));
            }
        }
    }
    // Requests are not stored, so reasoning comes back encrypted to be replayed.
    let mut body = serde_json::json!({"model":model,"input":input,"stream":stream,"store":false,"include":["reasoning.encrypted_content"]});
    if let Some(tools) = tools {
        body["tools"] = tools.iter().map(|tool| serde_json::json!({"type":"function","name":tool.function.name,"description":tool.function.description,"parameters":tool.function.parameters,"strict":false})).collect();
    }
    if let Some(effort) = effort {
        body["reasoning"] = serde_json::json!({"effort":effort});
    }
    Ok(body)
}
