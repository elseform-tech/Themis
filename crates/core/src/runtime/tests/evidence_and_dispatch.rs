//! Saved evidence, summaries, approvals, tool delivery and dispatch.

use super::*;

#[tokio::test]
async fn obsolete_latest_job_pointer_cannot_block_compaction() {
    use wiremock::{matchers::method, Mock, MockServer, ResponseTemplate};
    let server = MockServer::builder().start().await;
    Mock::given(method("POST")).respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({"choices":[{"message":{"content":"Goal/constraints: preserve. Confirmed findings: source supplied. Completed changes/tests: none. Unresolved work: continue. Next action: answer."},"finish_reason":"stop"}]}))).mount(&server).await;
    let llm = crate::providers::resolve(
        &crate::providers::ProviderConfig::new(crate::providers::ProviderKind::Go, "test-key")
            .with_model("test-model")
            .with_base_url(server.uri()),
    )
    .await
    .unwrap();
    let directory = tempfile::tempdir().unwrap();
    let obsolete = directory.path().join("pending-compaction.json");
    std::fs::create_dir(&obsolete).unwrap();
    let mut messages = initial_messages(
        vec![ConversationTurn {
            role: ConversationRole::User,
            text: "Original task evidence".repeat(500),
        }],
        "Continue".into(),
    );
    let (tx, mut rx) = tokio::sync::mpsc::channel(16);
    compact_context_with_evidence(
        &llm,
        &mut messages,
        &RunPolicy {
            total_turns: 8,
            context_token_budget: 24_000,
            recent_messages: 2,
        },
        &tx,
        &AtomicBool::new(false),
        std::time::Duration::from_secs(5),
        Some(directory.path().into()),
    )
    .await
    .unwrap();
    assert_eq!(server.received_requests().await.unwrap().len(), 1);
    assert!(std::iter::from_fn(|| rx.try_recv().ok())
        .any(|event| matches!(event, RunEvent::ContextCheckpoint { .. })));
    // The snapshot is the only job input written; no pending copy is left behind.
    assert!(!std::fs::read_dir(directory.path())
        .unwrap()
        .any(|entry| entry
            .unwrap()
            .file_name()
            .to_string_lossy()
            .starts_with("pending-job-")));
    assert!(
        obsolete.is_dir(),
        "obsolete artifacts must not be rewritten"
    );
}

#[tokio::test]
async fn originals_are_saved_before_the_summary_deadline_starts() {
    // Windows CI flaked here: the originals were saved inside the summary deadline, a
    // slow disk flush outlasted it, and the run said "Originals were not saved on disk"
    // while the save finished in the background. A zero deadline reproduces that order.
    use wiremock::{matchers::method, Mock, MockServer, ResponseTemplate};
    let server = MockServer::builder().start().await;
    Mock::given(method("POST"))
            .respond_with(
                ResponseTemplate::new(200)
                    .set_delay(std::time::Duration::from_secs(30))
                    .set_body_json(serde_json::json!({"choices":[{"message":{"role":"assistant","content":"Late summary"},"finish_reason":"stop"}]})),
            )
            .mount(&server)
            .await;
    let llm = crate::providers::resolve(
        &crate::providers::ProviderConfig::new(crate::providers::ProviderKind::Go, "test-key")
            .with_model("test-model")
            .with_base_url(server.uri()),
    )
    .await
    .unwrap();
    let evidence = tempfile::tempdir().unwrap();
    let mut messages = initial_messages(
        vec![ConversationTurn {
            role: ConversationRole::User,
            text: "Original decision: preserve red-753".repeat(500),
        }],
        "Continue the task".into(),
    );
    let (tx, _rx) = tokio::sync::mpsc::channel(16);
    let error = compact_context_with_evidence(
        &llm,
        &mut messages,
        &RunPolicy {
            total_turns: 200,
            context_token_budget: 200_000,
            recent_messages: 4,
        },
        &tx,
        &AtomicBool::new(false),
        std::time::Duration::ZERO,
        Some(evidence.path().to_owned()),
    )
    .await
    .unwrap_err()
    .to_string();
    assert!(error.contains("timed out"), "{error}");
    assert!(error.contains("Originals saved on disk."), "{error}");
    let saved = std::fs::read_dir(evidence.path())
        .unwrap()
        .map(|entry| entry.unwrap().path())
        .filter(|path| {
            path.extension()
                .is_some_and(|extension| extension == "jsonl")
        })
        .any(|path| std::fs::read_to_string(path).unwrap().contains("red-753"));
    assert!(
        saved,
        "the originals are on disk when the run reports them saved"
    );
}

