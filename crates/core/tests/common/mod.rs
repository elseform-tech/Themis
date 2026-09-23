//! Shared helpers for themis-core integration tests: scripted mock-LLM
//! (wiremock) chat-completions responses and event collection.
//!
//! Compiled into each integration-test target; not every target uses every
//! helper.
#![allow(dead_code)]

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;

use serde_json::{json, Value};
use themis_core::runtime::RunEvent;
use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

/// OpenAI-shaped final-text response body.
pub fn final_text_body(text: &str) -> Value {
    json!({
        "id": "chatcmpl-test",
        "object": "chat.completion",
        "choices": [{
            "index": 0,
            "message": {"role": "assistant", "content": text},
            "finish_reason": "stop"
        }]
    })
}

/// OpenAI-shaped single-tool-call response body.
pub fn tool_call_body(id: &str, name: &str, args: Value) -> Value {
    json!({
        "id": "chatcmpl-test",
        "object": "chat.completion",
        "choices": [{
            "index": 0,
            "message": {
                "role": "assistant",
                "content": Value::Null,
                "tool_calls": [{
                    "id": id,
                    "type": "function",
                    "function": {
                        "name": name,
                        "arguments": serde_json::to_string(&args).unwrap()
                    }
                }]
            },
            "finish_reason": "tool_calls"
        }]
    })
}

/// Serves `POST {server}/chat/completions` with `bodies` in request order.
///
/// The last body repeats for any further requests, so a script of one tool
/// call models a stubborn agent without hanging the run (bounded by
/// `max_turns`). Deterministic: no sleeps, no randomness.
pub async fn mount_script(server: &MockServer, bodies: Vec<Value>) {
    assert!(!bodies.is_empty(), "script needs at least one response");
    let bodies = Arc::new(bodies);
    let count = Arc::new(AtomicUsize::new(0));
    Mock::given(method("POST"))
        .and(path("/chat/completions"))
        .respond_with(move |_: &wiremock::Request| {
            let index = count.fetch_add(1, Ordering::SeqCst);
            let body = bodies
                .get(index)
                .unwrap_or_else(|| bodies.last().expect("non-empty script"))
                .clone();
            ResponseTemplate::new(200).set_body_json(body)
        })
        .mount(server)
        .await;
}

/// Collects every buffered event after the run's sender was dropped.
pub async fn drain(rx: &mut tokio::sync::mpsc::Receiver<RunEvent>) -> Vec<RunEvent> {
    let mut events = Vec::new();
    while let Ok(event) = rx.try_recv() {
        events.push(event);
    }
    events
}
