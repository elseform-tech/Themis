//! A chat run survives OpenCode Go reporting a failure of the model behind it as HTTP 400
//! (`server_error`, "Upstream response was not valid JSON"): the request is retried and the
//! person gets the answer, not "LLM request failed". Found by a live check, where a Vision's
//! turn failed this way on a free model.

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::Duration;

#[allow(dead_code)]
mod common;
use common::init_repo;
use themis_desktop::sink::{ChannelSink, TestEvent};
use themis_desktop::state::AppState;
use themis_desktop::types::{ProviderKind, ThreadEvent};
use wiremock::matchers::method;
use wiremock::{Mock, MockServer, ResponseTemplate};

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_chat_run_retries_an_upstream_server_error_reported_as_400() {
    let (_repo, root) = init_repo();
    let settings = tempfile::tempdir().unwrap();
    let state = AppState::new_for_test(settings.path().join("settings.json"));
    let server = MockServer::builder().start().await;
    let calls = Arc::new(AtomicUsize::new(0));
    let seen = calls.clone();
    Mock::given(method("POST"))
        .respond_with(move |_: &wiremock::Request| {
            if seen.fetch_add(1, Ordering::SeqCst) == 0 {
                ResponseTemplate::new(400).set_body_json(serde_json::json!({"error": {
                    "type": "server_error", "code": "server_error",
                    "message": "Upstream request failed: [server_error] Upstream response was not valid JSON"
                }}))
            } else {
                ResponseTemplate::new(200).set_body_json(serde_json::json!({
                    "choices": [{"message": {"role": "assistant", "content": "Hi there."}, "finish_reason": "stop"}]
                }))
            }
        })
        .mount(&server)
        .await;
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
    state
        .send_message(Arc::new(sink), thread.id.clone(), "say hi".to_owned())
        .await
        .unwrap();
    let outcome = tokio::time::timeout(Duration::from_secs(30), async {
        while let Some(event) = events.recv().await {
            if let TestEvent::Thread(envelope) = event {
                match envelope.event {
                    ThreadEvent::Finished { result, .. } => return Ok(result),
                    ThreadEvent::Failed { error } => return Err(error),
                    _ => {}
                }
            }
        }
        Err("no terminal event".to_owned())
    })
    .await
    .expect("the run ends");
    assert_eq!(outcome.as_deref(), Ok("Hi there."));
    assert_eq!(calls.load(Ordering::SeqCst), 2);
}
