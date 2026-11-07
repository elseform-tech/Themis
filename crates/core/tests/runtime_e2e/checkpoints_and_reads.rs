//! Forced checkpoints, bounded and repeated reads, retries and argument limits.

use super::*;

/// Deterministic harness proof; the provider is scripted, not a live model quality claim.
#[tokio::test]
async fn forced_checkpoint_resumes_task_at_24k_200k_and_256k_without_replaying_writes() {
    use themis_core::runtime::{ConversationRole, ConversationTurn};
    for budget in [24_000, 200_000, 256_000] {
        let server = MockServer::builder().start().await;
        let mut discarded = common::tool_call_body(
            "discarded",
            "write_file",
            json!({"file_path":"progress.txt","content":"phase1\n","append":true}),
        );
        discarded["choices"][0]["finish_reason"] = "length".into();
        common::mount_script_with_summary(
            &server,
            vec![discarded, common::tool_call_body(
                "remaining", "write_file",
                json!({"file_path":"progress.txt","content":"phase2\n","append":true}),
            ), common::final_text_body(&common::summary("Task resumed and completed"))],
            Some("Original task ORDERED-PHASES: phase1 already saved to progress.txt; append only phase2, preserve phase1, then finish."),
        ).await;
        let project = tempfile::tempdir().unwrap();
        std::fs::write(project.path().join("progress.txt"), "phase1\n").unwrap();
        let evidence = project.path().join(".themis/context/forced");
        std::fs::create_dir_all(&evidence).unwrap();
        let mut history = vec![ConversationTurn {
            role: ConversationRole::User,
            text: "Original task ORDERED-PHASES: append phase1 then phase2 exactly once each, preserving existing progress.".into(),
        }, ConversationTurn {
            role: ConversationRole::Assistant,
            text: "Completed phase1 in progress.txt. Remaining work: phase2 only.".into(),
        }];
        // Distinct unrelated historical records exceed the unchanged window. This
        // bypasses neither the production trigger nor its snapshot/summary pipeline.
        for index in 0..24 {
            history.push(ConversationTurn {
                role: ConversationRole::User,
                text: format!(
                    "Unrelated historical packet {index}: {}",
                    format!("district {index} observation; ").repeat(budget / 100)
                ),
            });
        }
        let llm = resolve(
            &ProviderConfig::new(ProviderKind::Go, "test-key")
                .with_model("test-model")
                .with_base_url(server.uri()),
        )
        .await
        .unwrap();
        let approvals: Arc<dyn ApprovalHook> = Arc::new(AllowAllHook);
        let (tx, mut rx) = tokio::sync::mpsc::channel(2048);
        let answer = themis_core::runtime::run_task_with_evidence(
            llm,
            boxed_tools(project.path(), approvals.clone()).unwrap(),
            "Resume the original ordered task and finish only the remaining work.".into(),
            history,
            approvals,
            RunPolicy {
                total_turns: 4,
                context_token_budget: budget,
                recent_messages: 4,
            },
            tx,
            Arc::new(AtomicBool::new(false)),
            String::new(),
            Some(evidence.clone()),
        )
        .await
        .unwrap();
        assert_eq!(answer, common::summary("Task resumed and completed"));
        assert_eq!(
            std::fs::read_to_string(project.path().join("progress.txt")).unwrap(),
            "phase1\nphase2\n"
        );
        let events = common::drain(&mut rx).await;
        let checkpoint = events
            .iter()
            .position(|event| matches!(event, RunEvent::ContextCheckpoint { .. }))
            .expect("must install a genuine checkpoint");
        let write = events
            .iter()
            .position(|event| matches!(event, RunEvent::ToolCallStarted { .. }))
            .expect("must perform useful work");
        assert!(checkpoint < write);
        assert_eq!(events.iter().filter(|event| matches!(event, RunEvent::ToolCallFinished {tool,ok:true,..} if tool=="write_file")).count(),1);
        assert_eq!(
            events
                .iter()
                .filter(|event| matches!(event, RunEvent::Finished { .. }))
                .count(),
            1
        );
        assert!(!events
            .iter()
            .any(|event| matches!(event, RunEvent::Failed { .. })));
        let archives = std::fs::read_dir(&evidence)
            .unwrap()
            .map(|entry| entry.unwrap().path())
            .filter(|path| path.extension().is_some_and(|ext| ext == "jsonl"))
            .collect::<Vec<_>>();
        assert!(
            !archives.is_empty(),
            "checkpoint needs durable original evidence"
        );
        assert!(
            archives
                .iter()
                .any(
                    |path| std::fs::read_to_string(path).unwrap().lines().any(|line| {
                        let record: serde_json::Value = serde_json::from_str(line).unwrap();
                        record["role"] == "User" && record.to_string().contains("ORDERED-PHASES")
                    })
                ),
            "original task must remain on disk"
        );
        let requests = server.received_requests().await.unwrap();
        let bodies = requests
            .iter()
            .map(|request| request.body_json::<serde_json::Value>().unwrap())
            .collect::<Vec<_>>();
        let summary_packets = bodies
            .iter()
            .filter(|body| {
                body["messages"][0]["content"]
                    .as_str()
                    .is_some_and(|text| text.contains("You summarize agent context"))
            })
            .map(|body| body["messages"].to_string())
            .collect::<Vec<_>>()
            .join("\n");
        assert!(summary_packets.contains("ORDERED-PHASES"));
        assert!(summary_packets.contains("Completed phase1 in progress.txt"));
        let ordinary_packets = bodies
            .iter()
            .filter(|body| {
                !body["messages"][0]["content"]
                    .as_str()
                    .is_some_and(|text| text.contains("You summarize agent context"))
            })
            .collect::<Vec<_>>();
        assert_eq!(
            ordinary_packets.len(),
            4,
            "one discarded attempt, one useful write, one completion, one spec check"
        );
        for packet in &ordinary_packets {
            let system = packet["messages"][0]["content"].as_str().unwrap();
            assert_eq!(system.matches("For factual and technical work").count(), 1);
            assert!(system.contains("verified current line or line range"));
            assert!(system.contains("why it addresses the task"));
            assert!(system.contains("using permitted reference fields without extra wrappers"));
        }
        assert!(
            ordinary_packets[0]["messages"]
                .to_string()
                .contains("phase1 already saved"),
            "installed checkpoint must reach ordinary generation"
        );
        for request in requests {
            let body: serde_json::Value = request.body_json().unwrap();
            let message_tokens = body["messages"]
                .as_array()
                .unwrap()
                .iter()
                .map(|m| m.to_string().len().div_ceil(4) + 16)
                .sum::<usize>();
            let schema_tokens = body
                .get("tools")
                .map_or(0, |tools| tools.to_string().len().div_ceil(4));
            assert!(
                message_tokens + schema_tokens + body["max_tokens"].as_u64().unwrap() as usize
                    <= budget,
                "provider request exceeds selected budget"
            );
        }
    }
}

