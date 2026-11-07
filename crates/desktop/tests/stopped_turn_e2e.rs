//! A run the person stops leaves a note in the conversation, so the next turn knows that
//! request was set aside instead of seeing it unanswered and doing it again.

use std::sync::Arc;
use std::time::Duration;

#[allow(dead_code)]
mod common;
use common::{init_repo, mount_scripted_llm};
use serde_json::Value;
use themis_desktop::sink::{ChannelSink, TestEvent};
use themis_desktop::state::AppState;
use themis_desktop::types::{ProviderKind, ThreadEvent};
use tokio::sync::mpsc::UnboundedReceiver;

/// Waits for the run's first approval and stops the run there.
async fn stop_at_first_approval(
    state: &AppState,
    thread_id: &str,
    events: &mut UnboundedReceiver<TestEvent>,
) {
    tokio::time::timeout(Duration::from_secs(10), async {
        while let Some(event) = events.recv().await {
            match event {
                TestEvent::Approval(_) => state.stop_thread(thread_id.to_owned()).await.unwrap(),
                TestEvent::Thread(envelope) => {
                    if let ThreadEvent::Failed { error } = envelope.event {
                        assert!(error.contains("Stopped by you"), "{error}");
                        return;
                    }
                }
                _ => {}
            }
        }
        panic!("missing terminal event");
    })
    .await
    .unwrap();
}

/// The chat messages (role, text) of the last request the provider received.
async fn last_request_messages(server: &wiremock::MockServer) -> Vec<(String, String)> {
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
async fn the_turn_after_a_stop_sees_that_the_earlier_request_was_stopped() {
    let (_repo, root) = init_repo();
    let settings = tempfile::tempdir().unwrap();
    let state = AppState::new_for_test(settings.path().join("settings.json"));
    let server = mount_scripted_llm().await;
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
        .send_message(
            sink.clone(),
            thread.id.clone(),
            "write hello.txt".to_owned(),
        )
        .await
        .unwrap();
    stop_at_first_approval(&state, &thread.id, &mut events).await;

    state
        .send_message(
            sink.clone(),
            thread.id.clone(),
            "now write bye.txt".to_owned(),
        )
        .await
        .unwrap();
    // The second run's first request reaches the provider before its approval shows.
    stop_at_first_approval(&state, &thread.id, &mut events).await;

    let messages = last_request_messages(&server).await;
    let first = messages
        .iter()
        .position(|(role, text)| role == "user" && text.contains("write hello.txt"))
        .expect("first request in context");
    let second = messages
        .iter()
        .position(|(role, text)| role == "user" && text.contains("now write bye.txt"))
        .expect("second request in context");
    let between: Vec<&(String, String)> = messages[first + 1..second].iter().collect();
    assert!(
        between
            .iter()
            .any(|(role, text)| role == "assistant" && text.contains("stopped")),
        "the stop must show between the two requests, got {between:?}"
    );
}
