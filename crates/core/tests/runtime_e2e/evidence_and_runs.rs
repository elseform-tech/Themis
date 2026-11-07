//! Maintenance, tool evidence, handoffs, approvals, skills and stopping runs.

use super::*;

#[tokio::test]
async fn maintenance_continues_same_request_without_unnecessary_checkpoint() {
    let server = MockServer::builder().start().await;
    common::mount_script_with_summary(
        &server,
        vec![
            {
                let mut call =
                    common::tool_call_body("call_1", "read_file", json!({"file_path":"note.txt"}));
                call["choices"][0]["message"]["content"] =
                    "progress narration ".repeat(2600).into();
                call
            },
            common::final_text_body(&common::summary("Done after checkpoint")),
        ],
        Some("Read note.txt; answer the original request next."),
    )
    .await;
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("note.txt"), "hello".repeat(3000)).unwrap();
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
    std::fs::create_dir_all(dir.path().join(".themis/context/test")).unwrap();
    let answer = themis_core::runtime::run_task_with_evidence(
        llm,
        tools,
        "Read the note".into(),
        vec![],
        approvals,
        RunPolicy {
            total_turns: 3,
            context_token_budget: 16_000,
            recent_messages: 4,
        },
        tx,
        Arc::new(AtomicBool::new(false)),
        String::new(),
        Some(dir.path().join(".themis/context/test")),
    )
    .await
    .unwrap();
    assert_eq!(answer, common::summary("Done after checkpoint"));
    let events = common::drain(&mut rx).await;
    assert!(
        !events
            .iter()
            .any(|event| matches!(event, RunEvent::ContextCheckpoint { .. })),
        "cheap maintenance should remove bulk before semantic compaction"
    );
    assert!(has_finished(&events));
    assert_eq!(
        events
            .iter()
            .filter(|event| matches!(event, RunEvent::ToolCallFinished { .. }))
            .count(),
        1
    );
    let requests = server.received_requests().await.unwrap();
    assert_eq!(requests.len(), 2, "no summarizer or repeated tool request");
    let final_request: serde_json::Value = requests.last().unwrap().body_json().unwrap();
    let messages = final_request["messages"].as_array().unwrap();
    assert!(messages
        .iter()
        .any(|message| message["role"] == "user" && message["content"] == "Read the note"));
    assert!(messages.iter().any(|message| message["role"] == "tool"));
}

#[tokio::test]
async fn tool_evidence_survives_completed_runs_without_compaction() {
    let server = MockServer::builder().start().await;
    common::mount_script(
        &server,
        vec![
            common::tool_call_body(
                "observed_1",
                "read_file",
                json!({"file_path":"observation.txt","find":"historical receipt:"}),
            ),
            common::final_text_body("READY"),
        ],
    )
    .await;
    let project = tempfile::tempdir().unwrap();
    let original = format!(
        "{}historical receipt: copper-753",
        "observed source line\n".repeat(5000)
    );
    std::fs::write(project.path().join("observation.txt"), &original).unwrap();
    let evidence = project.path().join("evidence");
    std::fs::create_dir(&evidence).unwrap();
    let llm = resolve(
        &ProviderConfig::new(ProviderKind::Go, "test-key")
            .with_model("test-model")
            .with_base_url(server.uri()),
    )
    .await
    .unwrap();
    let approvals: Arc<dyn ApprovalHook> = Arc::new(AllowAllHook);
    let (tx, mut rx) = tokio::sync::mpsc::channel(1024);
    let result = themis_core::runtime::run_task_with_evidence(
        llm,
        boxed_tools(project.path(), approvals.clone()).unwrap(),
        "Read observation.txt then reply only READY".into(),
        vec![],
        approvals,
        RunPolicy {
            total_turns: 5,
            context_token_budget: 200_000,
            recent_messages: 4,
        },
        tx,
        Arc::new(AtomicBool::new(false)),
        String::new(),
        Some(evidence.clone()),
    )
    .await
    .unwrap();
    assert_eq!(result, "READY");
    let events = common::drain(&mut rx).await;
    assert!(!events
        .iter()
        .any(|event| matches!(event, RunEvent::ContextCheckpoint { .. })));
    assert!(events.iter().any(|event| matches!(event, RunEvent::ToolCallFinished { output, .. } if output.contains("copper-753"))));
    std::fs::write(
        project.path().join("observation.txt"),
        "current receipt: silver-864",
    )
    .unwrap();
    let snapshots: Vec<_> = std::fs::read_dir(&evidence)
        .unwrap()
        .map(|entry| entry.unwrap().path())
        .collect();
    assert_eq!(
        snapshots.len(),
        1,
        "completed tool evidence must survive even without a checkpoint"
    );
    let records: Vec<serde_json::Value> = std::fs::read_to_string(&snapshots[0])
        .unwrap()
        .lines()
        .map(|line| serde_json::from_str(line).unwrap())
        .collect();
    assert_eq!(records[0]["message_type"]["ToolUse"][0]["id"], "observed_1");
    let output = records[1]["message_type"]["ToolResult"][0]["function"]["arguments"]
        .as_str()
        .unwrap();
    let observed: serde_json::Value = serde_json::from_str(output).unwrap();
    assert!(observed["content"].as_str().unwrap().contains("copper-753"));
    assert!(!observed["content"].as_str().unwrap().contains("silver-864"));
    assert!(observed["content"].as_str().unwrap().chars().count() <= 4000);
}