#[tokio::test]
async fn plain_source_read_recovers_from_jsonl_selectors_with_bounded_output() {
    let server = MockServer::builder().start().await;
    common::mount_script(
        &server,
        vec![
            common::tool_call_body(
                "wrong-format",
                "read_file",
                json!({"file_path":"source.py","jsonl_record":1,"json_pointer":"/content"}),
            ),
            common::tool_call_body("plain-read", "read_file", json!({"file_path":"source.py"})),
            common::final_text_body(&common::summary(
                "Read the source after correcting the selector",
            )),
        ],
    )
    .await;
    let project = tempfile::tempdir().unwrap();
    std::fs::write(
        project.path().join("source.py"),
        format!("import unittest\n{}", "# source detail\n".repeat(1000)),
    )
    .unwrap();
    let llm = resolve(
        &ProviderConfig::new(ProviderKind::Go, "test-key")
            .with_model("test-model")
            .with_base_url(server.uri()),
    )
    .await
    .unwrap();
    let approvals: Arc<dyn ApprovalHook> = Arc::new(AllowAllHook);
    let (tx, mut rx) = tokio::sync::mpsc::channel(128);
    let answer = run_task_with_policy(
        llm,
        boxed_tools(project.path(), approvals.clone()).unwrap(),
        "Inspect source.py".into(),
        vec![],
        approvals,
        RunPolicy {
            total_turns: 5,
            context_token_budget: 200_000,
            recent_messages: 4,
        },
        tx,
        Arc::new(AtomicBool::new(false)),
    )
    .await
    .unwrap();
    assert_eq!(
        answer,
        common::summary("Read the source after correcting the selector")
    );
    let events = common::drain(&mut rx).await;
    let results = events
        .iter()
        .filter_map(|event| match event {
            RunEvent::ToolCallFinished { ok, output, .. } => Some((*ok, output)),
            _ => None,
        })
        .collect::<Vec<_>>();
    assert_eq!(results.len(), 2);
    // Archive selectors on a plain file no longer fail: the file is read and the agent told.
    assert!(results[0].0, "{}", results[0].1);
    assert!(
        results[0].1.contains("selectors_ignored"),
        "{}",
        results[0].1
    );
    assert!(results[1].0);
    let text: serde_json::Value = serde_json::from_str(results[1].1).unwrap();
    assert!(text["content"]
        .as_str()
        .unwrap()
        .starts_with("import unittest"));
    assert!(
        text["content"].as_str().unwrap().chars().count() <= themis_core::tools::READ_PAGE_CHARS
    );
    assert!(text["next_offset"].is_number());
    assert!(has_finished(&events));
    let requests = server.received_requests().await.unwrap();
    assert_eq!(requests.len(), 3);
    // The first (selector) read already reached the model with the file's content.
    let correction: serde_json::Value = requests[1].body_json().unwrap();
    let delivered = serde_json::to_string(&correction["messages"]).unwrap();
    assert!(delivered.contains("selectors_ignored") && delivered.contains("import unittest"));
    let continuation: serde_json::Value = requests[2].body_json().unwrap();
    assert!(serde_json::to_string(&continuation["messages"])
        .unwrap()
        .contains("import unittest"));
    let body: serde_json::Value = requests[0].body_json().unwrap();
    // The plain-file guidance lives in the read_file schema; the shared system text no longer
    // repeats the JSONL recovery recipe.
    assert!(!body["messages"][0]["content"]
        .as_str()
        .unwrap()
        .contains("decode_tool_payload"));
    let read = body["tools"]
        .as_array()
        .unwrap()
        .iter()
        .find(|tool| tool["function"]["name"] == "read_file")
        .unwrap();
    assert!(body["messages"][0]["content"]
        .as_str()
        .unwrap()
        .contains("read_file returns at most 8000 characters"));
    // The description leads with plain files and keeps archive selectors to one sentence.
    let description = read["function"]["description"].as_str().unwrap();
    assert!(description.starts_with("Read a project text file. Returns up to 8000 characters"));
    assert!(description.contains("For saved chat archives (.jsonl) only"));
    assert!(description.len() < 800, "{}", description.len());
}