#[tokio::test]
async fn required_summary_failure_preserves_originals_without_continuation() {
    use wiremock::{matchers::method, Mock, MockServer, ResponseTemplate};
    for case in [
        "timeout",
        "cancel",
        "empty",
        "oversized",
        "provider",
        "invalid",
    ] {
        let cancel = case == "cancel";
        let server = MockServer::builder().start().await;
        let content = if case == "oversized" {
            "x".repeat(900_000)
        } else if case == "empty" {
            String::new()
        } else {
            "Late summary".to_owned()
        };
        let response = ResponseTemplate::new(if case == "provider" { 503 } else { 200 })
                .set_body_json(serde_json::json!({"choices":[{"message":{"role":"assistant","content":content},"finish_reason":"stop"}]}));
        let response = if case == "timeout" || cancel {
            response.set_delay(std::time::Duration::from_secs(30))
        } else {
            response
        };
        let request_started = Arc::new(AtomicBool::new(false));
        let observed = request_started.clone();
        Mock::given(method("POST"))
            .respond_with(move |_: &wiremock::Request| {
                observed.store(true, Ordering::SeqCst);
                response.clone()
            })
            .mount(&server)
            .await;
        let llm = crate::providers::resolve(
            &crate::providers::ProviderConfig::new(crate::providers::ProviderKind::Go, "test-key")
                .with_model("test-model")
                .with_base_url(server.uri()),
        )
        .await
        .unwrap();
        let evidence = tempfile::tempdir().unwrap();
        let mut messages = initial_messages(
                vec![ConversationTurn {
                    role: ConversationRole::Assistant,
                    text: "Earlier context checkpoint (historical; later user requests take precedence):\nPRIOR_STATE_753: keep the earlier constraint.".into(),
                }, ConversationTurn {
                    role: ConversationRole::User,
                    text: "Original decision: preserve red-753".repeat(500),
                }],
                "Continue the task".into(),
            );
        let before = serde_json::to_value(&messages).unwrap();
        let stopped = Arc::new(AtomicBool::new(false));
        if cancel {
            let stopped = stopped.clone();
            tokio::spawn(async move {
                // Cancel after the summary request starts: originals are then durable.
                while !request_started.load(Ordering::SeqCst) {
                    tokio::time::sleep(std::time::Duration::from_millis(1)).await;
                }
                stopped.store(true, Ordering::SeqCst);
            });
        }
        let (tx, mut rx) = tokio::sync::mpsc::channel(16);
        let result = compact_context_with_evidence(
            &llm,
            &mut messages,
            &RunPolicy {
                total_turns: 200,
                context_token_budget: 200_000,
                recent_messages: 4,
            },
            &tx,
            &stopped,
            std::time::Duration::from_secs(5),
            Some(evidence.path().to_owned()),
        )
        .await;
        let mut events = Vec::new();
        while let Ok(event) = rx.try_recv() {
            events.push(event);
        }
        assert!(
            result.is_err(),
            "required summary failure must suspend: {case}"
        );
        assert_eq!(serde_json::to_value(&messages).unwrap(), before);
        assert!(!events
            .iter()
            .any(|event| matches!(event, RunEvent::ContextCheckpoint { .. })));
        let paths: Vec<_> = std::fs::read_dir(evidence.path())
            .unwrap()
            .map(|entry| entry.unwrap().path())
            .filter(|path| {
                path.extension()
                    .is_some_and(|extension| extension == "jsonl")
            })
            .filter(|path| {
                std::fs::read_to_string(path)
                    .unwrap()
                    .contains("\"role\":\"User\"")
            })
            .collect();
        assert_eq!(paths.len(), 1);
        let records: Vec<ChatMessage> = std::fs::read_to_string(&paths[0])
            .unwrap()
            .lines()
            .map(|line| serde_json::from_str(line).unwrap())
            .collect();
        assert_eq!(
            serde_json::to_value(records).unwrap().as_array().unwrap(),
            &before.as_array().unwrap()[1..]
        );
    }
}

#[tokio::test]
async fn malformed_nonempty_summary_is_rejected() {
    use wiremock::{matchers::method, Mock, MockServer, ResponseTemplate};
    let server = MockServer::builder().start().await;
    Mock::given(method("POST")).respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({"choices":[{"message":{"role":"assistant","content":"OK"},"finish_reason":"stop"}]}))).mount(&server).await;
    let llm = crate::providers::resolve(
        &crate::providers::ProviderConfig::new(crate::providers::ProviderKind::Go, "test-key")
            .with_model("test-model")
            .with_base_url(server.uri()),
    )
    .await
    .unwrap();
    assert!(compaction_chat(
        &llm,
        &context_update_messages("Source", "", "Continue", true),
        1000
    )
    .await
    .is_err());
}

#[tokio::test]
async fn excerpted_checkpoint_keeps_the_middle_in_originals_after_a_failed_attempt() {
    use wiremock::{matchers::method, Mock, MockServer, ResponseTemplate};
    let server = MockServer::builder().start().await;
    let calls = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let seen = Arc::new(AtomicBool::new(false));
    let count = calls.clone();
    let middle_seen = seen.clone();
    Mock::given(method("POST")).respond_with(move |request: &wiremock::Request| {
            let n = count.fetch_add(1, Ordering::SeqCst);
            if String::from_utf8_lossy(&request.body).contains("MIDDLE_CANONICAL_947") {
                middle_seen.store(true, Ordering::SeqCst);
            }
            if n == 0 {
                ResponseTemplate::new(400)
            } else {
                ResponseTemplate::new(200).set_body_json(serde_json::json!({"choices":[{"message":{"role":"assistant","content":"Goal/constraints: retain exact original references. Confirmed findings: source supplied. Completed changes/tests: none. Unresolved work: continue. Next action: inspect current task."},"finish_reason":"stop"}]}))
            }
        }).mount(&server).await;
    let llm = crate::providers::resolve(
        &crate::providers::ProviderConfig::new(crate::providers::ProviderKind::Go, "test-key")
            .with_model("test-model")
            .with_base_url(server.uri()),
    )
    .await
    .unwrap();
    let directory = tempfile::tempdir().unwrap();
    let original = initial_messages(
        vec![ConversationTurn {
            role: ConversationRole::User,
            text: format!(
                "{} MIDDLE_CANONICAL_947 {}",
                "alpha ".repeat(15_000),
                "omega ".repeat(15_000)
            ),
        }],
        "Continue the narrative".into(),
    );
    let policy = RunPolicy {
        total_turns: 8,
        context_token_budget: 24_000,
        recent_messages: 2,
    };
    let (tx, mut rx) = tokio::sync::mpsc::channel(32);
    let mut messages = original.clone();
    let stopped = AtomicBool::new(false);
    assert!(compact_context_with_evidence(
        &llm,
        &mut messages,
        &policy,
        &tx,
        &stopped,
        std::time::Duration::from_secs(30),
        Some(directory.path().into())
    )
    .await
    .is_err());
    assert_eq!(
        serde_json::to_value(&messages).unwrap(),
        serde_json::to_value(&original).unwrap()
    );
    assert!(!std::iter::from_fn(|| rx.try_recv().ok())
        .any(|e| matches!(e, RunEvent::ContextCheckpoint { .. })));
    compact_context_with_evidence(
        &llm,
        &mut messages,
        &policy,
        &tx,
        &stopped,
        std::time::Duration::from_secs(30),
        Some(directory.path().into()),
    )
    .await
    .unwrap();
    // One request per attempt: the failed one and the installed one.
    assert_eq!(server.received_requests().await.unwrap().len(), 2);
    // The excerpt omits the middle; the summary is told so, and the saved original keeps it.
    assert!(!seen.load(Ordering::SeqCst));
    assert!(messages
        .iter()
        .any(|m| m.content.contains("coverage is incomplete")));
    assert!(std::fs::read_dir(directory.path())
        .unwrap()
        .map(|entry| entry.unwrap().path())
        .filter(|path| path.extension().is_some_and(|e| e == "jsonl"))
        .any(|path| std::fs::read_to_string(path)
            .unwrap()
            .contains("MIDDLE_CANONICAL_947")));
}