#[tokio::test]
async fn failed_checkpoint_archive_does_not_advertise_a_saved_job() {
    let server = MockServer::builder().start().await;
    let project = tempfile::tempdir().unwrap();
    let evidence = project.path().join("evidence");
    std::fs::write(&evidence, "cannot hold snapshots").unwrap();
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
        "Continue the task".into(),
        vec![themis_core::runtime::ConversationTurn {
            role: themis_core::runtime::ConversationRole::User,
            text: "Retain the original task constraint. ".repeat(4000),
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
    .await;
    let error = result.unwrap_err().to_string();
    assert!(error.starts_with("Context checkpoint failed:"), "{error}");
    assert!(!error.contains("/retry-compaction"), "{error}");
    assert!(!error.contains("Originals preserved"), "{error}");
    let events = common::drain(&mut rx).await;
    assert!(events
        .iter()
        .any(|event| matches!(event, RunEvent::Failed { error: emitted } if emitted == &error)));
    assert!(!has_finished(&events));
    assert!(server.received_requests().await.unwrap().is_empty());
}

#[tokio::test]
async fn failed_tool_evidence_save_stops_before_model_continuation() {
    let server = MockServer::builder().start().await;
    common::mount_script(
        &server,
        vec![
            common::tool_call_body("read_1", "read_file", json!({"file_path":"note.txt"})),
            common::final_text_body("Must not continue"),
        ],
    )
    .await;
    let project = tempfile::tempdir().unwrap();
    std::fs::write(project.path().join("note.txt"), "historical evidence").unwrap();
    let evidence = project.path().join("evidence");
    std::fs::write(&evidence, "a file cannot hold snapshots").unwrap();
    let llm = resolve(
        &ProviderConfig::new(ProviderKind::Go, "test-key")
            .with_model("test-model")
            .with_base_url(server.uri()),
    )
    .await
    .unwrap();
    let approvals: Arc<dyn ApprovalHook> = Arc::new(AllowAllHook);
    let (tx, mut rx) = tokio::sync::mpsc::channel(1024);
    let result = themis_core::runtime::run_task_with_evidence(
        llm,
        boxed_tools(project.path(), approvals.clone()).unwrap(),
        "Read note.txt".into(),
        vec![],
        approvals,
        RunPolicy {
            total_turns: 5,
            context_token_budget: 200_000,
            recent_messages: 4,
        },
        tx,
        Arc::new(AtomicBool::new(false)),
        String::new(),
        Some(evidence),
    )
    .await;
    assert!(result.is_err());
    let events = common::drain(&mut rx).await;
    assert!(events.iter().any(|event| matches!(event, RunEvent::Failed { error } if error.starts_with("Tool evidence failed:"))));
    assert!(
        events
            .iter()
            .any(|event| matches!(event, RunEvent::ToolCallFinished { ok: true, .. })),
        "the completed read must be reported even when saving fails"
    );
    assert!(!has_finished(&events));
    assert_eq!(server.received_requests().await.unwrap().len(), 1);
}

#[tokio::test]
async fn evidence_snapshots_preserve_tool_results_across_checkpoints() {
    let server = MockServer::builder().start().await;
    common::mount_script_with_summary(
        &server,
        vec![
            {
                let mut call =
                    common::tool_call_body("read_1", "read_file", json!({"file_path":"note.txt"}));
                call["choices"][0]["message"]["content"] =
                    "progress narration ".repeat(2600).into();
                call
            },
            {
                let mut call =
                    common::tool_call_body("read_2", "read_file", json!({"file_path":"other.txt"}));
                call["choices"][0]["message"]["content"] =
                    "progress narration ".repeat(2600).into();
                call
            },
            common::final_text_body("Done"),
        ],
        Some("Read notes; originals remain recoverable."),
    )
    .await;
    let dir = tempfile::tempdir().unwrap();
    let evidence = dir.path().join("evidence");
    std::fs::create_dir(&evidence).unwrap();
    std::fs::write(
        dir.path().join("note.txt"),
        format!("original tool evidence: red-753 {}", "x".repeat(12000)),
    )
    .unwrap();
    std::fs::write(
        dir.path().join("other.txt"),
        format!("second tool evidence: blue-864 {}", "x".repeat(12000)),
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
    let (tx, mut rx) = tokio::sync::mpsc::channel(1024);
    themis_core::runtime::run_task_with_evidence(
        llm,
        boxed_tools(dir.path(), approvals.clone()).unwrap(),
        "Read both notes".into(),
        vec![themis_core::runtime::ConversationTurn {
            role: themis_core::runtime::ConversationRole::User,
            text: "Historical semantic state ".repeat(4000),
        }],
        approvals,
        RunPolicy {
            total_turns: 3,
            context_token_budget: 16_000,
            recent_messages: 4,
        },
        tx,
        Arc::new(AtomicBool::new(false)),
        String::new(),
        Some(evidence.clone()),
    )
    .await
    .unwrap();
    let snapshots: Vec<_> = std::fs::read_dir(&evidence)
        .unwrap()
        .map(|entry| entry.unwrap().path())
        .filter(|path| {
            path.extension()
                .is_some_and(|extension| extension == "jsonl")
        })
        .collect();
    let events = common::drain(&mut rx).await;
    let checkpoints = events
        .iter()
        .filter_map(|event| match event {
            RunEvent::ContextCheckpoint { summary } => Some(summary),
            _ => None,
        })
        .collect::<Vec<_>>();
    assert_eq!(
        checkpoints.len(),
        1,
        "historical semantic pressure compacts once; tool bulk uses maintenance"
    );
    assert!(checkpoints[0].contains(&serde_json::to_string(&evidence).unwrap()));
    assert!(
        snapshots.len() >= 3,
        "checkpoint and both completed observations must be durable"
    );
    for fact in ["red-753", "blue-864"] {
        assert!(
            snapshots
                .iter()
                .any(
                    |path| std::fs::read_to_string(path).unwrap().lines().any(|line| {
                        let record: serde_json::Value = serde_json::from_str(line).unwrap();
                        record["role"] == "Tool" && record.to_string().contains(fact)
                    })
                ),
            "original observed source must survive: {fact}"
        );
    }
    assert_eq!(
        events
            .iter()
            .filter(|event| matches!(event, RunEvent::ToolCallFinished { .. }))
            .count(),
        2,
        "compaction must not replay completed reads"
    );
}

#[tokio::test]
async fn hard_cap_returns_natural_handoff_without_failing() {
    let server = MockServer::builder().start().await;
    common::mount_script(
        &server,
        vec![
            common::tool_call_body(
                "call_1",
                "write_file",
                json!({"file_path":"progress.txt","content":"step 1\n","append":true}),
            ),
            common::final_text_body(
                "I wrote step 1 to progress.txt. Next, write step 2 and verify the file.",
            ),
        ],
    )
    .await;
    let dir = tempfile::tempdir().unwrap();
    let approvals: Arc<dyn ApprovalHook> = Arc::new(AllowAllHook);
    let tools = boxed_tools(dir.path(), Arc::clone(&approvals)).unwrap();
    let llm = resolve(
        &ProviderConfig::new(ProviderKind::Go, "test-key")
            .with_model("test-model")
            .with_base_url(server.uri()),
    )
    .await
    .unwrap();
    let (tx, mut rx) = tokio::sync::mpsc::channel(1024);
    let answer = run_task_with_policy(
        llm,
        tools,
        "Write ordered steps".into(),
        vec![],
        approvals,
        RunPolicy {
            total_turns: 1,
            context_token_budget: 16_000,
            recent_messages: 4,
        },
        tx,
        Arc::new(AtomicBool::new(false)),
    )
    .await
    .unwrap();
    assert_eq!(
        answer,
        "I wrote step 1 to progress.txt. Next, write step 2 and verify the file."
    );
    assert_eq!(
        std::fs::read_to_string(dir.path().join("progress.txt")).unwrap(),
        "step 1\n"
    );
    let events = common::drain(&mut rx).await;
    assert!(
        !events
            .iter()
            .any(|event| matches!(event, RunEvent::ContextCheckpoint { .. })),
        "a turn cap alone must not trigger compaction"
    );
    assert!(events
        .iter()
        .any(|event| matches!(event, RunEvent::Incomplete { .. })));
    assert!(!events
        .iter()
        .any(|event| matches!(event, RunEvent::Failed { .. })));
}

#[tokio::test]
async fn empty_handoff_does_not_claim_the_task_was_completed() {
    let server = MockServer::builder().start().await;
    common::mount_script(
        &server,
        vec![
            common::tool_call_body(
                "call_1",
                "write_file",
                json!({"file_path":"progress.txt","content":"step 1\n","append":true}),
            ),
            common::final_text_body(""),
        ],
    )
    .await;
    let dir = tempfile::tempdir().unwrap();
    let approvals: Arc<dyn ApprovalHook> = Arc::new(AllowAllHook);
    let tools = boxed_tools(dir.path(), Arc::clone(&approvals)).unwrap();
    let llm = resolve(
        &ProviderConfig::new(ProviderKind::Go, "test-key")
            .with_model("test-model")
            .with_base_url(server.uri()),
    )
    .await
    .unwrap();
    let (tx, _rx) = tokio::sync::mpsc::channel(1024);
    let answer = run_task_with_policy(
        llm,
        tools,
        "Write ordered steps".into(),
        vec![],
        approvals,
        RunPolicy {
            total_turns: 1,
            context_token_budget: 16_000,
            recent_messages: 4,
        },
        tx,
        Arc::new(AtomicBool::new(false)),
    )
    .await
    .unwrap();

    assert_eq!(answer, "I couldn't produce a final summary. Please review the recorded changes and choose the next step.");
}

#[tokio::test]
async fn mock_multi_turn_edit_writes_file_and_finishes() {
    let server = MockServer::builder().start().await;
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().join("proj");
    std::fs::create_dir(&root).unwrap();
    std::fs::write(root.join("hello.txt"), "before\n").unwrap();

    common::mount_script(
        &server,
        vec![
            common::tool_call_body(
                "call_1",
                "write_file",
                json!({"file_path": "hello.txt", "content": "after\n", "append": false}),
            ),
            common::final_text_body("done"),
        ],
    )
    .await;

    let config = ProviderConfig::new(ProviderKind::Go, "test-key")
        .with_model("test-model")
        .with_base_url(server.uri());
    let llm = resolve(&config).await.unwrap();
    let approvals: Arc<dyn ApprovalHook> = Arc::new(AllowAllHook);
    let tools = boxed_tools(&root, Arc::clone(&approvals)).unwrap();

    let (events_tx, mut events_rx) = tokio::sync::mpsc::channel(1024);
    let result = run_task(
        llm,
        tools,
        "replace hello.txt with the new greeting".to_string(),
        approvals,
        5,
        events_tx,
    )
    .await
    .unwrap();

    let events = common::drain(&mut events_rx).await;
    assert_eq!(
        std::fs::read_to_string(root.join("hello.txt")).unwrap(),
        "after\n"
    );
    assert!(has_finished(&events), "events: {events:?}");
    assert!(
        events.iter().any(
            |event| matches!(event, RunEvent::ToolCallFinished { tool, ok: true, .. } if tool == "write_file")
        ),
        "events: {events:?}"
    );
    assert_eq!(result, "done");
}

#[tokio::test]
async fn deny_all_blocks_shell_and_terminates() {
    let server = MockServer::builder().start().await;
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().join("proj");
    std::fs::create_dir(&root).unwrap();

    // A stubborn agent: the script repeats the shell call every turn.
    common::mount_script(
        &server,
        vec![common::tool_call_body(
            "call_1",
            "shell",
            json!({"command": "touch", "args": ["pwned.txt"], "cwd": ""}),
        )],
    )
    .await;

    let config = ProviderConfig::new(ProviderKind::Go, "test-key")
        .with_model("test-model")
        .with_base_url(server.uri());
    let llm = resolve(&config).await.unwrap();
    let approvals: Arc<dyn ApprovalHook> = Arc::new(DenyAllHook);
    let tools = boxed_tools(&root, Arc::clone(&approvals)).unwrap();

    let (events_tx, mut events_rx) = tokio::sync::mpsc::channel(1024);
    let error = run_task(
        llm,
        tools,
        "touch pwned.txt via the shell".to_string(),
        approvals,
        2,
        events_tx,
    )
    .await
    .unwrap_err();

    let events = common::drain(&mut events_rx).await;
    assert!(
        !root.join("pwned.txt").exists(),
        "denied shell call must not run"
    );
    assert!(
        events.iter().any(
            |event| matches!(event, RunEvent::ApprovalDecided { tool, decision: ApprovalDecision::Deny } if tool == "shell")
        ),
        "events: {events:?}"
    );
    assert!(
        !events
            .iter()
            .any(|event| matches!(event, RunEvent::ToolCallFinished { ok: true, .. })),
        "events: {events:?}"
    );
    assert!(
        events
            .iter()
            .any(|event| matches!(event, RunEvent::Failed { .. })),
        "events: {events:?}"
    );
    assert!(error.to_string().contains("max turns"), "{error}");
}

#[tokio::test]
#[ignore]
async fn live_go_chat_completions_run() {
    let key = std::env::var("THEMIS_GO_API_KEY")
        .ok()
        .filter(|key| !key.trim().is_empty());
    let model = std::env::var("THEMIS_GO_MODEL")
        .ok()
        .filter(|model| !model.trim().is_empty());
    let (Some(key), Some(model)) = (key, model) else {
        eprintln!("skipping live Go test: set THEMIS_GO_API_KEY and THEMIS_GO_MODEL to run");
        return;
    };

    let tmp = tempfile::tempdir().unwrap();
    let config = ProviderConfig::new(ProviderKind::Go, key).with_model(model);
    let llm = resolve(&config).await.unwrap();
    let approvals: Arc<dyn ApprovalHook> = Arc::new(AllowAllHook);
    let tools = boxed_tools(tmp.path(), Arc::clone(&approvals)).unwrap();

    let (events_tx, mut events_rx) = tokio::sync::mpsc::channel(1024);
    let result = run_task(
        llm,
        tools,
        "Reply with exactly: LIVE-OK".to_string(),
        approvals,
        3,
        events_tx,
    )
    .await
    .unwrap();

    let events = common::drain(&mut events_rx).await;
    assert!(has_finished(&events), "events: {events:?}");
    assert!(!result.trim().is_empty(), "events: {events:?}");
}

#[tokio::test]
async fn skill_catalog_reaches_system_request_without_body() {
    let server = MockServer::builder().start().await;
    let llm = resolve(
        &ProviderConfig::new(ProviderKind::Go, "test-key")
            .with_model("test-model")
            .with_base_url(server.uri()),
    )
    .await
    .unwrap();
    let root = tempfile::tempdir().unwrap();
    let skill = themis_core::skills::Skill {
        id: "review".into(),
        name: "Review".into(),
        description: "Inspect changes".into(),
        instructions: "BODY_MUST_REMAIN_LAZY".into(),
        allowed_tools: vec![],
        scripts: vec![],
    };
    let catalog =
        themis_core::skills::materialize_catalog(&[skill], root.path(), "catalog-run").unwrap();
    let entries: serde_json::Value = serde_json::from_str(
        catalog
            .split("Available skills (metadata only): ")
            .nth(1)
            .unwrap()
            .lines()
            .next()
            .unwrap(),
    )
    .unwrap();
    let path = entries[0]["path"].as_str().unwrap().to_owned();
    common::mount_script(
        &server,
        vec![
            common::tool_call_body("read_skill", "read_file", json!({"file_path": path})),
            common::final_text_body("Done"),
        ],
    )
    .await;
    let (tx, mut rx) = tokio::sync::mpsc::channel(1024);
    themis_core::runtime::run_task_with_policy_and_catalog(
        llm,
        boxed_tools(root.path(), Arc::new(AllowAllHook)).unwrap(),
        "Inspect changes".into(),
        vec![],
        Arc::new(AllowAllHook),
        RunPolicy {
            total_turns: 2,
            context_token_budget: usize::MAX,
            recent_messages: 4,
        },
        tx,
        Arc::new(AtomicBool::new(false)),
        catalog,
    )
    .await
    .unwrap();
    common::drain(&mut rx).await;
    let requests = server.received_requests().await.unwrap();
    let request: serde_json::Value = serde_json::from_slice(&requests[0].body).unwrap();
    let system = request["messages"][0]["content"].as_str().unwrap();
    assert_eq!(request["messages"][0]["role"], "system");
    assert!(system.contains("Inspect changes"));
    assert!(system.contains(&serde_json::to_string(&path).unwrap()));
    assert!(std::path::Path::new(&path).ends_with(std::path::Path::new("review").join("SKILL.md")));
    let followup: serde_json::Value = serde_json::from_slice(&requests[1].body).unwrap();
    for packet in [&request, &followup] {
        let system = packet["messages"][0]["content"].as_str().unwrap();
        assert_eq!(system.matches("For factual and technical work").count(), 1);
        assert!(system.contains("verified current line or line range"));
        assert!(system.contains("why it addresses the task"));
        assert!(system.contains("using permitted reference fields without extra wrappers"));
    }
    assert!(followup["messages"]
        .as_array()
        .unwrap()
        .iter()
        .any(|message| message["role"] == "tool"
            && message["content"]
                .as_str()
                .unwrap_or("")
                .contains("BODY_MUST_REMAIN_LAZY")));
    assert!(!system.contains("BODY_MUST_REMAIN_LAZY"));
}

async fn stopped_provider_request_finishes(compacting: bool) {
    use std::sync::atomic::Ordering;
    use std::time::Duration;
    use themis_core::runtime::{ConversationRole, ConversationTurn};
    use wiremock::{matchers::method, Mock, ResponseTemplate};
    let server = MockServer::builder().start().await;
    Mock::given(method("POST"))
        .respond_with(
            ResponseTemplate::new(200)
                .set_delay(Duration::from_secs(30))
                .set_body_json(common::final_text_body("Too late")),
        )
        .mount(&server)
        .await;
    let llm = resolve(
        &ProviderConfig::new(ProviderKind::Go, "test-key")
            .with_model("test-model")
            .with_base_url(server.uri()),
    )
    .await
    .unwrap();
    let stopped = Arc::new(AtomicBool::new(false));
    let (tx, mut rx) = tokio::sync::mpsc::channel(1024);
    let history = if compacting {
        vec![ConversationTurn {
            role: ConversationRole::User,
            text: "old context ".repeat(5000),
        }]
    } else {
        vec![]
    };
    let archive = tempfile::tempdir().unwrap();
    let run = tokio::spawn(themis_core::runtime::run_task_with_evidence(
        llm,
        vec![],
        "Continue".into(),
        history,
        Arc::new(AllowAllHook),
        RunPolicy {
            total_turns: 5,
            context_token_budget: if compacting { 16_000 } else { usize::MAX },
            recent_messages: 4,
        },
        tx,
        stopped.clone(),
        String::new(),
        Some(archive.path().to_owned()),
    ));
    let mut events = Vec::new();
    loop {
        let event = tokio::time::timeout(Duration::from_secs(2), rx.recv())
            .await
            .unwrap()
            .unwrap();
        let ready = if compacting {
            matches!(event, RunEvent::ContextCompacting)
        } else {
            matches!(event, RunEvent::Started { .. })
        };
        events.push(event);
        if ready {
            break;
        }
    }
    tokio::time::timeout(Duration::from_secs(2), async {
        while server.received_requests().await.unwrap().is_empty() {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .unwrap();
    stopped.store(true, Ordering::SeqCst);
    let result = tokio::time::timeout(Duration::from_millis(500), run)
        .await
        .expect("Stop must preempt a stalled provider request")
        .unwrap();
    assert!(result.unwrap_err().to_string().contains("Stopped by you"));
    events.extend(common::drain(&mut rx).await);
    assert_eq!(
        events
            .iter()
            .filter(|event| matches!(event, RunEvent::Failed { .. }))
            .count(),
        1,
        "{events:?}"
    );
    assert!(!events.iter().any(|event| matches!(
        event,
        RunEvent::ContextCheckpoint { .. } | RunEvent::Finished { .. }
    )));
}

#[tokio::test]
async fn stop_cancels_stalled_compaction_provider_request() {
    stopped_provider_request_finishes(true).await;
}

#[tokio::test]
async fn stop_cancels_stalled_answer_provider_request() {
    stopped_provider_request_finishes(false).await;
}

#[tokio::test]
async fn bounded_file_read_prevents_compaction_and_keeps_completion() {
    use wiremock::{matchers::method, Mock, Request, ResponseTemplate};
    let server = MockServer::builder().start().await;
    Mock::given(method("POST"))
        .respond_with(|request: &Request| {
            let body: serde_json::Value = serde_json::from_slice(&request.body).unwrap();
            let messages = body["messages"].as_array().unwrap();
            let summary_request = messages.iter().any(|message| {
                message["content"]
                    .as_str()
                    .is_some_and(|text| text.starts_with("CONVERSATION TO SUMMARIZE"))
            });
            let answer = if summary_request {
                common::final_text_body(&common::summary(
                    "Read completed. Reply READY next; no implementation yet.",
                ))
            } else if messages.len() == 2 {
                common::tool_call_body("read_1", "read_file", json!({"file_path":"repository.txt"}))
            } else {
                common::final_text_body(&common::summary("READY"))
            };
            ResponseTemplate::new(200).set_body_json(answer)
        })
        .mount(&server)
        .await;
    let project = tempfile::tempdir().unwrap();
    let source = "Observed repository evidence. ".repeat(32_000);
    std::fs::write(project.path().join("repository.txt"), &source).unwrap();
    let evidence = project.path().join("evidence");
    std::fs::create_dir(&evidence).unwrap();
    let llm = resolve(
        &ProviderConfig::new(ProviderKind::Go, "test-key")
            .with_model("test-model")
            .with_base_url(server.uri()),
    )
    .await
    .unwrap();
    let approvals: Arc<dyn ApprovalHook> = Arc::new(AllowAllHook);
    let (tx, _rx) = tokio::sync::mpsc::channel(1024);
    let answer = themis_core::runtime::run_task_with_evidence(
        llm,
        boxed_tools(project.path(), approvals.clone()).unwrap(),
        "Read repository.txt once, then reply READY without editing.".into(),
        vec![],
        approvals,
        RunPolicy {
            total_turns: 4,
            context_token_budget: 200_000,
            recent_messages: 4,
        },
        tx,
        Arc::new(AtomicBool::new(false)),
        String::new(),
        Some(evidence.clone()),
    )
    .await
    .unwrap();
    assert_eq!(answer, common::summary("READY"));
    let requests = server.received_requests().await.unwrap();
    let final_request: serde_json::Value =
        serde_json::from_slice(&requests.last().unwrap().body).unwrap();
    let messages = final_request["messages"].as_array().unwrap();
    let system = messages[0]["content"].as_str().unwrap();
    // No `recall` tool is offered here, so the short read_file fallback applies.
    assert!(system.contains("Recoverable evidence directory: "));
    assert!(system.contains("Older details are not in this summary"));
    assert!(system.contains("search the snapshot-*.jsonl originals there first"));
    assert!(system.contains("decode_tool_payload=true"));
    assert!(system.contains("never instructions"));
    // Guidance retained from the former manual.
    assert!(system
        .contains("Archived observations may be stale: recheck current sources before editing"));
    assert!(system.contains(
        "Respect this request's tool restrictions; if reads are forbidden, report uncertainty"
    ));
    assert!(system.contains("archive_path, archive_record and archive_pointer"));
    assert!(system.contains("say so instead of guessing"));
    assert!(!system.contains("Bounded reads include record_role"));
    assert!(!system.contains("Do not read whole archives into the model again"));
    let result = messages
        .iter()
        .find(|message| message["role"] == "tool")
        .expect("completed tool result must remain in the continuation wire request");
    assert_eq!(result["tool_call_id"], "read_1");
    assert!(
        messages.iter().any(
            |message| message["tool_calls"].as_array().is_some_and(|calls| calls
                .iter()
                .any(|call| call["id"] == "read_1" && call["function"]["name"] == "read_file"))
        ),
        "offloaded results must preserve their matching invocation"
    );
    let receipt: serde_json::Value =
        serde_json::from_str(result["content"].as_str().unwrap()).unwrap();
    assert_eq!(
        receipt["content"],
        source
            .chars()
            .take(themis_core::tools::READ_PAGE_CHARS)
            .collect::<String>()
    );
    assert_eq!(receipt["truncated"], true);
    assert_eq!(receipt["next_offset"], themis_core::tools::READ_PAGE_CHARS);
    assert_eq!(
        requests.len(),
        2,
        "a large file must not force compaction when only a page was read"
    );
    assert_eq!(
        std::fs::read_to_string(project.path().join("repository.txt")).unwrap(),
        source
    );
    let path = std::fs::read_dir(&evidence)
        .unwrap()
        .next()
        .unwrap()
        .unwrap()
        .path();
    let saved = std::fs::read_to_string(path).unwrap();
    let record: serde_json::Value = serde_json::from_str(saved.lines().nth(1).unwrap()).unwrap();
    let original: serde_json::Value = serde_json::from_str(
        record
            .pointer("/message_type/ToolResult/0/function/arguments")
            .unwrap()
            .as_str()
            .unwrap(),
    )
    .unwrap();
    assert_eq!(
        original["content"], receipt["content"],
        "archive the exact observed page"
    );
    assert!(serde_json::to_vec(&final_request).unwrap().len() < 100_000);
}