#[tokio::test]
async fn retired_compaction_command_does_not_intercept_user_instructions() {
    let server = MockServer::builder().start().await;
    common::mount_script(&server, vec![common::final_text_body("Ordinary answer")]).await;
    let project = tempfile::tempdir().unwrap();
    let llm = resolve(
        &ProviderConfig::new(ProviderKind::Go, "test-key")
            .with_model("test-model")
            .with_base_url(server.uri()),
    )
    .await
    .unwrap();
    let approvals: Arc<dyn ApprovalHook> = Arc::new(AllowAllHook);
    let (tx, _rx) = tokio::sync::mpsc::channel(128);
    let text = "/retry-compaction explain this previously discussed command";
    let answer = run_task_with_policy(
        llm,
        boxed_tools(project.path(), approvals.clone()).unwrap(),
        text.into(),
        vec![],
        approvals,
        RunPolicy {
            total_turns: 5,
            context_token_budget: 200_000,
            recent_messages: 4,
        },
        tx,
        Arc::new(AtomicBool::new(false)),
    )
    .await
    .unwrap();
    assert_eq!(answer, "Ordinary answer");
    let requests = server.received_requests().await.unwrap();
    assert_eq!(requests.len(), 1);
    let body: serde_json::Value = requests[0].body_json().unwrap();
    assert_eq!(
        body["messages"].as_array().unwrap().last().unwrap()["content"],
        text
    );
}

#[tokio::test]
async fn automatic_summary_retries_do_not_repeat_a_completed_write() {
    use std::sync::atomic::{AtomicUsize, Ordering};
    use wiremock::{matchers::method, Mock, ResponseTemplate};
    let server = MockServer::builder().start().await;
    let project = tempfile::tempdir().unwrap();
    let receipt = project.path().join("receipt.txt");
    let summaries_follow_write = Arc::new(AtomicBool::new(true));
    let order = summaries_follow_write.clone();
    let ordinary = Arc::new(AtomicUsize::new(0));
    let summaries = Arc::new(AtomicUsize::new(0));
    let ordinary_count = ordinary.clone();
    let summary_count = summaries.clone();
    Mock::given(method("POST")).respond_with(move |request: &wiremock::Request| {
        let body: serde_json::Value = request.body_json().unwrap();
        if body["messages"][0]["content"].as_str().unwrap().contains("You summarize agent context") {
            order.fetch_and(ordinary_count.load(Ordering::SeqCst) == 1 && std::fs::read_to_string(&receipt).is_ok_and(|text| text == "once\n"), Ordering::SeqCst);
            if summary_count.fetch_add(1, Ordering::SeqCst) < 2 {
                return ResponseTemplate::new(503);
            }
            return ResponseTemplate::new(200).set_body_json(common::final_text_body(&common::summary("Goal/constraints: finish. Confirmed findings: write saved. Completed changes/tests: appended once to receipt.txt. Unresolved work: report completion. Next action: answer without repeating the write.")));
        }
        let response = if ordinary_count.fetch_add(1, Ordering::SeqCst) == 0 {
            let mut call = common::tool_call_body("append-once", "write_file", json!({"file_path":"receipt.txt","content":"once\n","append":true}));
            call["choices"][0]["message"]["content"] = "progress narration ".repeat(50).into();
            call
        } else { common::final_text_body(&common::summary("Completed once")) };
        ResponseTemplate::new(200).set_body_json(response)
    }).mount(&server).await;
    let llm = resolve(
        &ProviderConfig::new(ProviderKind::Go, "test-key")
            .with_model("test-model")
            .with_base_url(server.uri()),
    )
    .await
    .unwrap();
    let approvals: Arc<dyn ApprovalHook> = Arc::new(AllowAllHook);
    let (tx, mut rx) = tokio::sync::mpsc::channel(128);
    let evidence = project.path().join(".themis/context/check");
    std::fs::create_dir_all(&evidence).unwrap();
    let answer = themis_core::runtime::run_task_with_evidence(
        llm,
        boxed_tools(project.path(), approvals.clone()).unwrap(),
        "Append once and finish".into(),
        vec![themis_core::runtime::ConversationTurn {
            role: themis_core::runtime::ConversationRole::Assistant,
            text: "Historical semantic state ".repeat(2300),
        }],
        approvals,
        RunPolicy {
            total_turns: 5,
            context_token_budget: 24_000,
            recent_messages: 2,
        },
        tx,
        Arc::new(AtomicBool::new(false)),
        String::new(),
        Some(evidence),
    )
    .await
    .unwrap();
    assert_eq!(answer, common::summary("Completed once"));
    assert_eq!(
        std::fs::read_to_string(project.path().join("receipt.txt")).unwrap(),
        "once\n"
    );
    // The write, then the completion and its spec check.
    assert_eq!(ordinary.load(Ordering::SeqCst), 3);
    assert_eq!(summaries.load(Ordering::SeqCst), 3);
    assert!(
        summaries_follow_write.load(Ordering::SeqCst),
        "all summary attempts must follow the completed append"
    );
    let events = common::drain(&mut rx).await;
    assert_eq!(
        events
            .iter()
            .filter(|event| matches!(event, RunEvent::ToolCallStarted { .. }))
            .count(),
        1
    );
    assert_eq!(
        events
            .iter()
            .filter(|event| matches!(event, RunEvent::ContextCheckpoint { .. }))
            .count(),
        1
    );
    assert!(has_finished(&events));
    assert!(!events
        .iter()
        .any(|event| matches!(event, RunEvent::Failed { .. })));
}