#[test]
fn evidence_snapshot_is_visible_only_after_all_records_are_saved() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().to_owned();
    let messages = initial_messages(
        vec![ConversationTurn {
            role: ConversationRole::User,
            text: "\"\n".repeat(500_000),
        }],
        "Continue".into(),
    );
    let worker = std::thread::spawn(move || archive_context(&path, &messages));
    let mut incomplete = false;
    while !worker.is_finished() {
        for entry in std::fs::read_dir(directory.path()).unwrap() {
            let path = entry.unwrap().path();
            if path
                .extension()
                .is_some_and(|extension| extension == "jsonl")
            {
                let text = std::fs::read_to_string(path).unwrap();
                let records = text
                    .lines()
                    .map(serde_json::from_str::<serde_json::Value>)
                    .collect::<Result<Vec<_>, _>>();
                incomplete |=
                    records.is_err() || records.as_ref().is_ok_and(|records| records.len() != 2);
            }
        }
        if incomplete {
            break;
        }
        std::thread::yield_now();
    }
    let (_, snapshot) = worker.join().unwrap().unwrap();
    assert!(
        !incomplete,
        "a published original must never expose incomplete JSONL records"
    );
    assert_eq!(
        std::fs::read_to_string(snapshot).unwrap().lines().count(),
        2
    );
}

#[tokio::test]
async fn bounded_retrieval_survives_lossy_checkpoint_and_restart() {
    let original = serde_json::json!({"path":".themis/context/original.jsonl", "jsonl_record":3,
            "json_pointer":"/content", "record_role":"User", "found":true, "excerpt_offset":715,
            "match_offset":915, "next_offset":4715, "content":"Friday September 8: she died yesterday. é🔎"});
    let ledger_read = {
        let mut read = original.clone();
        read["kind"] = "read".into();
        read.to_string()
    };
    let result = ToolCall {
        id: "retrieved".into(),
        call_type: "function".into(),
        function: FunctionCall {
            name: "read_file".into(),
            arguments: original.to_string(),
        },
    };
    let mut messages = initial_messages(Vec::new(), "Answer from originals".into());
    messages.push(ChatMessage {
        role: ChatRole::Tool,
        message_type: MessageType::ToolResult(vec![result]),
        content: String::new(),
    });
    messages.push(ChatMessage {
        role: ChatRole::Assistant,
        message_type: MessageType::Text,
        content: "Retrieved evidence".into(),
    });
    let policy = RunPolicy {
        total_turns: 200,
        context_token_budget: 200_000,
        recent_messages: 4,
    };
    let (tx, mut rx) = tokio::sync::mpsc::channel(16);
    let split = messages.len();
    replace_context(
        &mut messages,
        &policy,
        split,
        "Task state only; details omitted.".into(),
        &tx,
        None,
    )
    .await
    .unwrap();
    assert!(messages
        .iter()
        .any(|message| message.content.contains(&ledger_read)));
    let RunEvent::ContextCheckpoint { summary } = rx.recv().await.unwrap() else {
        panic!("checkpoint required")
    };
    assert!(summary.contains(&ledger_read));
    assert!(ledger_text(&messages, 10, None).is_empty());
    let mut navigation = original.clone();
    navigation["record_role"] = serde_json::json!("Assistant");
    navigation["content"] = serde_json::json!("Navigation must not become primary evidence");
    let mut rejected = messages.clone();
    rejected.push(ChatMessage {
        role: ChatRole::Tool,
        message_type: MessageType::ToolResult(vec![ToolCall {
            id: "navigation".into(),
            call_type: "function".into(),
            function: FunctionCall {
                name: "read_file".into(),
                arguments: navigation.to_string(),
            },
        }]),
        content: String::new(),
    });
    assert!(!ledger_text(&rejected, 64_000, None)
        .contains("Navigation must not become primary evidence"));
    if let MessageType::ToolResult(results) = &mut rejected.last_mut().unwrap().message_type {
        navigation["record_role"] = "Tool".into();
        navigation["evidence_kind"] = "recovery".into();
        results[0].function.arguments = navigation.to_string();
    }
    assert!(
        !ledger_text(&rejected, 64_000, None)
            .contains("Navigation must not become primary evidence"),
        "generated recovery output must not become a cached original excerpt"
    );
    // Restart rebuilds model history from the persisted checkpoint, not in-memory tool messages.
    let mut restarted = initial_messages(
        vec![ConversationTurn {
            role: ConversationRole::Assistant,
            text: format!("Earlier context checkpoint:\n{summary}"),
        }],
        "Continue the same task".into(),
    );
    let split = restarted.len();
    replace_context(
        &mut restarted,
        &policy,
        split,
        "Another lossy state.".into(),
        &tx,
        None,
    )
    .await
    .unwrap();
    assert!(restarted
        .iter()
        .any(|message| message.content.contains(&ledger_read)));
    let RunEvent::ContextCheckpoint { summary } = rx.recv().await.unwrap() else {
        panic!("checkpoint required")
    };
    assert_eq!(summary.matches(&ledger_read).count(), 1);
}

#[test]
fn earlier_evidence_location_survives_excerpt_eviction_and_another_checkpoint() {
    let earlier = serde_json::json!({"path":"original.jsonl", "jsonl_record":3,
            "json_pointer":"/message_type/ToolResult/0/function/arguments", "record_role":"Tool", "decode_tool_payload":true, "found":true,
            "find":"reconciliation", "excerpt_offset":715, "next_offset":4715,
            "content":format!("WRITTEN September 6; DELIVERED September 8. {}", "é".repeat(300))});
    let mut later = earlier.clone();
    later["excerpt_offset"] = serde_json::json!(9000);
    later["find"] = serde_json::json!("mourning");
    later["content"] = serde_json::json!("later unrelated evidence ".repeat(25));
    let messages = vec![ChatMessage {
        role: ChatRole::Tool,
        message_type: MessageType::ToolResult(
            [earlier.clone(), later.clone()]
                .into_iter()
                .enumerate()
                .map(|(index, value)| ToolCall {
                    id: format!("read_{index}"),
                    call_type: "function".into(),
                    function: FunctionCall {
                        name: "read_file".into(),
                        arguments: value.to_string(),
                    },
                })
                .collect(),
        ),
        content: String::new(),
    }];
    // One ledger budget: locations are reserved before excerpts are upgraded.
    let state = ledger_text(&messages, 1300, None);
    assert!(
        !state.contains("WRITTEN September 6"),
        "earlier excerpt must actually be evicted"
    );
    let mut later_read = later.clone();
    later_read["kind"] = "read".into();
    assert!(
        state.contains(&later_read.to_string()),
        "recent excerpt remains bounded"
    );
    let pointer: serde_json::Value = state
        .lines()
        .filter_map(|line| serde_json::from_str::<serde_json::Value>(line).ok())
        .find(|entry| entry["kind"] == "location")
        .expect("eviction must preserve source locations");
    assert_eq!(pointer["path"], "original.jsonl");
    assert_eq!(pointer["jsonl_record"], 3);
    assert_eq!(
        pointer["json_pointer"],
        "/message_type/ToolResult/0/function/arguments"
    );
    assert_eq!(
        pointer["decode_tool_payload"], true,
        "decoded offsets must retain their coordinate space after excerpt eviction"
    );
    assert_eq!(pointer["excerpt_offset"], 715);
    assert_eq!(pointer["find"], "reconciliation");
    assert!(
        pointer.get("content").is_none(),
        "location does not duplicate the evicted body"
    );
    let restarted = initial_messages(
        vec![ConversationTurn {
            role: ConversationRole::Assistant,
            text: format!("Earlier context checkpoint:\n{state}"),
        }],
        "Continue".into(),
    );
    let next = ledger_text(&restarted, 1300, None);
    assert_eq!(
        next.matches(&pointer.to_string()).count(),
        1,
        "earlier reference survives another lossy checkpoint without duplication"
    );
}

