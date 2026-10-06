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
    let requests = server.received_requests().await.unwrap();
    let final_request: serde_json::Value =
        serde_json::from_slice(&requests.last().unwrap().body).unwrap();
    assert!(final_request["messages"]
        .as_array()
        .unwrap()
        .iter()
        .any(|message| message["role"] == "user" && message["content"] == "Read the note"));
}

#[tokio::test]
async fn hard_cap_returns_natural_handoff_without_failing() {
    let server = MockServer::start().await;
    common::mount_script(
        &server,
        vec![
            common::tool_call_body(
                "call_1",
                "write_file",
                json!({"file_path":"progress.txt","content":"step 1\n","append":true}),
            ),
            common::final_text_body(
                "The user requested ordered steps; step 1 was written to progress.txt.",
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
            segment_turns: 1,
            total_turns: 1,
            context_token_budget: 2000,
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
    assert!(events
        .iter()
        .any(|event| matches!(event, RunEvent::ContextCheckpoint { .. })));
    assert!(events
        .iter()
        .any(|event| matches!(event, RunEvent::Incomplete { .. })));
    assert!(!events
        .iter()
        .any(|event| matches!(event, RunEvent::Failed { .. })));
}

#[tokio::test]
async fn empty_handoff_does_not_claim_the_task_was_completed() {
    let server = MockServer::start().await;
    common::mount_script(
        &server,
        vec![
            common::tool_call_body(
                "call_1",
                "write_file",
                json!({"file_path":"progress.txt","content":"step 1\n","append":true}),
            ),
            common::final_text_body("The first step was written."),
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
            segment_turns: 1,
            total_turns: 1,
            context_token_budget: 2000,
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

#[tokio::test]
async fn skill_catalog_reaches_system_request_without_body() {
    let server = MockServer::start().await;
    common::mount_script(&server, vec![common::final_text_body("Done")]).await;
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
    themis_core::skills::materialize_scripts(std::slice::from_ref(&skill), root.path()).unwrap();
    let catalog = themis_core::skills::catalog_prompt(&[skill], root.path());
    let (tx, mut rx) = tokio::sync::mpsc::channel(1024);
    themis_core::runtime::run_task_with_policy_and_catalog(
        llm,
        vec![],
        "Inspect changes".into(),
        vec![],
        Arc::new(AllowAllHook),
        RunPolicy {
            segment_turns: 2,
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
    assert!(system.contains(
        root.path()
            .join(".themis/skills/review/SKILL.md")
            .to_str()
            .unwrap()
    ));
    assert!(!system.contains("BODY_MUST_REMAIN_LAZY"));
}

async fn stopped_provider_request_finishes(compacting: bool) {
    use std::sync::atomic::Ordering;
    use std::time::Duration;
    use themis_core::runtime::{ConversationRole, ConversationTurn};
    use wiremock::{matchers::method, Mock, ResponseTemplate};
    let server = MockServer::start().await;
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
            text: "old context ".repeat(500),
        }]
    } else {
        vec![]
    };
    let run = tokio::spawn(run_task_with_policy(
        llm,
        vec![],
        "Continue".into(),
        history,
        Arc::new(AllowAllHook),
        RunPolicy {
            segment_turns: 5,
            total_turns: 5,
            context_token_budget: if compacting { 64 } else { usize::MAX },
            recent_messages: 4,
        },
        tx,
        stopped.clone(),
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