#[tokio::test]
async fn unchanged_reads_still_in_context_get_a_freshness_receipt_but_changed_files_do_not() {
    let server = MockServer::builder().start().await;
    common::mount_script(
        &server,
        vec![
            common::tool_call_body("first", "read_file", json!({"file_path":"source.txt"})),
            common::tool_call_body("other", "read_file", json!({"file_path":"other.txt"})),
            common::tool_call_body("again", "read_file", json!({"file_path":"./source.txt"})),
            common::tool_call_body(
                "edit",
                "write_file",
                json!({"file_path":"source.txt","content":"changed","append":false}),
            ),
            common::tool_call_body("changed", "read_file", json!({"file_path":"source.txt"})),
            common::final_text_body("Finished from the updated observation"),
        ],
    )
    .await;
    let project = tempfile::tempdir().unwrap();
    std::fs::write(project.path().join("source.txt"), "original").unwrap();
    std::fs::write(project.path().join("other.txt"), "other").unwrap();
    let evidence = project.path().join(".themis/context/repeats");
    std::fs::create_dir_all(&evidence).unwrap();
    let llm = resolve(
        &ProviderConfig::new(ProviderKind::Go, "test-key")
            .with_model("test-model")
            .with_base_url(server.uri()),
    )
    .await
    .unwrap();
    let approvals: Arc<dyn ApprovalHook> = Arc::new(AllowAllHook);
    let (tx, _rx) = tokio::sync::mpsc::channel(128);
    themis_core::runtime::run_task_with_evidence(
        llm,
        boxed_tools(project.path(), approvals.clone()).unwrap(),
        "Inspect, edit, and inspect".into(),
        vec![],
        approvals,
        RunPolicy {
            total_turns: 8,
            context_token_budget: 200_000,
            recent_messages: 20,
        },
        tx,
        Arc::new(AtomicBool::new(false)),
        String::new(),
        Some(evidence),
    )
    .await
    .unwrap();
    let requests = server.received_requests().await.unwrap();
    let tool_output = |index: usize, id: &str| -> String {
        let body: serde_json::Value = requests[index].body_json().unwrap();
        body["messages"]
            .as_array()
            .unwrap()
            .iter()
            .find(|m| m["tool_call_id"] == id)
            .unwrap()["content"]
            .as_str()
            .unwrap()
            .into()
    };
    let repeated = tool_output(3, "again");
    let receipt: serde_json::Value = serde_json::from_str(&repeated).unwrap();
    assert_eq!(receipt["repeated_unchanged"], true);
    assert_eq!(
        receipt["unchanged_since_call"], "first",
        "a repeat points at the identical result still in context"
    );
    assert_eq!(receipt["found"], true, "coordinates stay for paging");
    assert!(
        receipt.get("content").is_none(),
        "content already in context is not resent"
    );
    assert!(
        tool_output(3, "first").contains("original"),
        "the referenced result is in the same request"
    );
    let changed = tool_output(5, "changed");
    assert!(!changed.contains("repeated_unchanged"));
    assert!(changed.contains("changed"));
}