#[tokio::test]
async fn evidence_locations_recover_historical_reads_after_live_file_changes() {
    let directory = tempfile::tempdir().unwrap();
    let live = directory.path().join("source.txt");
    std::fs::write(&live, "original decision").unwrap();
    let original = serde_json::json!({"path":live, "found":true,
            "excerpt_offset":0, "next_offset":17, "content":"original decision"});
    let mut messages = initial_messages(Vec::new(), "Read source".into());
    messages.push(ChatMessage {
        role: ChatRole::Tool,
        message_type: MessageType::ToolResult(vec![ToolCall {
            id: "read".into(),
            call_type: "function".into(),
            function: FunctionCall {
                name: "read_file".into(),
                arguments: original.to_string(),
            },
        }]),
        content: String::new(),
    });
    let (_, archive) = archive_context(directory.path(), &messages).unwrap();
    let state = ledger_text(&messages, 4000, Some(&archive));
    std::fs::write(&live, "changed decision").unwrap();
    let restarted = initial_messages(
        vec![ConversationTurn {
            role: ConversationRole::Assistant,
            text: format!("Earlier context checkpoint:\n{state}"),
        }],
        "Continue".into(),
    );
    let next = ledger_text(&restarted, 4000, None);
    let entries: Vec<serde_json::Value> = next
        .lines()
        .filter_map(|line| serde_json::from_str(line).ok())
        .collect();
    assert_eq!(
        entries.len(),
        1,
        "one read survives restart without duplication"
    );
    let location = &entries[0];
    assert_eq!(
        location["archive_record"], 2,
        "System messages are not archived"
    );
    let tools =
        crate::tools::boxed_tools(directory.path(), Arc::new(crate::tools::AllowAllHook)).unwrap();
    let reader = tools
        .iter()
        .find(|tool| tool.name() == "read_file")
        .unwrap();
    let archived_path = std::path::Path::new(location["archive_path"].as_str().unwrap());
    let recovered = reader
        .execute(serde_json::json!({
            "file_path": archived_path.strip_prefix(directory.path()).unwrap(),
            "jsonl_record": location["archive_record"],
            "json_pointer": location["archive_pointer"], "offset":0
        }))
        .await
        .unwrap();
    assert_eq!(recovered["record_role"], "Tool");
    let receipt: serde_json::Value =
        serde_json::from_str(recovered["content"].as_str().unwrap()).unwrap();
    assert_eq!(receipt, original);
    assert_eq!(std::fs::read_to_string(&live).unwrap(), "changed decision");
}

#[tokio::test]
async fn truncated_summary_retries_with_more_headroom_without_accepting_partial_text() {
    use wiremock::{matchers::method, Mock, MockServer, Request, ResponseTemplate};
    for limit in [4096, 8192] {
        for recovers in [true, false] {
            let server = MockServer::builder().start().await;
            let calls = Arc::new(std::sync::atomic::AtomicUsize::new(0));
            let observed = Arc::clone(&calls);
            Mock::given(method("POST")).respond_with(move |_: &Request| {
                    let attempt = observed.fetch_add(1, Ordering::SeqCst);
                    let complete = recovers && attempt == 1;
                    ResponseTemplate::new(200).set_body_json(serde_json::json!({
                        "choices":[{"message":{"role":"assistant","content":if complete { "Goal/constraints: retain task. Confirmed findings: originals supplied. Completed changes/tests: none. Unresolved work: continue. Next action: complete state" } else { "partial state" }},"finish_reason":if complete { "stop" } else { "length" }}]
                    }))
                }).mount(&server).await;
            let llm = crate::providers::resolve(
                &crate::providers::ProviderConfig::new(
                    crate::providers::ProviderKind::Go,
                    "test-key",
                )
                .with_model("test-model")
                .with_base_url(server.uri()),
            )
            .await
            .unwrap();
            let messages = [ChatMessage {
                role: ChatRole::User,
                message_type: MessageType::Text,
                content: "preserve task state".into(),
            }];
            let result = compaction_chat(&llm, &messages, limit).await;
            if recovers {
                assert_eq!(result.unwrap(), "Goal/constraints: retain task. Confirmed findings: originals supplied. Completed changes/tests: none. Unresolved work: continue. Next action: complete state");
            } else {
                assert!(result.is_err(), "never accept truncated text");
            }
            let requests = server.received_requests().await.unwrap();
            assert_eq!(
                requests.len(),
                if recovers { 2 } else { 3 },
                "bounded retry count"
            );
            for (index, request) in requests.iter().enumerate() {
                let body: serde_json::Value = serde_json::from_slice(&request.body).unwrap();
                assert_eq!(body["max_tokens"], limit * if index == 0 { 1 } else { 2 });
                assert!(body["messages"][0]["content"]
                    .as_str()
                    .unwrap()
                    .starts_with("preserve task state\n"));
                let first: serde_json::Value = requests[0].body_json().unwrap();
                assert_eq!(
                    body["messages"], first["messages"],
                    "retry retains the same context and guidance"
                );
            }
        }
    }
}

