use super::*;
use crate::transcript::TranscriptStore;

#[tokio::test(flavor = "multi_thread")]
async fn oversized_legacy_response_returns_error_and_subscription_requests_resync() {
    let data = tempfile::tempdir().unwrap();
    let project = tempfile::tempdir().unwrap();
    let state = AppState::new_for_test(data.path().join("settings.json"));
    let thread = state
        .create_thread(
            project.path().to_string_lossy().into(),
            crate::types::ProviderKind::Go,
            None,
        )
        .await
        .unwrap();
    TranscriptStore::open(&data.path().join("sessions.sqlite3"))
        .unwrap()
        .append_user(&thread.id, "run", &"x".repeat(MAX_FRAME))
        .unwrap();
    let server = Server::bind(state, data.path()).await.unwrap();
    let sink = server.sink.clone();
    let serving = tokio::spawn(server.run());
    let client = Client::new(data.path().into());
    let error = client
        .call_raw("get_thread_history", json!({"threadId":thread.id}))
        .await
        .unwrap_err();
    assert!(error.contains("get_thread_history_page"), "{error}");
    let mut subscription = client.subscribe().await.unwrap();
    sink.emit_thread_event(&ThreadEventEnvelope {
        thread_id: thread.id.clone(),
        run_id: "run".into(),
        event: crate::types::ThreadEvent::Finished {
            model: None,
            result: "x".repeat(MAX_FRAME),
        },
    });
    let event = Client::next_event(&mut subscription).await;
    client.call("shutdown", json!({})).await.unwrap();
    serving.await.unwrap().unwrap();
    assert_eq!(
        event.expect("Oversized subscription data must request history reconciliation, not EOF"),
        json!({"name":"lagged","payload":null})
    );
}

#[tokio::test]
async fn history_adapter_rejects_stalled_cursors_changed_watermarks_and_partial_endings() {
    for replies in [
        vec![json!({"jsonl":"", "through_seq":1,"next_cursor":{"after_seq":0,"offset":0}})],
        vec![
            json!({"jsonl":"{", "through_seq":1,"next_cursor":{"after_seq":0,"offset":1}}),
            json!({"jsonl":"}","through_seq":2,"next_cursor":null}),
        ],
        vec![json!({"jsonl":"{", "through_seq":1,"next_cursor":null})],
    ] {
        let data = tempfile::tempdir().unwrap();
        let listener = TcpListener::bind((std::net::Ipv4Addr::LOCALHOST, 0))
            .await
            .unwrap();
        write_discovery(
            data.path(),
            &Discovery {
                port: listener.local_addr().unwrap().port(),
                token: "fixture".into(),
            },
        )
        .unwrap();
        let serving = tokio::spawn(async move {
            for value in replies {
                let (mut stream, _) = listener.accept().await.unwrap();
                let request: Request = read_frame(&mut stream).await.unwrap();
                assert_eq!(request.method, "get_thread_history_page");
                write_frame(
                    &mut stream,
                    &Response {
                        ok: true,
                        value,
                        error: None,
                    },
                )
                .await
                .unwrap();
            }
        });
        let result = tokio::time::timeout(
            std::time::Duration::from_secs(3),
            Client::new(data.path().into()).call("get_thread_history", json!({"threadId":"chat"})),
        )
        .await
        .unwrap();
        assert!(result.is_err());
        serving.await.unwrap();
    }
}
