//! Runtime integration tests: scripted mock-LLM multi-turn runs.
//!
//! No real network: the LLM is a wiremock script. The only live test is
//! `live_go_chat_completions_run`, marked `#[ignore]` behind
//! `THEMIS_GO_API_KEY` + `THEMIS_GO_MODEL`.

mod common;

use std::sync::Arc;

use serde_json::json;
use themis_core::providers::{resolve, ProviderConfig, ProviderKind};
use themis_core::runtime::{run_task, ApprovalDecision, RunEvent};
use themis_core::tools::{boxed_tools, AllowAllHook, ApprovalHook, DenyAllHook};
use wiremock::MockServer;

fn has_finished(events: &[RunEvent]) -> bool {
    events
        .iter()
        .any(|event| matches!(event, RunEvent::Finished { .. }))
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
