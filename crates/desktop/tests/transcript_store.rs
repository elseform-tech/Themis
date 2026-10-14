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
                model: None,
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
        matches!(&history[2], HistoryItem::Event { envelope } if matches!(&envelope.event, ThreadEvent::Finished { result, .. } if result == "I will remember ORBIT-17"))
    );
    assert_eq!(reopened.history("thread-b").unwrap().len(), 1);
    assert_eq!(reopened.context("thread-a", 2).unwrap().len(), 2);
}

#[test]
fn request_aware_recall_recovers_old_topics_without_replacing_latest_task() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("sessions.sqlite3");
    let store = TranscriptStore::open(&path).unwrap();
    store
        .append_user(
            "thread",
            "old",
            "Let's discuss amber migration. Proposed key AMBER-794; no implementation yet.",
        )
        .unwrap();
    store
        .append_event(&ThreadEventEnvelope {
            thread_id: "thread".into(),
            run_id: "old".into(),
            event: ThreadEvent::Finished {
                model: None,
                result: "Proposal only: AMBER-794 uses ordered epochs.".into(),
            },
        })
        .unwrap();
    for i in 0..20 {
        store
            .append_user(
                "thread",
                &format!("aside-{i}"),
                &format!("Unrelated question {i}"),
            )
            .unwrap();
    }
    store
        .append_user(
            "thread",
            "latest",
            "Latest task: write cobalt narrative. Hero is Mira.",
        )
        .unwrap();
    store
        .append_event(&ThreadEventEnvelope {
            thread_id: "thread".into(),
            run_id: "latest".into(),
            event: ThreadEvent::Finished {
                model: None,
                result: "Mira waits at the cobalt bridge.".into(),
            },
        })
        .unwrap();
    store
        .append_event(&ThreadEventEnvelope {
            thread_id: "thread".into(),
            run_id: "latest".into(),
            event: ThreadEvent::ContextCheckpoint {
                summary: "Chaotic lossy summary with no topic facts.".into(),
            },
        })
        .unwrap();
    drop(store);
    let store = TranscriptStore::open(&path).unwrap();
    let recalled = store
        .context_for_request("thread", "What did we discuss about amber migration?", 8000)
        .unwrap()
        .into_iter()
        .map(|t| t.text)
        .collect::<Vec<_>>()
        .join("\n");
    assert!(recalled.contains("AMBER-794"));
    assert!(recalled.contains("no implementation yet"));
    assert!(recalled.contains("source_seq"));
    let latest = store
        .context_for_request("thread", "Continue that", 8000)
        .unwrap()
        .into_iter()
        .map(|t| t.text)
        .collect::<Vec<_>>()
        .join("\n");
    assert!(latest.contains("Mira waits at the cobalt bridge"));
    assert!(latest.contains("Latest task"));
    assert!(
        !latest.contains("AMBER-794"),
        "a vague follow-up must not select an older topic"
    );
    assert!(latest.len() < 8000);
    for greeting in [
        "hi",
        "Thanks!",
        "hello there, thank you",
        "你好",
        "谢谢你！",
        "こんにちは",
        "안녕하세요",
        "Спасибо!",
        "gracias",
        "👋",
    ] {
        let greeted = store
            .context_for_request("thread", greeting, 8000)
            .unwrap()
            .into_iter()
            .map(|t| t.text)
            .collect::<Vec<_>>()
            .join("\n");
        assert!(
            !greeted.contains("source_seq") && !greeted.contains("Mira"),
            "{greeting:?} must not pull history anchors back: {greeted}"
        );
    }
    // A CJK request with content still recalls by term; a mixed greeting plus content does too.
    for request in ["你好，请回忆 amber migration", "thanks, what about amber?"] {
        let recalled = store
            .context_for_request("thread", request, 8000)
            .unwrap()
            .into_iter()
            .map(|t| t.text)
            .collect::<Vec<_>>()
            .join("\n");
        assert!(recalled.contains("AMBER-794"), "{request:?}");
    }
}