#[tokio::test]
async fn oversized_checkpoint_preserves_original_context() {
    use wiremock::{matchers::method, Mock, MockServer, ResponseTemplate};
    let server = MockServer::builder().start().await;
    Mock::given(method("POST"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({"choices":[{"message":{"role":"assistant","content":format!("Goal/constraints: retain task. Confirmed findings: originals supplied. Completed changes/tests: none. Unresolved work: continue. Next action: {}", " apple".repeat(10000))},"finish_reason":"stop"}]})))
            .mount(&server).await;
    let llm = crate::providers::resolve(
        &crate::providers::ProviderConfig::new(crate::providers::ProviderKind::Go, "test-key")
            .with_model("test-model")
            .with_base_url(server.uri()),
    )
    .await
    .unwrap();
    let mut messages = initial_messages(
        vec![ConversationTurn {
            role: ConversationRole::User,
            text: "Original context".into(),
        }],
        "Continue".into(),
    );
    let before = serde_json::to_value(&messages).unwrap();
    let (tx, mut rx) = tokio::sync::mpsc::channel(16);
    let result = compact_context_with_timeout(
        &llm,
        &mut messages,
        &RunPolicy {
            total_turns: 5,
            context_token_budget: 8000,
            recent_messages: 4,
        },
        &tx,
        &AtomicBool::new(false),
        std::time::Duration::from_secs(5),
    )
    .await;
    assert!(result.unwrap_err().to_string().contains("context budget"));
    assert_eq!(serde_json::to_value(&messages).unwrap(), before);
    assert!(matches!(
        rx.try_recv().unwrap(),
        RunEvent::ContextCompacting
    ));
    assert!(rx.try_recv().is_err());
}

#[test]
fn prior_turns_precede_new_task_in_model_context() {
    let messages = initial_messages(
        vec![
            ConversationTurn {
                role: ConversationRole::User,
                text: "Remember ORBIT-17".into(),
            },
            ConversationTurn {
                role: ConversationRole::Assistant,
                text: "Saved".into(),
            },
        ],
        "What was the code?".into(),
    );
    assert_eq!(messages.len(), 4);
    assert!(matches!(messages[0].role, ChatRole::System));
    assert!(messages[0]
        .content
        .contains("what you are doing now and why, or what you found and what comes next"));
    // Social messages get a reply in kind, not a recap of parked work.
    assert!(messages[0].content.contains(
        "do not recap, summarize or resume earlier or parked work unless the user asks about it"
    ));
    assert!(!messages[0].content.contains("decode_tool_payload"));
    assert!(!messages[0]
        .content
        .contains("first line is exactly Milestone:"));
    for instruction in [
        "sources you actually inspected",
        "verified current line or line range",
        "why it addresses the task",
        "material tradeoffs",
        "what you tested and how it came out",
        "Never invent references or coordinates",
        "Creative prose needs no evidence citations",
        "using permitted reference fields without extra wrappers",
    ] {
        assert!(messages[0].content.contains(instruction), "{instruction}");
    }
    assert_eq!(
        messages[0]
            .content
            .matches("For factual and technical work")
            .count(),
        1
    );
    assert!(matches!(messages[1].role, ChatRole::User));
    assert_eq!(messages[1].content, "Remember ORBIT-17");
    assert!(matches!(messages[2].role, ChatRole::Assistant));
    assert_eq!(messages[3].content, "What was the code?");
}

#[test]
fn allow_always_is_cached_per_tool() {
    let calls = Arc::new(AtomicUsize::new(0));
    let moved = Arc::clone(&calls);
    let hook = FnHook::new(move |_: &ToolAction| {
        moved.fetch_add(1, Ordering::SeqCst);
        Approval::AllowAlways
    });
    let caching = CachingApprovals::new(Arc::new(hook));
    assert_eq!(caching.approve(&action("shell")), Approval::AllowAlways);
    assert_eq!(caching.approve(&action("shell")), Approval::AllowAlways);
    assert_eq!(caching.approve(&action("git")), Approval::AllowAlways);
    assert_eq!(calls.load(Ordering::SeqCst), 2);
    assert!(caching.is_cached("shell"));
    assert!(!caching.is_cached("read_file"));
}

#[test]
fn allow_once_and_deny_are_never_cached() {
    let calls = Arc::new(AtomicUsize::new(0));
    let moved = Arc::clone(&calls);
    let hook = FnHook::new(move |action: &ToolAction| {
        moved.fetch_add(1, Ordering::SeqCst);
        if action.tool == "git" {
            Approval::AllowOnce
        } else {
            Approval::Deny
        }
    });
    let caching = CachingApprovals::new(Arc::new(hook));
    assert_eq!(caching.approve(&action("git")), Approval::AllowOnce);
    assert_eq!(caching.approve(&action("git")), Approval::AllowOnce);
    assert_eq!(caching.approve(&action("shell")), Approval::Deny);
    assert_eq!(caching.approve(&action("shell")), Approval::Deny);
    assert_eq!(calls.load(Ordering::SeqCst), 4);
    assert!(!caching.is_cached("git"));
}

#[test]
fn risk_classification_matches_tool_kinds() {
    assert_eq!(risk_for("read_file"), RiskLevel::Read);
    assert_eq!(risk_for("list_dir"), RiskLevel::Read);
    assert_eq!(risk_for("search_file"), RiskLevel::Read);
    assert_eq!(risk_for("shell"), RiskLevel::Execute);
    assert_eq!(risk_for("write_file"), RiskLevel::Write);
    assert_eq!(risk_for("apply_patch"), RiskLevel::Write);
    assert_eq!(risk_for("git"), RiskLevel::Write);
    assert_eq!(risk_for("something-new"), RiskLevel::Write);
}

#[test]
fn summary_folds_whitespace_and_truncates() {
    let short = summarize_call("git", "{\"args\": [\"status\"]}");
    assert_eq!(short, "call `git` with {\"args\":[\"status\"]}");
    // The path comes first even when the model sent a long content first.
    let write = summarize_call(
        "write_file",
        &serde_json::json!({"content": "x".repeat(500), "file_path": "src/scene.js"}).to_string(),
    );
    assert!(
        write.starts_with("call `write_file` with {\"file_path\":\"src/scene.js\",\"content\":"),
        "{write}"
    );
    let long = summarize_call("shell", &"x".repeat(500));
    assert!(long.chars().count() <= 201, "{long}");
    assert!(long.ends_with('…'), "{long}");
}

