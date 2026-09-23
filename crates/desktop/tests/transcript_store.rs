use std::sync::Arc;
use themis_desktop::sink::{ChannelSink, TestEvent};
use themis_desktop::state::AppState;
use themis_desktop::transcript::{HistoryItem, TranscriptStore};
use themis_desktop::types::ProviderKind;
use themis_desktop::types::{ThreadEvent, ThreadEventEnvelope};
use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

#[test]
fn transcript_survives_reopen_and_keeps_threads_separate() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("sessions.sqlite3");
    let store = TranscriptStore::open(&path).unwrap();
    store
        .append_user("thread-a", "run-1", "Remember ORBIT-17")
        .unwrap();
    store
        .append_event(&ThreadEventEnvelope {
            thread_id: "thread-a".into(),
            run_id: "run-1".into(),
            event: ThreadEvent::ToolFinished {
                tool: "read_file".into(),
                ok: true,
                output: "saved tool result".into(),
            },
        })
        .unwrap();
    store
        .append_event(&ThreadEventEnvelope {
            thread_id: "thread-a".into(),
            run_id: "run-1".into(),
            event: ThreadEvent::Finished {
                result: "I will remember ORBIT-17".into(),
            },
        })
        .unwrap();
    store.append_user("thread-b", "run-2", "Unrelated").unwrap();
    drop(store);

    let reopened = TranscriptStore::open(&path).unwrap();
    let history = reopened.history("thread-a").unwrap();
    assert_eq!(history.len(), 3);
    assert!(matches!(&history[0], HistoryItem::User { text, .. } if text == "Remember ORBIT-17"));
    assert!(
        matches!(&history[1], HistoryItem::Event { envelope } if matches!(&envelope.event, ThreadEvent::ToolFinished { output, .. } if output == "saved tool result"))
    );
    assert!(
        matches!(&history[2], HistoryItem::Event { envelope } if matches!(&envelope.event, ThreadEvent::Finished { result } if result == "I will remember ORBIT-17"))
    );
    assert_eq!(reopened.history("thread-b").unwrap().len(), 1);
    assert_eq!(reopened.context("thread-a", 2).unwrap().len(), 2);
}

#[test]
fn legacy_display_messages_import_once_into_model_context() {
    let dir = tempfile::tempdir().unwrap();
    let store = TranscriptStore::open(&dir.path().join("sessions.sqlite3")).unwrap();
    let messages = vec![
        serde_json::json!({"id":"old-1","role":"user","text":"Remember ORBIT-17"}),
        serde_json::json!({"id":"old-2","role":"assistant","text":"Noted","final":true}),
    ];
    store.import_legacy("thread-a", &messages).unwrap();
    store.import_legacy("thread-a", &messages).unwrap();
    assert_eq!(store.history("thread-a").unwrap().len(), 2);
    assert_eq!(store.context("thread-a", 20).unwrap().len(), 2);
}

#[test]
fn checkpoint_replaces_old_model_context_but_preserves_full_history() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("sessions.sqlite3");
    let store = TranscriptStore::open(&path).unwrap();
    store.append_user("thread", "run", "Original task").unwrap();
    store
        .append_user("thread", "run", "Middle request")
        .unwrap();
    store
        .append_user("thread", "run", "Latest request")
        .unwrap();
    store
        .append_event(&ThreadEventEnvelope {
            thread_id: "thread".into(),
            run_id: "run".into(),
            event: ThreadEvent::ContextCheckpoint {
                summary: "Original task; read note.txt".into(),
            },
        })
        .unwrap();
    store
        .append_event(&ThreadEventEnvelope {
            thread_id: "thread".into(),
            run_id: "run".into(),
            event: ThreadEvent::Finished {
                result: "Done".into(),
            },
        })
        .unwrap();
    drop(store);
    let reopened = TranscriptStore::open(&path).unwrap();
    assert_eq!(reopened.history("thread").unwrap().len(), 5);
    let context = reopened.context("thread", 20).unwrap();
    assert_eq!(context.len(), 2);
    assert!(context[0].text.contains("Original task; read note.txt"));
    assert!(context[0]
        .text
        .contains("Verbatim first and recent user requests (historical):\nOriginal task"));
    assert!(context[0].text.contains("Middle request\nLatest request"));
    assert_eq!(context[1].text, "Done");
}

#[tokio::test]
async fn followup_after_restart_receives_prior_conversation() {
    let server = MockServer::start().await;
    Mock::given(method("POST")).and(path("/chat/completions"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "choices": [{"message": {"role": "assistant", "content": "Noted"}, "finish_reason": "stop"}]
        }))).mount(&server).await;
    let app = tempfile::tempdir().unwrap();
    let project = tempfile::tempdir().unwrap();
    let settings = app.path().join("settings.json");
    let first = AppState::new_for_test(settings.clone());
    first
        .open_project(project.path().to_string_lossy().into_owned())
        .await
        .unwrap();
    first
        .set_secret("custom".into(), "synthetic-test-key".into())
        .await
        .unwrap();
    first.set_custom_base_url_override(Some(server.uri()));
    let thread = first
        .create_thread(
            project.path().to_string_lossy().into_owned(),
            ProviderKind::Custom,
            Some("test-model".into()),
        )
        .await
        .unwrap();
    let (sink, mut rx) = ChannelSink::channel();
    first
        .send_message(
            Arc::new(sink),
            thread.id.clone(),
            "Remember ORBIT-17".into(),
        )
        .await
        .unwrap();
    loop {
        if let Some(TestEvent::Thread(envelope)) = rx.recv().await {
            if matches!(envelope.event, ThreadEvent::Finished { .. }) {
                break;
            }
        }
    }
    drop(first);
    let second = AppState::new_for_test(settings);
    second
        .set_secret("custom".into(), "synthetic-test-key".into())
        .await
        .unwrap();
    second.set_custom_base_url_override(Some(server.uri()));
    assert_eq!(
        second.get_thread_history(&thread.id).await.unwrap().len(),
        4
    );
    let (sink, mut rx) = ChannelSink::channel();
    second
        .send_message(Arc::new(sink), thread.id.clone(), "What code?".into())
        .await
        .unwrap();
    loop {
        if let Some(TestEvent::Thread(envelope)) = rx.recv().await {
            if matches!(envelope.event, ThreadEvent::Finished { .. }) {
                break;
            }
        }
    }
    let requests = server.received_requests().await.unwrap();
    let body: serde_json::Value = serde_json::from_slice(&requests[1].body).unwrap();
    let messages = body["messages"].as_array().unwrap();
    assert!(messages
        .iter()
        .any(|message| message["role"] == "user" && message["content"] == "Remember ORBIT-17"));
    assert!(messages
        .iter()
        .any(|message| message["role"] == "assistant" && message["content"] == "Noted"));
}