#[tokio::test]
async fn alternating_silent_shell_writes_remain_progress() {
    let server = MockServer::builder().start().await;
    let mut script = (0..5)
        .map(|i| {
            common::tool_call_body(
                &format!("append-{i}"),
                "shell",
                json!({"command":"sh","args":["-c", if i % 2 == 0 {
                    "printf x >> a.txt"
                } else {
                    "printf x >> b.txt"
                }],"cwd":""}),
            )
        })
        .collect::<Vec<_>>();
    script.push(common::final_text_body("Five writes completed"));
    common::mount_script(&server, script).await;
    let project = tempfile::tempdir().unwrap();
    let llm = resolve(
        &ProviderConfig::new(ProviderKind::Go, "test-key")
            .with_model("test-model")
            .with_base_url(server.uri()),
    )
    .await
    .unwrap();
    let approvals: Arc<dyn ApprovalHook> = Arc::new(AllowAllHook);
    let (tx, mut rx) = tokio::sync::mpsc::channel(128);
    let result = run_task_with_policy(
        llm,
        boxed_tools(project.path(), approvals.clone()).unwrap(),
        "Append five items".into(),
        vec![],
        approvals,
        RunPolicy {
            total_turns: 8,
            context_token_budget: 200_000,
            recent_messages: 20,
        },
        tx,
        Arc::new(AtomicBool::new(false)),
    )
    .await;
    assert!(result.is_ok(), "shell writes are actions: {result:?}");
    assert!(has_finished(&common::drain(&mut rx).await));
    assert_eq!(
        std::fs::read_to_string(project.path().join("a.txt")).unwrap(),
        "xxx"
    );
    assert_eq!(
        std::fs::read_to_string(project.path().join("b.txt")).unwrap(),
        "xx"
    );
}

#[tokio::test]
async fn shell_args_sent_as_a_json_string_run_as_the_list() {
    let server = MockServer::builder().start().await;
    common::mount_script(
        &server,
        vec![
            common::tool_call_body(
                "string-args",
                "shell",
                json!({"command":"sh","args":"[\"-c\", \"printf ok > a.txt\"]"}),
            ),
            common::final_text_body("Wrote a.txt"),
        ],
    )
    .await;
    let project = tempfile::tempdir().unwrap();
    let llm = resolve(
        &ProviderConfig::new(ProviderKind::Go, "test-key")
            .with_model("test-model")
            .with_base_url(server.uri()),
    )
    .await
    .unwrap();
    let approvals: Arc<dyn ApprovalHook> = Arc::new(AllowAllHook);
    let (tx, mut rx) = tokio::sync::mpsc::channel(128);
    let result = run_task_with_policy(
        llm,
        boxed_tools(project.path(), approvals.clone()).unwrap(),
        "Write a.txt".into(),
        vec![],
        approvals,
        RunPolicy {
            total_turns: 4,
            context_token_budget: 200_000,
            recent_messages: 20,
        },
        tx,
        Arc::new(AtomicBool::new(false)),
    )
    .await;
    assert!(result.is_ok(), "{result:?}");
    assert!(has_finished(&common::drain(&mut rx).await));
    assert_eq!(
        std::fs::read_to_string(project.path().join("a.txt")).unwrap(),
        "ok"
    );
}

#[tokio::test]
async fn an_unavailable_upstream_499_is_retried_and_the_run_finishes() {
    let server = MockServer::builder().start().await;
    // OpenCode Go's answer when its upstream endpoint is briefly unavailable.
    wiremock::Mock::given(wiremock::matchers::method("POST"))
        .respond_with(wiremock::ResponseTemplate::new(499).set_body_json(json!({
            "error": {"type": "server_error", "message": "Upstream request failed: Endpoint is unavailable."}
        })))
        .up_to_n_times(1)
        .with_priority(1)
        .mount(&server)
        .await;
    common::mount_script(&server, vec![common::final_text_body("Recovered")]).await;
    let llm = resolve(
        &ProviderConfig::new(ProviderKind::Go, "test-key")
            .with_model("test-model")
            .with_base_url(server.uri()),
    )
    .await
    .unwrap();
    let approvals: Arc<dyn ApprovalHook> = Arc::new(AllowAllHook);
    let project = tempfile::tempdir().unwrap();
    let (tx, mut rx) = tokio::sync::mpsc::channel(128);
    let result = run_task_with_policy(
        llm,
        boxed_tools(project.path(), approvals.clone()).unwrap(),
        "Say hello".into(),
        vec![],
        approvals,
        RunPolicy {
            total_turns: 4,
            context_token_budget: 200_000,
            recent_messages: 20,
        },
        tx,
        Arc::new(AtomicBool::new(false)),
    )
    .await;
    assert!(result.is_ok(), "{result:?}");
    assert!(has_finished(&common::drain(&mut rx).await));
    assert_eq!(server.received_requests().await.unwrap().len(), 2);
}

