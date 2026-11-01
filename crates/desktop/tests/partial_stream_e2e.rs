//! A run that fails mid-stream keeps its streamed text in the durable
//! transcript, so a reloaded window can show the partial answer.

use std::sync::Arc;
use std::time::Duration;

use serde_json::json;
use themis_desktop::sink::{ChannelSink, TestEvent};
use themis_desktop::state::AppState;
use themis_desktop::transcript::HistoryItem;
use themis_desktop::types::{ProviderKind, ThreadEvent};
use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

#[allow(dead_code)]
mod common;

#[tokio::test(flavor = "multi_thread")]
async fn streamed_text_before_a_failure_is_persisted_before_the_failed_event() {
    let chunk = json!({"choices":[{"delta":{"content":"Partial answer so far"},"index":0}]});
    let body = format!("data: {chunk}\n\ndata: {{not json\n\n");
    let server = MockServer::builder().start().await;
    Mock::given(method("POST"))
        .and(path("/chat/completions"))
        .respond_with(ResponseTemplate::new(200).set_body_raw(body, "text/event-stream"))
        .mount(&server)
        .await;
    let (_repo, project) = common::init_repo();
    let dir = tempfile::tempdir().unwrap();
    let state = AppState::new_for_test(dir.path().join("settings.json"));
    common::use_mock_llm(&state, &server).await;
    let thread = state
        .create_thread(
            project.to_string_lossy().into_owned(),
            ProviderKind::Go,
            Some("test-model".to_owned()),
        )
        .await
        .unwrap();
    let (sink, mut rx) = ChannelSink::channel();
    state
        .send_message(Arc::new(sink), thread.id.clone(), "answer".into())
        .await
        .unwrap();
    let live = tokio::time::timeout(Duration::from_secs(60), async {
        let mut streamed = String::new();
        while let Some(event) = rx.recv().await {
            if let TestEvent::Thread(envelope) = event {
                match envelope.event {
                    ThreadEvent::AssistantText { text } => streamed.push_str(&text),
                    ThreadEvent::Finished { .. } | ThreadEvent::Failed { .. } => break,
                    _ => {}
                }
            }
        }
        streamed
    })
    .await
    .expect("run ended");
    assert_eq!(live, "Partial answer so far");
    let history = state.get_thread_history(&thread.id).await.unwrap();
    let kinds: Vec<&ThreadEvent> = history
        .iter()
        .filter_map(|item| match item {
            HistoryItem::Event { envelope } => Some(&envelope.event),
            _ => None,
        })
        .collect();
    let text_at = kinds
        .iter()
        .position(
            |e| matches!(e, ThreadEvent::AssistantText { text } if text == "Partial answer so far"),
        )
        .expect("partial text persisted");
    let failed_at = kinds
        .iter()
        .position(|e| matches!(e, ThreadEvent::Failed { .. }))
        .expect("failure persisted");
    assert!(text_at < failed_at, "{kinds:?}");
}