#[tokio::test]
async fn capacity_and_output_reserves_reach_actual_provider_requests_at_each_budget() {
    use wiremock::{matchers::method, Mock, MockServer, ResponseTemplate};
    let server = MockServer::builder().start().await;
    Mock::given(method("POST")).respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({"choices":[{"message":{"content":"Complete"},"finish_reason":"stop"}]}))).mount(&server).await;
    let llm = crate::providers::resolve(
        &crate::providers::ProviderConfig::new(crate::providers::ProviderKind::Go, "synthetic")
            .with_model("test-model")
            .with_base_url(server.uri()),
    )
    .await
    .unwrap();
    for budget in [24000, 200000, 256000] {
        let (tx, _rx) = tokio::sync::mpsc::channel(32);
        let policy = RunPolicy {
            total_turns: 8,
            context_token_budget: budget,
            recent_messages: 4,
        };
        let answer = with_model_context_capacity(
            Some(Some(1000000)),
            run_task_with_evidence(
                llm.clone(),
                vec![],
                "Answer simply".into(),
                vec![],
                Arc::new(crate::tools::AllowAllHook),
                policy,
                tx,
                Arc::new(AtomicBool::new(false)),
                String::new(),
                None,
            ),
        )
        .await
        .unwrap();
        assert_eq!(answer, "Complete");
    }
    for (request, budget) in server
        .received_requests()
        .await
        .unwrap()
        .iter()
        .zip([24000usize, 200000, 256000])
    {
        let body: serde_json::Value = request.body_json().unwrap();
        assert_eq!(body["max_tokens"], (budget / 8).min(8192));
    }
    assert_eq!(
        with_model_context_capacity(Some(Some(32000)), async {
            effective_context_budget(256000, "m")
        })
        .await,
        32000
    );
    assert_eq!(
        with_model_context_capacity(Some(None), async { effective_context_budget(256000, "m") })
            .await,
        crate::context_window::UNKNOWN_WINDOW_GUESS
    );
}

#[test]
fn char_prefix_preserves_unicode_and_detects_truncation() {
    assert_eq!(take_char_prefix("é🙂", 2), ("é🙂".to_owned(), false));
    assert_eq!(take_char_prefix("é🙂x", 2), ("é🙂".to_owned(), true));
}

#[test]
fn approval_decision_names_are_stable() {
    assert_eq!(
        ApprovalDecision::from(Approval::AllowOnce).to_string(),
        "allow-once"
    );
    assert_eq!(
        ApprovalDecision::from(Approval::AllowAlways).to_string(),
        "allow-always"
    );
    assert_eq!(ApprovalDecision::from(Approval::Deny).to_string(), "deny");
}

#[test]
fn run_event_serializes_with_kind_tag() {
    let event = RunEvent::ApprovalDecided {
        tool: "shell".to_owned(),
        decision: ApprovalDecision::Deny,
    };
    let json = serde_json::to_value(&event).unwrap();
    assert_eq!(json["kind"], "approval_decided");
    assert_eq!(json["data"]["tool"], "shell");
    assert_eq!(json["data"]["decision"], "deny");
}

#[tokio::test]
async fn original_derived_excerpts_survive_real_bounded_tool_delivery() {
    let root = tempfile::tempdir().unwrap();
    let folder = root.path().join(".themis/context/test");
    std::fs::create_dir_all(&folder).unwrap();
    let padding = "🔎\"\\\n".repeat(2000);
    let source_text = format!("FAIL_DELIVERY_KEEP\n{padding}MATCH_DELIVERY_KEEP{padding}");
    let source =
        serde_json::json!({"role":"User","evidence_kind":"original","content":source_text});
    std::fs::write(folder.join("source.jsonl"), format!("{source}\n")).unwrap();
    let tools =
        crate::tools::boxed_tools(root.path(), Arc::new(crate::tools::AllowAllHook)).unwrap();
    let approvals =
        CachingApprovals::wrap(Arc::new(FnHook::new(|_: &ToolAction| Approval::AllowOnce)));
    for (batch_bytes, count, find) in [
        (4096, 1, None),
        (8192, 4, None),
        (2048, 1, Some("MATCH_DELIVERY_KEEP")),
    ] {
        let calls: Vec<_> = (0..count).map(|index| ToolCall {
                id:format!("read-{index}"), call_type:"function".into(),
                function:FunctionCall { name:"read_file".into(), arguments: {
                    let mut args = serde_json::json!({"file_path":".themis/context/test/source.jsonl", "jsonl_record":1});
                    if let Some(needle) = find { args["find"] = needle.into(); }
                    args.to_string()
                } }
            }).collect();
        let (tx, _rx) = tokio::sync::mpsc::channel(64);
        let evidence = folder.join(format!("delivery-{batch_bytes}-{count}"));
        std::fs::create_dir(&evidence).unwrap();
        let results = execute_tool_calls(
            &tools,
            &approvals,
            &calls,
            &tx,
            &AtomicBool::new(false),
            Some(&evidence),
            "",
            batch_bytes,
        )
        .await
        .unwrap();
        assert!(serde_json::to_vec(&results).unwrap().len() <= batch_bytes);
        let messages = [
            ChatMessage {
                role: ChatRole::Assistant,
                message_type: MessageType::ToolUse(calls),
                content: String::new(),
            },
            ChatMessage {
                role: ChatRole::Tool,
                message_type: MessageType::ToolResult(results.clone()),
                content: String::new(),
            },
        ];
        let (projected, _) = summary_evidence::records(&messages).unwrap();
        let excerpts: Vec<_> = projected
            .iter()
            .filter(|v| v["evidence_kind"] == "original_derived_excerpt")
            .collect();
        assert_eq!(
            excerpts.len(),
            1,
            "actual delivery must retain one deduplicated bounded original-derived excerpt"
        );
        assert!(excerpts[0]["content"]
            .as_str()
            .unwrap()
            .starts_with(find.unwrap_or("FAIL_DELIVERY_KEEP")));
        for result in results {
            let delivered: serde_json::Value =
                serde_json::from_str(&result.function.arguments).unwrap();
            let length = delivered["content"].as_str().unwrap().chars().count();
            assert!(length > 0 && length < 4000);
            if let Some(needle) = find {
                assert_eq!(delivered["excerpt_offset"], delivered["match_offset"]);
                assert_eq!(
                    delivered["next_offset"].as_u64().unwrap(),
                    delivered["match_offset"].as_u64().unwrap() + needle.chars().count() as u64
                );
                assert!(delivered["content"].as_str().unwrap().contains(needle));
            } else {
                assert_eq!(delivered["next_offset"], length);
            }
            assert_eq!(delivered["truncated"], true);
        }
        let saved: Vec<_> = std::fs::read_dir(&evidence).unwrap().collect();
        assert_eq!(saved.len(), count);
        for entry in saved {
            let snapshot = std::fs::read_to_string(entry.unwrap().path()).unwrap();
            let record: serde_json::Value =
                serde_json::from_str(snapshot.lines().nth(1).unwrap()).unwrap();
            assert_eq!(record["evidence_kind"], "recovery");
            let output: serde_json::Value = serde_json::from_str(
                record["message_type"]["ToolResult"][0]["function"]["arguments"]
                    .as_str()
                    .unwrap(),
            )
            .unwrap();
            assert_eq!(
                output["content"].as_str().unwrap().chars().count(),
                crate::tools::READ_PAGE_CHARS,
                "full authorized result is saved before delivery shrinks it"
            );
        }
    }
}