#[tokio::test]
async fn oversized_tool_schema_is_rejected_before_provider_request() {
    #[derive(Debug)]
    struct HugeSchema;
    #[themis_core::tools::async_trait]
    impl themis_core::tools::ToolRuntime for HugeSchema {
        async fn execute(
            &self,
            _: serde_json::Value,
        ) -> Result<serde_json::Value, themis_core::tools::ToolCallError> {
            Ok(json!(null))
        }
    }
    impl themis_core::tools::ToolT for HugeSchema {
        fn name(&self) -> &str {
            "huge_schema"
        }
        fn description(&self) -> &str {
            "Test a schema that alone exceeds the input budget"
        }
        fn args_schema(&self) -> serde_json::Value {
            json!({"type":"object","description":"schema".repeat(20_000)})
        }
    }
    let server = MockServer::builder().start().await;
    common::mount_script(
        &server,
        vec![common::final_text_body("Should not be requested")],
    )
    .await;
    let llm = resolve(
        &ProviderConfig::new(ProviderKind::Go, "test-key")
            .with_model("test-model")
            .with_base_url(server.uri()),
    )
    .await
    .unwrap();
    let (tx, mut rx) = tokio::sync::mpsc::channel(128);
    let result = run_task_with_policy(
        llm,
        vec![Box::new(HugeSchema)],
        "Hello".into(),
        vec![],
        Arc::new(AllowAllHook),
        RunPolicy {
            total_turns: 2,
            context_token_budget: 10_000,
            recent_messages: 20,
        },
        tx,
        Arc::new(AtomicBool::new(false)),
    )
    .await;
    assert!(
        result.is_err(),
        "tool schemas must count toward the complete request budget"
    );
    assert!(result.unwrap_err().to_string().contains("budget"));
    assert!(server.received_requests().await.unwrap().is_empty());
    assert!(common::drain(&mut rx)
        .await
        .iter()
        .any(|e| matches!(e,RunEvent::Failed{error} if error.contains("budget"))));
}

#[tokio::test]
async fn oversized_write_arguments_are_rejected_before_modifying_files() {
    let server = MockServer::builder().start().await;
    let content = "large write Ω".repeat(20_000);
    common::mount_script(
        &server,
        vec![
            common::tool_call_body(
                "write-large",
                "write_file",
                json!({"file_path":"keep.txt","content":content,"append":false}),
            ),
            common::final_text_body("Write rejected; use a generator"),
        ],
    )
    .await;
    let project = tempfile::tempdir().unwrap();
    std::fs::write(project.path().join("keep.txt"), "original").unwrap();
    let evidence = project.path().join(".themis/context/write");
    std::fs::create_dir_all(&evidence).unwrap();
    let llm = resolve(
        &ProviderConfig::new(ProviderKind::Go, "test-key")
            .with_model("test-model")
            .with_base_url(server.uri()),
    )
    .await
    .unwrap();
    let approvals: Arc<dyn ApprovalHook> = Arc::new(AllowAllHook);
    let (tx, mut rx) = tokio::sync::mpsc::channel(128);
    themis_core::runtime::run_task_with_evidence(
        llm,
        boxed_tools(project.path(), approvals.clone()).unwrap(),
        "Write a generated file".into(),
        vec![],
        approvals,
        RunPolicy {
            total_turns: 5,
            context_token_budget: 200_000,
            recent_messages: 20,
        },
        tx,
        Arc::new(AtomicBool::new(false)),
        String::new(),
        Some(evidence.clone()),
    )
    .await
    .unwrap();
    assert!(
        std::fs::read_to_string(project.path().join("keep.txt")).unwrap() == "original",
        "rejected arguments must leave the file untouched"
    );
    let requests = server.received_requests().await.unwrap();
    let body: serde_json::Value = serde_json::from_slice(&requests[1].body).unwrap();
    assert!(!body.to_string().contains(&content));
    assert!(body.to_string().contains("arguments_rejected"));
    let originals = std::fs::read_dir(evidence)
        .unwrap()
        .map(|e| std::fs::read_to_string(e.unwrap().path()).unwrap())
        .collect::<String>();
    assert!(originals.contains(&content));
    assert!(!common::drain(&mut rx)
        .await
        .iter()
        .any(|e| matches!(e, RunEvent::ContextCompacting)));
}

