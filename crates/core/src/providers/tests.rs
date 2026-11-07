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

mod catalog;
mod reasoning_and_tools;
mod transport;
