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
    mount_script_with_summary(server, bodies, None).await;
}

/// Compaction can issue any number of section/merge requests without advancing the task script.
pub async fn mount_script_with_summary(
    server: &MockServer,
    bodies: Vec<Value>,
    summary: Option<&str>,
) {
    mount_script_scripted(server, bodies, summary, None, 0).await;
}

/// Like `mount_script_with_summary`, but ordinary answers report `usage.prompt_tokens`
/// as `usage_multiplier` times the request body's bytes / 4 (the runtime's own estimate
/// basis), and the first `overflows` ordinary requests fail with a context-length 400.
pub async fn mount_script_scripted(
    server: &MockServer,
    bodies: Vec<Value>,
    summary: Option<&str>,
    usage_multiplier: Option<f64>,
    overflows: usize,
) {
    assert!(!bodies.is_empty(), "script needs at least one response");
    let bodies = Arc::new(bodies);
    let count = Arc::new(AtomicUsize::new(0));
    let attempts = Arc::new(AtomicUsize::new(0));
    let summary = summary.map(|text| format!("Goal/constraints: preserve the request. Confirmed findings: {text} Completed changes/tests: observed actions only. Unresolved work: finish the request. Next action: continue from recorded state."));
    Mock::given(method("POST"))
        .and(path("/chat/completions"))
        .respond_with(move |request: &wiremock::Request| {
            if let Some(summary) = &summary {
                let input: Value = request.body_json().unwrap();
                if input["messages"][0]["content"]
                    .as_str()
                    .is_some_and(|text| text.contains("You summarize agent context"))
                {
                    return ResponseTemplate::new(200).set_body_json(final_text_body(summary));
                }
            }
            if attempts.fetch_add(1, Ordering::SeqCst) < overflows {
                return ResponseTemplate::new(400).set_body_json(json!({"error": {
                    "code": "context_length_exceeded",
                    "message": "This model's maximum context length is 8192 tokens."
                }}));
            }
            let index = count.fetch_add(1, Ordering::SeqCst);
            let mut body = bodies
                .get(index)
                .unwrap_or_else(|| bodies.last().expect("non-empty script"))
                .clone();
            if let Some(multiplier) = usage_multiplier {
                let prompt = (request.body.len() as f64 / 4.0 * multiplier) as u64;
                body["usage"] = json!({"prompt_tokens": prompt, "completion_tokens": 5,
                    "total_tokens": prompt + 5});
            }
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