#[tokio::test]
async fn tool_delivery_offloads_large_results_and_bounds_the_batch() {
    #[derive(Debug)]
    struct OutputTool;
    #[autoagents::async_trait]
    impl autoagents::core::tool::ToolRuntime for OutputTool {
        async fn execute(
            &self,
            args: serde_json::Value,
        ) -> Result<serde_json::Value, autoagents::core::tool::ToolCallError> {
            Ok(
                serde_json::json!({"success":false,"isError":true,"truncated":true,"exit_code":7,"output":args["text"].as_str().unwrap().repeat(args["repeat"].as_u64().unwrap() as usize)}),
            )
        }
    }
    impl ToolT for OutputTool {
        fn name(&self) -> &str {
            "output"
        }
        fn description(&self) -> &str {
            "Generate test output"
        }
        fn args_schema(&self) -> serde_json::Value {
            serde_json::json!({})
        }
    }
    for (text, repeat) in [("x", 12_000), ("é\"\\\n", 30_000)] {
        let directory = tempfile::tempdir().unwrap();
        let tools: Vec<Box<dyn ToolT>> = vec![Box::new(OutputTool)];
        let approvals =
            CachingApprovals::wrap(Arc::new(FnHook::new(|_: &ToolAction| Approval::AllowOnce)));
        let calls = (0..5)
            .map(|index| ToolCall {
                id: format!("output-{index}"),
                call_type: "function".into(),
                function: FunctionCall {
                    name: "output".into(),
                    arguments: serde_json::json!({"text":text,"repeat":repeat}).to_string(),
                },
            })
            .collect::<Vec<_>>();
        let (tx, _rx) = tokio::sync::mpsc::channel(64);
        let results = execute_tool_calls(
            &tools,
            &approvals,
            &calls,
            &tx,
            &AtomicBool::new(false),
            Some(directory.path()),
            "",
            200_000,
        )
        .await
        .unwrap();
        assert!(
            serde_json::to_vec(&results).unwrap().len() <= 48 * 1024,
            "combined model delivery must stay bounded"
        );
        let mut offloaded = 0;
        for result in results {
            assert!(serde_json::to_vec(&result).unwrap().len() <= 16 * 1024);
            let value: serde_json::Value =
                serde_json::from_str(&result.function.arguments).unwrap();
            if value["result_offloaded"] != true {
                continue;
            }
            offloaded += 1;
            assert_eq!(value["success"], false);
            assert_eq!(value["isError"], true);
            assert_eq!(value["truncated"], true);
            assert_eq!(value["exit_code"], 7);
            assert_eq!(value["source"]["jsonl_record"], 2);
            assert_eq!(
                value["source"]["json_pointer"],
                "/message_type/ToolResult/0/function/arguments"
            );
            let archive =
                std::fs::read_to_string(value["original_snapshot"].as_str().unwrap()).unwrap();
            let record: serde_json::Value =
                serde_json::from_str(archive.lines().nth(1).unwrap()).unwrap();
            let original: serde_json::Value = serde_json::from_str(
                record
                    .pointer("/message_type/ToolResult/0/function/arguments")
                    .unwrap()
                    .as_str()
                    .unwrap(),
            )
            .unwrap();
            assert_eq!(original["output"], text.repeat(repeat));
        }
        assert!(offloaded > 0);
        assert_eq!(std::fs::read_dir(directory.path()).unwrap().count(), 5);
    }
}

#[tokio::test]
async fn completed_tool_evidence_survives_stop_and_save_failure_is_terminal() {
    for save_fails in [false, true] {
        let directory = tempfile::tempdir().unwrap();
        let evidence = directory.path().join("evidence");
        if save_fails {
            std::fs::write(&evidence, "not a directory").unwrap();
        } else {
            std::fs::create_dir(&evidence).unwrap();
        }
        let stopped = Arc::new(AtomicBool::new(false));
        let tools: Vec<Box<dyn ToolT>> = vec![Box::new(StubTool(Some(stopped.clone())))];
        let hook: Arc<dyn ApprovalHook> =
            Arc::new(FnHook::new(|_: &ToolAction| Approval::AllowOnce));
        let approvals = CachingApprovals::wrap(hook);
        let (tx, mut rx) = tokio::sync::mpsc::channel(16);
        let call = ToolCall {
            id: "completed-before-stop".into(),
            call_type: "function".into(),
            function: FunctionCall {
                name: "stub".into(),
                arguments: "{}".into(),
            },
        };
        let result = execute_tool_calls(
            &tools,
            &approvals,
            &[call],
            &tx,
            &stopped,
            Some(&evidence),
            "",
            200_000,
        )
        .await;
        assert_eq!(result.is_err(), save_fails);
        if !save_fails {
            let path = std::fs::read_dir(&evidence)
                .unwrap()
                .next()
                .unwrap()
                .unwrap()
                .path();
            let records: Vec<serde_json::Value> = std::fs::read_to_string(path)
                .unwrap()
                .lines()
                .map(|line| serde_json::from_str(line).unwrap())
                .collect();
            assert_eq!(
                records[1]["message_type"]["ToolResult"][0]["id"],
                "completed-before-stop"
            );
            assert!(check_stopped(&stopped, &tx).await.is_err());
        }
        let mut events = Vec::new();
        while let Ok(event) = rx.try_recv() {
            events.push(event);
        }
        assert!(events
            .iter()
            .any(|event| matches!(event, RunEvent::ToolCallFinished { ok: true, .. })));
        assert_eq!(
            events
                .iter()
                .filter(|event| matches!(event, RunEvent::Failed { .. }))
                .count(),
            1,
            "stop and save failure must produce exactly one terminal event: {events:?}"
        );
        if save_fails {
            assert!(events.iter().any(|event| matches!(event, RunEvent::Failed { error } if error.starts_with("Tool evidence failed:"))));
        }
    }
}