#[tokio::test]
async fn repeated_archive_reads_are_warned_without_another_compaction() {
    let server = MockServer::builder().start().await;
    let args =
        json!({"file_path":".themis/context/source/snapshot-original.jsonl","jsonl_record":1});
    let mut script = (0..8)
        .map(|i| common::tool_call_body(&format!("read-{i}"), "read_file", args.clone()))
        .collect::<Vec<_>>();
    script.push(common::final_text_body("Repeated the same evidence"));
    common::mount_script(&server, script).await;
    let project = tempfile::tempdir().unwrap();
    let evidence = project.path().join(".themis/context/source");
    std::fs::create_dir_all(&evidence).unwrap();
    std::fs::write(
        evidence.join("snapshot-original.jsonl"),
        json!({"role":"User","content":"Original fact: 17."}).to_string(),
    )
    .unwrap();
    let llm = resolve(
        &ProviderConfig::new(ProviderKind::Go, "test-key")
            .with_model("test-model")
            .with_base_url(server.uri()),
    )
    .await
    .unwrap();
    let approvals: Arc<dyn ApprovalHook> = Arc::new(AllowAllHook);
    let (tx, mut rx) = tokio::sync::mpsc::channel(128);
    let result = themis_core::runtime::run_task_with_evidence(
        llm,
        boxed_tools(project.path(), approvals.clone()).unwrap(),
        "Recover the original fact".into(),
        vec![],
        approvals,
        RunPolicy {
            total_turns: 20,
            context_token_budget: 200_000,
            recent_messages: 20,
        },
        tx,
        Arc::new(AtomicBool::new(false)),
        String::new(),
        Some(evidence),
    )
    .await;
    // Repeated archive reads are warned like any others and never stopped.
    assert_eq!(result.unwrap(), "Repeated the same evidence");
    let events = common::drain(&mut rx).await;
    assert!(!events
        .iter()
        .any(|e| matches!(e, RunEvent::ContextCompacting)));
    let requests = server.received_requests().await.unwrap();
    let last = String::from_utf8_lossy(&requests.last().unwrap().body).into_owned();
    assert_eq!(last.matches("Loop warning").count(), 2, "{last}");
}

#[tokio::test]
async fn repeated_ordinary_reads_are_warned_each_streak_and_never_stopped() {
    let server = MockServer::builder().start().await;
    let args = json!({"file_path":"story.jsonl","jsonl_record":1});
    let mut script = (0..8)
        .map(|i| common::tool_call_body(&format!("read-{i}"), "read_file", args.clone()))
        .collect::<Vec<_>>();
    script.push(common::final_text_body(&common::summary(
        "Repeated the same evidence",
    )));
    common::mount_script(&server, script).await;
    let project = tempfile::tempdir().unwrap();
    let evidence = project.path().join(".themis/context/source");
    std::fs::create_dir_all(&evidence).unwrap();
    std::fs::write(
        project.path().join("story.jsonl"),
        json!({"role":"User","content":"Original fact: 17."}).to_string(),
    )
    .unwrap();
    let llm = resolve(
        &ProviderConfig::new(ProviderKind::Go, "test-key")
            .with_model("test-model")
            .with_base_url(server.uri()),
    )
    .await
    .unwrap();
    let approvals: Arc<dyn ApprovalHook> = Arc::new(AllowAllHook);
    let (tx, mut rx) = tokio::sync::mpsc::channel(128);
    let result = themis_core::runtime::run_task_with_evidence(
        llm,
        boxed_tools(project.path(), approvals.clone()).unwrap(),
        "Recover the original fact".into(),
        vec![],
        approvals,
        RunPolicy {
            total_turns: 20,
            context_token_budget: 200_000,
            recent_messages: 20,
        },
        tx,
        Arc::new(AtomicBool::new(false)),
        String::new(),
        Some(evidence),
    )
    .await;
    // Repeats are warned, never stopped: the run finishes with the model's own answer.
    assert_eq!(
        result.unwrap(),
        common::summary("Repeated the same evidence")
    );
    let events = common::drain(&mut rx).await;
    assert!(!events.iter().any(|e| matches!(e, RunEvent::Failed { .. })));
    let requests = server.received_requests().await.unwrap();
    assert_eq!(requests.len(), 9, "eight reads, then the answer");
    // One new read, then seven repeats: streaks end at the third and sixth repeat.
    let last = String::from_utf8_lossy(&requests[8].body).into_owned();
    assert_eq!(last.matches("Loop warning").count(), 2, "{last}");
    assert!(!last.contains("stops the run"));
    // The old read-one-write-one instruction is gone from the provider request.
    let first: serde_json::Value = requests[0].body_json().unwrap();
    assert!(
        first["messages"][0]["role"] == "system"
            && !first["messages"][0]["content"]
                .as_str()
                .unwrap()
                .contains("before reading the next few")
    );
}

