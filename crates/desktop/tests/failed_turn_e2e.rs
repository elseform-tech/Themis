//! A run that fails leaves a note in the conversation, so the next request does not carry the
//! failed request as an unanswered message. Without it, every failed turn stacked one more user
//! message: a Vision whose wakes failed sent five unanswered wakes in a row (live check
//! 2026-11-13).

use std::sync::Arc;
use std::time::Duration;

#[allow(dead_code)]
mod common;
use common::{final_text_body, init_repo};
use serde_json::{json, Value};
use themis_desktop::sink::{ChannelSink, TestEvent};
use themis_desktop::state::AppState;
use themis_desktop::types::{ProviderKind, ThreadEvent};
use tokio::sync::mpsc::UnboundedReceiver;
use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, Request, ResponseTemplate};

/// Answers every request, except one whose newest user message says BREAK: that gets an HTTP
/// 400 the runtime does not retry.
async fn mount_breakable_llm() -> MockServer {
    let server = MockServer::builder().start().await;
    Mock::given(method("POST"))
        .and(path("/chat/completions"))
        .respond_with(|request: &Request| {
            let body: Value = serde_json::from_slice(&request.body).unwrap_or(Value::Null);
            let newest_user = body["messages"]
                .as_array()
                .and_then(|messages| messages.iter().rev().find(|m| m["role"] == "user"))
                .and_then(|message| message["content"].as_str())
                .unwrap_or_default()
                .to_owned();
            if newest_user.contains("BREAK") {
                ResponseTemplate::new(400).set_body_json(json!({
                    "error": {"type": "invalid_request_error", "message": "refused for the test"}
                }))
            } else {
                ResponseTemplate::new(200).set_body_json(final_text_body())
            }
        })
        .mount(&server)
        .await;
    server
}

/// Waits for the run to end; whether it failed.
async fn run_end(events: &mut UnboundedReceiver<TestEvent>) -> bool {
    tokio::time::timeout(Duration::from_secs(20), async {
        while let Some(event) = events.recv().await {
            if let TestEvent::Thread(envelope) = event {
                match envelope.event {
                    ThreadEvent::Failed { .. } => return true,
                    ThreadEvent::Finished { .. } => return false,
                    _ => {}
                }
            }
        }
        panic!("missing terminal event");
    })
    .await
    .unwrap()
}

/// The chat messages (role, text) of the last request the provider received.
async fn last_request_messages(server: &MockServer) -> Vec<(String, String)> {
    let requests = server.received_requests().await.unwrap();
    let body: Value = serde_json::from_slice(&requests.last().unwrap().body).unwrap();
    body["messages"]
        .as_array()
        .unwrap()
        .iter()
        .map(|message| {
            let text = match &message["content"] {
                Value::String(text) => text.clone(),
                other => other.to_string(),
            };
            (
                message["role"].as_str().unwrap_or_default().to_owned(),
                text,
            )
        })
        .collect()
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_turn_after_a_failure_sees_that_the_earlier_reply_failed() {
    let (_repo, root) = init_repo();
    let settings = tempfile::tempdir().unwrap();
    let state = AppState::new_for_test(settings.path().join("settings.json"));
    let server = mount_breakable_llm().await;
    state
        .set_secret("go".to_owned(), "test-key".to_owned())
        .await
        .unwrap();
    state.set_go_base_url_override(Some(server.uri()));
    let thread = state
        .create_thread(
            root.to_string_lossy().into_owned(),
            ProviderKind::Go,
            Some("test-model".to_owned()),
        )
        .await
        .unwrap();
    let (sink, mut events) = ChannelSink::channel();
    let sink = Arc::new(sink);

    state
        .send_message(sink.clone(), thread.id.clone(), "BREAK this one".to_owned())
        .await
        .unwrap();
    assert!(run_end(&mut events).await, "the first run must fail");

    state
        .send_message(sink.clone(), thread.id.clone(), "say hello".to_owned())
        .await
        .unwrap();
    assert!(!run_end(&mut events).await, "the second run must finish");

    let messages = last_request_messages(&server).await;
    let first = messages
        .iter()
        .position(|(role, text)| role == "user" && text.contains("BREAK this one"))
        .expect("failed request in context");
    let second = messages
        .iter()
        .position(|(role, text)| role == "user" && text.contains("say hello"))
        .expect("second request in context");
    let between: Vec<&(String, String)> = messages[first + 1..second].iter().collect();
    assert!(
        between
            .iter()
            .any(|(role, text)| role == "assistant" && text.contains("failed")),
        "the failure must show between the two requests, got {between:?}"
    );
    assert!(
        !between
            .iter()
            .any(|(_, text)| text.contains("refused for the test")),
        "the note must not carry the provider's error text, got {between:?}"
    );
}