#[tokio::test]
async fn dispatch_respects_destructive_policy() {
    use crate::configuration::{
        configured_hook, ApprovalMode, ApprovalPolicy, ApprovalRule, PolicyAction,
    };
    let root = tempfile::tempdir().unwrap();
    let victim = root.path().join("keep.txt");
    std::fs::write(&victim, "keep").unwrap();
    let hook = configured_hook(
        ApprovalMode::Custom,
        ApprovalPolicy {
            default: PolicyAction::Allow,
            rules: vec![ApprovalRule {
                command: None,
                tool: "*".into(),
                action: PolicyAction::Deny,
                risk: Some("destructive".into()),
            }],
            ..Default::default()
        },
        Arc::new(crate::tools::AllowAllHook),
    );
    let tools = crate::tools::boxed_tools(root.path(), hook.clone()).unwrap();
    let caching = CachingApprovals::wrap(hook);
    let (events, _) = tokio::sync::mpsc::channel(16);
    let result = execute_call(
        &tools,
        &caching,
        "shell",
        &serde_json::json!({"command":"rm","args":["-rf","keep.txt"]}).to_string(),
        "rm -rf keep.txt",
        &events,
        &AtomicBool::new(false),
    )
    .await;
    assert!(result.unwrap_err().contains("approval denied"));
    assert_eq!(std::fs::read_to_string(victim).unwrap(), "keep");
}

/// The dialog's first prompt for a shell call (git included) comes from this dispatch
/// gate, so it must carry the exact argv (not the JSON summary).
#[tokio::test]
async fn dispatch_gate_sends_exact_argv_for_shell_calls() {
    let seen: Arc<std::sync::Mutex<Vec<ToolAction>>> = Arc::default();
    let recorder = {
        let seen = Arc::clone(&seen);
        move |action: &ToolAction| {
            seen.lock().unwrap().push(action.clone());
            Approval::Deny
        }
    };
    let hook: Arc<dyn ApprovalHook> = Arc::new(FnHook::new(recorder));
    let root = tempfile::tempdir().unwrap();
    let tools = crate::tools::boxed_tools(root.path(), Arc::clone(&hook)).unwrap();
    let caching = CachingApprovals::wrap(hook);
    let (events, _rx) = tokio::sync::mpsc::channel(16);
    let calls = [
        (
            "shell",
            serde_json::json!({"command":"rm","args":["-rf /","x\u{202e}y"],"cwd":"sub dir","background":true}),
        ),
        ("shell", serde_json::json!({"command":"ls","args":["-la"]})),
        ("read_file", serde_json::json!({"file_path":"a.txt"})),
    ];
    for (name, args) in &calls {
        let text = args.to_string();
        let _ = execute_call(
            &tools,
            &caching,
            name,
            &text,
            &summarize_call(name, &text),
            &events,
            &AtomicBool::new(false),
        )
        .await;
    }
    let seen = seen.lock().unwrap();
    let display = |index: usize| seen[index].display.clone();
    let first = display(0).expect("shell display");
    assert_eq!(first.argv, ["rm", "-rf /", "x\u{202e}y"]);
    assert_eq!(first.cwd.as_deref(), Some("sub dir"));
    assert!(first.background);
    let second = display(1).expect("shell display");
    assert_eq!(
        (second.argv.as_slice(), second.cwd, second.background),
        (&["ls".to_owned(), "-la".to_owned()][..], None, false)
    );
    assert!(seen[2].display.is_none());
}

#[tokio::test]
async fn dispatch_and_execute_consult_hook_exactly_once() {
    use crate::tools::GatedTool;

    let consults = Arc::new(AtomicUsize::new(0));
    let counting = {
        let consults = Arc::clone(&consults);
        move |_action: &ToolAction| {
            consults.fetch_add(1, Ordering::SeqCst);
            Approval::AllowOnce
        }
    };
    let hook: Arc<dyn ApprovalHook> = Arc::new(FnHook::new(counting));
    let caching = CachingApprovals::wrap(Arc::clone(&hook));
    let tools: Vec<Box<dyn ToolT>> = vec![Box::new(GatedTool::new(
        Box::new(StubTool(None)),
        Arc::clone(&hook),
        RiskLevel::Write,
    ))];
    let (events_tx, _events_rx) = tokio::sync::mpsc::channel(16);

    // Dispatch consults (dialog #1); the gated execution consumes the
    // dispatch permit instead of consulting again: exactly one prompt.
    let result = execute_call(
        &tools,
        &caching,
        "stub",
        "{}",
        "test call",
        &events_tx,
        &AtomicBool::new(false),
    )
    .await;
    assert!(result.is_ok(), "{result:?}");
    assert_eq!(consults.load(Ordering::SeqCst), 1);

    // Outside a dispatch scope the execution check still gates: a direct
    // call consults the hook again.
    let direct = tools[0]
        .execute(serde_json::json!({}))
        .await
        .expect("direct execute allowed");
    assert_eq!(direct, serde_json::json!({"ok": true}));
    assert_eq!(consults.load(Ordering::SeqCst), 2);
}

/// The approval for a file write the run asks about names the file in plain words, as the
/// file tool's own approval does, never a raw tool call.
#[tokio::test]
async fn dispatch_gate_names_the_file_a_write_changes() {
    let seen: Arc<std::sync::Mutex<Vec<ToolAction>>> = Arc::default();
    let recorder = {
        let seen = Arc::clone(&seen);
        move |action: &ToolAction| {
            seen.lock().unwrap().push(action.clone());
            Approval::Deny
        }
    };
    let hook: Arc<dyn ApprovalHook> = Arc::new(FnHook::new(recorder));
    let root = tempfile::tempdir().unwrap();
    let tools = crate::tools::boxed_tools(root.path(), Arc::clone(&hook)).unwrap();
    let caching = CachingApprovals::wrap(hook);
    let (events, _rx) = tokio::sync::mpsc::channel(16);
    let args = serde_json::json!({"file_path":"backup.sh","append":false,"content":"#!/bin/sh\necho hi\n"});
    let text = args.to_string();
    let _ = execute_call(
        &tools,
        &caching,
        "write_file",
        &text,
        &summarize_call("write_file", &text),
        &events,
        &AtomicBool::new(false),
    )
    .await;
    let seen = seen.lock().unwrap();
    assert_eq!(seen[0].summary, "write `backup.sh`");
}
