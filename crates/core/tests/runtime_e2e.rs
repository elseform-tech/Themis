//! Runtime integration tests: scripted mock-LLM multi-turn runs.
//!
//! No real network: the LLM is a wiremock script. The only live test is
//! `live_go_chat_completions_run`, marked `#[ignore]` behind
//! `THEMIS_GO_API_KEY` + `THEMIS_GO_MODEL`.

mod common;

use std::sync::{atomic::AtomicBool, Arc};

use serde_json::json;
use themis_core::providers::{resolve, ProviderConfig, ProviderKind};
use themis_core::runtime::{run_task, run_task_with_policy, ApprovalDecision, RunEvent, RunPolicy};
use themis_core::tools::{boxed_tools, AllowAllHook, ApprovalHook, DenyAllHook};
use wiremock::MockServer;

fn has_finished(events: &[RunEvent]) -> bool {
    events
        .iter()
        .any(|event| matches!(event, RunEvent::Finished { .. }))
}

#[tokio::test]
async fn checkpoint_continues_same_request_after_segment_limit() {
    let server = MockServer::start().await;
    common::mount_script(
        &server,
        vec![
            common::tool_call_body("call_1", "read_file", json!({"file_path":"note.txt"})),
            common::final_text_body("Read note.txt; answer the original request next."),
            common::final_text_body("Done after checkpoint"),
        ],
    )
    .await;
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("note.txt"), "hello").unwrap();
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
        "Read the note".into(),
        vec![],
        approvals,
        RunPolicy {
            segment_turns: 1,
            total_turns: 3,
            context_token_budget: 16000,
            recent_messages: 4,
        },
        tx,
        Arc::new(AtomicBool::new(false)),
    )
    .await
    .unwrap();
    assert_eq!(answer, "Done after checkpoint");
    let events = common::drain(&mut rx).await;
    assert!(events.iter().any(|event| matches!(event, RunEvent::ContextCheckpoint { summary } if summary.contains("Read note.txt"))), "{events:?}");
    assert!(has_finished(&events));
}

#[tokio::test]
async fn mock_multi_turn_edit_writes_file_and_finishes() {
    let server = MockServer::start().await;
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
    let server = MockServer::start().await;
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