#[tokio::test]
async fn alternating_unchanged_reads_are_warned_after_compaction_at_both_budgets() {
    for budget in [24_000, 200_000] {
        let server = MockServer::builder().start().await;
        let mut script = (0..8)
            .map(|i| {
                common::tool_call_body(
                    &format!("read-{i}"),
                    "read_file",
                    json!({"file_path":if i % 2 == 0 {"a.txt"} else {"b.txt"}}),
                )
            })
            .collect::<Vec<_>>();
        script.push(common::final_text_body("Finished alternating observations"));
        common::mount_script_with_summary(
            &server,
            script,
            Some("Both sources remain available; inspect them once."),
        )
        .await;
        let project = tempfile::tempdir().unwrap();
        std::fs::write(project.path().join("a.txt"), "A").unwrap();
        std::fs::write(project.path().join("b.txt"), "B").unwrap();
        let llm = resolve(
            &ProviderConfig::new(ProviderKind::Go, "test-key")
                .with_model("test-model")
                .with_base_url(server.uri()),
        )
        .await
        .unwrap();
        let approvals: Arc<dyn ApprovalHook> = Arc::new(AllowAllHook);
        let evidence = project.path().join(".themis/context/alternating");
        std::fs::create_dir_all(&evidence).unwrap();
        let (tx, mut rx) = tokio::sync::mpsc::channel(128);
        let result = themis_core::runtime::run_task_with_evidence(
            llm,
            boxed_tools(project.path(), approvals.clone()).unwrap(),
            "Inspect both files".into(),
            vec![themis_core::runtime::ConversationTurn {
                role: themis_core::runtime::ConversationRole::User,
                text: "historical context ".repeat(budget),
            }],
            approvals,
            RunPolicy {
                total_turns: 20,
                context_token_budget: budget,
                recent_messages: 2,
            },
            tx,
            Arc::new(AtomicBool::new(false)),
            String::new(),
            Some(evidence),
        )
        .await;
        assert_eq!(
            result.unwrap(),
            "Finished alternating observations",
            "alternating unchanged reads are warned, not stopped, at {budget}"
        );
        let events = common::drain(&mut rx).await;
        assert_eq!(
            events
                .iter()
                .filter(|e| matches!(e, RunEvent::ContextCompacting))
                .count(),
            1
        );
        assert_eq!(
            events
                .iter()
                .filter(|e| matches!(e, RunEvent::ToolCallFinished { .. }))
                .count(),
            8,
            "two new reads, then six repeats in two warned streaks"
        );
        assert!(has_finished(&events));
        let requests = server.received_requests().await.unwrap();
        let warned = requests
            .iter()
            .map(|request| String::from_utf8_lossy(&request.body).into_owned())
            .rfind(|body| !body.contains("You summarize agent context"))
            .unwrap();
        assert_eq!(warned.matches("Loop warning").count(), 2, "at {budget}");
    }
}

#[tokio::test]
async fn context_within_budget_keeps_exact_tool_result() {
    let server = MockServer::builder().start().await;
    common::mount_script(
        &server,
        vec![
            common::tool_call_body("read_1", "read_file", json!({"file_path":"note.txt"})),
            common::final_text_body(&common::summary("Answered from the original evidence")),
            common::final_text_body(&common::summary("Answered after a lossy checkpoint")),
        ],
    )
    .await;
    let dir = tempfile::tempdir().unwrap();
    let original = "The letter was WRITTEN September 6 and DELIVERED September 8, after the death.";
    std::fs::write(dir.path().join("note.txt"), original).unwrap();
    let llm = resolve(
        &ProviderConfig::new(ProviderKind::Go, "test-key")
            .with_model("test-model")
            .with_base_url(server.uri()),
    )
    .await
    .unwrap();
    let approvals: Arc<dyn ApprovalHook> = Arc::new(AllowAllHook);
    let tools = boxed_tools(dir.path(), Arc::clone(&approvals)).unwrap();
    let (tx, mut rx) = tokio::sync::mpsc::channel(1024);
    let answer = run_task_with_policy(
        llm,
        tools,
        "When did the letter arrive?".into(),
        vec![],
        approvals,
        RunPolicy {
            total_turns: 3,
            context_token_budget: 200000,
            recent_messages: 1,
        },
        tx,
        Arc::new(AtomicBool::new(false)),
    )
    .await
    .unwrap();
    let events = common::drain(&mut rx).await;
    assert!(
        !events
            .iter()
            .any(|event| matches!(event, RunEvent::ContextCheckpoint { .. })),
        "context within budget must not be summarized solely because a segment ended: {events:?}"
    );
    assert_eq!(
        answer,
        common::summary("Answered from the original evidence")
    );
    let requests = server.received_requests().await.unwrap();
    assert_eq!(requests.len(), 2);
    let request: serde_json::Value = serde_json::from_slice(&requests[1].body).unwrap();
    assert!(request["messages"]
        .as_array()
        .unwrap()
        .iter()
        .any(|message| {
            message["role"] == "tool"
                && message["content"]
                    .as_str()
                    .is_some_and(|text| text.contains(original))
        }));
    assert!(has_finished(&events));
}