#[test]
fn anchors_skip_exchanges_already_verbatim_in_the_working_context() {
    let dir = tempfile::tempdir().unwrap();
    let store = TranscriptStore::open(&dir.path().join("sessions.sqlite3")).unwrap();
    store
        .append_user("thread", "one", "Write the cobalt narrative. Hero is Mira.")
        .unwrap();
    store
        .append_event(&ThreadEventEnvelope {
            thread_id: "thread".into(),
            run_id: "one".into(),
            event: ThreadEvent::Finished {
                model: None,
                result: "Mira waits at the cobalt bridge.".into(),
            },
        })
        .unwrap();
    let texts = store
        .context_for_request("thread", "Continue that", 8000)
        .unwrap()
        .into_iter()
        .map(|t| t.text)
        .collect::<Vec<_>>();
    assert_eq!(
        texts.len(),
        2,
        "both exchanges are already in context: {texts:?}"
    );
    assert!(texts.iter().all(|text| !text.contains("source_seq")));
}

#[test]
fn usage_query_survives_restart_and_filters_run_without_returning_content() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("sessions.sqlite3");
    let store = TranscriptStore::open(&path).unwrap();
    store
        .append_user("thread", "first", "PRIVATE-PROMPT-CONTENT")
        .unwrap();
    for (run, count) in [("first", 2), ("second", 7)] {
        store.append_event(&ThreadEventEnvelope{thread_id:"thread".into(),run_id:run.into(),event:ThreadEvent::ContextUsage{metrics:serde_json::json!({"total_requests":count,"provider_input_tokens":null})}}).unwrap();
    }
    drop(store);
    let store = TranscriptStore::open(&path).unwrap();
    assert_eq!(
        store.context_usage("thread", None).unwrap()["total_requests"],
        7
    );
    assert_eq!(
        store.context_usage("thread", Some("first")).unwrap()["total_requests"],
        2
    );
    assert!(store.context_usage("missing", None).unwrap().is_null());
    assert!(!store
        .context_usage("thread", None)
        .unwrap()
        .to_string()
        .contains("PRIVATE-PROMPT"));
}

#[test]
fn historical_original_exports_are_incremental_and_restart_reuses_segments() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("sessions.sqlite3");
    let exports = directory.path().join("sources");
    std::fs::create_dir(&exports).unwrap();
    let store = TranscriptStore::open(&path).unwrap();
    let original = "Exact original 中 \" quotation\n".repeat(5000);
    store.append_user("thread", "first", &original).unwrap();
    store.publish_context_sources("thread", &exports).unwrap();
    let first = std::fs::read_dir(&exports)
        .unwrap()
        .next()
        .unwrap()
        .unwrap()
        .path();
    let bytes = std::fs::read(&first).unwrap();
    let record: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(record["content"], original);
    assert_eq!(record["role"], "User");
    store.append_user("thread", "second", "New task").unwrap();
    store.publish_context_sources("thread", &exports).unwrap();
    assert_eq!(std::fs::read(&first).unwrap(), bytes);
    assert_eq!(std::fs::read_dir(&exports).unwrap().count(), 2);
    drop(store);
    let store = TranscriptStore::open(&path).unwrap();
    store.publish_context_sources("thread", &exports).unwrap();
    assert_eq!(std::fs::read_dir(&exports).unwrap().count(), 2);
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
                model: Some("original-model".into()),
                result: "Done".into(),
            },
        })
        .unwrap();
    drop(store);
    let reopened = TranscriptStore::open(&path).unwrap();
    let history = reopened.history("thread").unwrap();
    assert_eq!(history.len(), 5);
    assert!(
        matches!(&history[4], HistoryItem::Event { envelope } if matches!(&envelope.event, ThreadEvent::Finished { model: Some(model), .. } if model == "original-model"))
    );
    let context = reopened.context("thread", 20).unwrap();
    assert_eq!(context[0].text, "Original task; read note.txt");
    assert_eq!(format!("{:?}", context[0].role), "Checkpoint");
    assert!(context[1]
        .text
        .contains("Original task\nMiddle request\nLatest request"));
    assert_eq!(context[2].text, "Done");
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
        .set_secret("go".into(), "synthetic-test-key".into())
        .await
        .unwrap();
    first.set_go_base_url_override(Some(server.uri()));
    let thread = first
        .create_thread(
            project.path().to_string_lossy().into_owned(),
            ProviderKind::Go,
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
        .set_secret("go".into(), "synthetic-test-key".into())
        .await
        .unwrap();
    second.set_go_base_url_override(Some(server.uri()));
    assert_eq!(
        second
            .get_thread_history(&thread.id)
            .await
            .unwrap()
            .iter()
            .filter(|item| !matches!(
                item,
                HistoryItem::Event {
                    envelope: ThreadEventEnvelope {
                        event: ThreadEvent::ContextUsage { .. } | ThreadEvent::ContextBudget { .. },
                        ..
                    }
                }
            ))
            .count(),
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
