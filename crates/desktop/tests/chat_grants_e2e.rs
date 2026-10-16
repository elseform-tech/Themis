//! "Allow in this chat" grants through the shared app server: they persist in
//! the chat's own file, cover later runs and the chat's automations, never
//! cover destructive variants, lose to deny rules, and extend to a whole MCP
//! server. Scripted mock provider; no network beyond localhost.
#![cfg(unix)]

use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

use serde_json::{json, Value};
use themis_desktop::server::{Client, Server};
use themis_desktop::sink::{ChannelSink, TestEvent};
use themis_desktop::state::AppState;
use themis_desktop::types::{
    ApprovalDecision, ApprovalRequest, AutomationInput, ProviderKind, ThreadEvent,
};
use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, Request, ResponseTemplate};

#[allow(dead_code)]
mod common;

type Script = Vec<(&'static str, Vec<Vec<&'static str>>)>;

/// Mock provider: the last user message picks a script by substring and its
/// shell calls run one per round; the answer follows once all produced results.
async fn mount_script(script: Script) -> MockServer {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/chat/completions"))
        .respond_with(move |request: &Request| {
            let body: Value = serde_json::from_slice(&request.body).unwrap_or(Value::Null);
            let messages = body["messages"].as_array().cloned().unwrap_or_default();
            let last_user = messages.iter().rposition(|m| m["role"] == "user");
            let task = last_user
                .and_then(|index| messages[index]["content"].as_str())
                .unwrap_or_default()
                .to_owned();
            let done = last_user
                .map(|index| messages[index..].iter().filter(|m| m["role"] == "tool").count())
                .unwrap_or(0);
            let calls = script
                .iter()
                .find(|(key, _)| task.contains(key))
                .map(|(_, calls)| calls.clone())
                .unwrap_or_default();
            let payload = match calls.get(done) {
                Some(argv) => json!({"choices":[{"message":{"role":"assistant","content":null,
                    "tool_calls":[{"id":format!("call_{done}"),"type":"function","function":{
                        "name":"shell",
                        "arguments":json!({"command":argv[0],"args":&argv[1..]}).to_string()}}]}}]}),
                None => json!({"choices":[{"message":{"role":"assistant","content":"finished"},
                    "finish_reason":"stop"}]}),
            };
            ResponseTemplate::new(200).set_body_json(payload)
        })
        .mount(&server)
        .await;
    server
}

struct Observed {
    approvals: Vec<ApprovalRequest>,
    tools_failed: usize,
}

/// Runs one message to its terminal event, answering every dialog with `answer`.
async fn run_message(
    state: &AppState,
    thread_id: &str,
    text: &str,
    answer: ApprovalDecision,
) -> Observed {
    let (sink, mut rx) = ChannelSink::channel();
    state
        .send_message(Arc::new(sink), thread_id.to_owned(), text.to_owned())
        .await
        .unwrap();
    drain(state, &mut rx, answer).await
}

async fn drain(
    state: &AppState,
    rx: &mut tokio::sync::mpsc::UnboundedReceiver<TestEvent>,
    answer: ApprovalDecision,
) -> Observed {
    let mut observed = Observed {
        approvals: Vec::new(),
        tools_failed: 0,
    };
    tokio::time::timeout(Duration::from_secs(60), async {
        while let Some(event) = rx.recv().await {
            match event {
                TestEvent::Approval(request) => {
                    state
                        .approve_action(
                            request.thread_id.clone(),
                            request.approval_id.clone(),
                            answer,
                        )
                        .await
                        .unwrap();
                    observed.approvals.push(request);
                }
                TestEvent::Thread(envelope) => match envelope.event {
                    ThreadEvent::ToolFinished { ok: false, .. } => observed.tools_failed += 1,
                    ThreadEvent::Finished { .. } | ThreadEvent::Failed { .. } => break,
                    _ => {}
                },
                _ => {}
            }
        }
    })
    .await
    .expect("run finished");
    observed
}

async fn new_chat(state: &AppState, project: &Path) -> String {
    state
        .create_thread(
            project.to_string_lossy().into_owned(),
            ProviderKind::Go,
            Some("test-model".to_owned()),
        )
        .await
        .unwrap()
        .id
}

fn grants_file(data: &Path, thread_id: &str) -> std::path::PathBuf {
    data.join("chat_grants").join(format!("{thread_id}.json"))
}

#[tokio::test(flavor = "multi_thread")]
async fn chat_grant_persists_across_runs_of_one_chat_and_not_other_chats() {
    if !common::git_available() {
        return;
    }
    let (_repo, project) = common::init_repo();
    let data = tempfile::tempdir().unwrap();
    let state = AppState::new_for_test(data.path().join("settings.json"));
    let provider = mount_script(vec![
        ("make-a", vec![vec!["touch", "a.txt"]]),
        ("force", vec![vec!["git", "push", "--force", "origin", "x"]]),
    ])
    .await;
    common::use_mock_llm(&state, &provider).await;
    let chat = new_chat(&state, &project).await;
    let other = new_chat(&state, &project).await;

    let first = run_message(&state, &chat, "make-a first", ApprovalDecision::Chat).await;
    assert_eq!(first.approvals.len(), 1);
    assert!(first.approvals[0].grantable);
    assert!(project.join("a.txt").exists());
    let stored: Value =
        serde_json::from_slice(&std::fs::read(grants_file(data.path(), &chat)).unwrap()).unwrap();
    assert_eq!(stored["version"], 1);
    assert_eq!(stored["grants"][0]["command"], json!(["touch", "a.txt"]));

    // A later run of the same chat is covered; the command really ran again.
    std::fs::remove_file(project.join("a.txt")).unwrap();
    let second = run_message(&state, &chat, "make-a second", ApprovalDecision::Deny).await;
    assert!(second.approvals.is_empty(), "{:?}", second.approvals);
    assert!(project.join("a.txt").exists());

    // Another chat has no grant and asks.
    let third = run_message(&state, &other, "make-a third", ApprovalDecision::Deny).await;
    assert_eq!(third.approvals.len(), 1);
    assert!(!grants_file(data.path(), &other).exists());

    // Destructive variants are not offered for the chat and ask every time,
    // even when the chat answers "Allow in this chat".
    for round in 0..2 {
        let forced = run_message(&state, &chat, "force push", ApprovalDecision::Chat).await;
        assert_eq!(forced.approvals.len(), 1, "round {round}");
        assert!(!forced.approvals[0].grantable);
    }
    let stored = std::fs::read_to_string(grants_file(data.path(), &chat)).unwrap();
    assert!(!stored.contains("--force"), "{stored}");

    // Discarding the chat removes its grants.
    state.discard_thread(chat.clone()).await.unwrap();
    assert!(!grants_file(data.path(), &chat).exists());
}

#[tokio::test(flavor = "multi_thread")]
async fn deny_rule_beats_a_saved_chat_grant() {
    if !common::git_available() {
        return;
    }
    let (_repo, project) = common::init_repo();
    let data = tempfile::tempdir().unwrap();
    let state = AppState::new_for_test(data.path().join("settings.json"));
    let provider = mount_script(vec![("make-a", vec![vec!["touch", "a.txt"]])]).await;
    common::use_mock_llm(&state, &provider).await;
    let chat = new_chat(&state, &project).await;
    run_message(&state, &chat, "make-a first", ApprovalDecision::Chat).await;
    std::fs::remove_file(project.join("a.txt")).unwrap();
    state
        .save_runtime_configuration(
            "user",
            Some(project.to_string_lossy().into_owned()),
            r#"{"approval":{"rules":[{"tool":"shell","command":["touch"],"action":"deny"}]}}"#,
        )
        .unwrap();
    let denied = run_message(&state, &chat, "make-a again", ApprovalDecision::Chat).await;
    assert!(denied.approvals.is_empty());
    assert_eq!(denied.tools_failed, 1);
    assert!(!project.join("a.txt").exists(), "the deny rule must win");
}

#[tokio::test(flavor = "multi_thread")]
async fn chat_automation_inherits_grants_and_denies_destructive_variants_unattended() {
    if !common::git_available() {
        return;
    }
    let (_repo, project) = common::init_repo();
    let data = tempfile::tempdir().unwrap();
    let state = AppState::new_for_test(data.path().join("settings.json"));
    let provider = mount_script(vec![
        ("make-a", vec![vec!["touch", "a.txt"]]),
        ("force", vec![vec!["git", "push", "--force", "origin", "x"]]),
    ])
    .await;
    common::use_mock_llm(&state, &provider).await;
    let chat = new_chat(&state, &project).await;
    run_message(&state, &chat, "make-a first", ApprovalDecision::Chat).await;
    std::fs::remove_file(project.join("a.txt")).unwrap();

    let automation = |task: &str| AutomationInput {
        name: "Watch".to_owned(),
        project_root: project.to_string_lossy().into_owned(),
        provider: ProviderKind::Go,
        model: "test-model".to_owned(),
        reasoning_effort: None,
        target_thread_id: Some(chat.clone()),
        skill_ids: Vec::new(),
        interval_mins: 60,
        schedule: None,
        task: task.to_owned(),
        enabled: true,
        watch: Default::default(),
    };
    for (task, expect_tool_failure) in [("make-a auto", false), ("force auto", true)] {
        let created = state.create_automation(automation(task)).await.unwrap();
        let (sink, mut rx) = ChannelSink::channel();
        state
            .run_automation_now(Arc::new(sink), created.id)
            .await
            .unwrap();
        let observed = drain(&state, &mut rx, ApprovalDecision::Deny).await;
        assert!(
            observed.approvals.is_empty(),
            "{task}: nobody can answer an automation, so nothing is asked: {:?}",
            observed.approvals
        );
        assert_eq!(observed.tools_failed == 1, expect_tool_failure, "{task}");
    }
    assert!(
        project.join("a.txt").exists(),
        "the automation inherited the chat's grant"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn mcp_server_starts_without_a_prompt_and_one_grant_covers_its_other_tools() {
    let data = tempfile::tempdir().unwrap();
    let project = tempfile::tempdir().unwrap();
    let state = AppState::new_for_test(data.path().join("settings.json"));
    let mcp = MockServer::start().await;
    Mock::given(method("POST"))
        .respond_with(|request: &Request| {
            let body: Value = serde_json::from_slice(&request.body).unwrap();
            let result = match body["method"].as_str().unwrap() {
                "initialize" => json!({"protocolVersion":"2025-03-26","capabilities":{},"serverInfo":{"name":"t","version":"1"}}),
                "tools/list" => json!({"tools":[
                    {"name":"lookup","description":"Look up","inputSchema":{"type":"object","properties":{}}},
                    {"name":"other","description":"Other","inputSchema":{"type":"object","properties":{}}}]}),
                "tools/call" => json!({"content":[{"type":"text","text":"ok"}]}),
                _ => json!({}),
            };
            ResponseTemplate::new(200)
                .set_body_json(json!({"jsonrpc":"2.0","id":body["id"],"result":result}))
        })
        .mount(&mcp)
        .await;
    let llm = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/chat/completions"))
        .respond_with(|request: &Request| {
            let body: Value = serde_json::from_slice(&request.body).unwrap();
            let messages = body["messages"].as_array().unwrap();
            let last_user = messages.iter().rposition(|m| m["role"] == "user").unwrap();
            let done = messages[last_user..].iter().filter(|m| m["role"] == "tool").count();
            let tool = |suffix: &str| {
                body["tools"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .filter_map(|t| t["function"]["name"].as_str())
                    .find(|name| name.starts_with("mcp_") && name.ends_with(suffix))
                    .map(str::to_owned)
            };
            let wanted = ["_lookup", "_other"].get(done).and_then(|suffix| tool(suffix));
            let message = match wanted {
                Some(name) => json!({"role":"assistant","content":null,"tool_calls":[{"id":format!("c{done}"),"type":"function","function":{"name":name,"arguments":"{}"}}]}),
                None => json!({"role":"assistant","content":"finished"}),
            };
            ResponseTemplate::new(200).set_body_json(json!({"choices":[{"message":message}]}))
        })
        .mount(&llm)
        .await;
    common::use_mock_llm(&state, &llm).await;
    state
        .plugin_action(
            json!({"action":"save","scope":"local","projectRoot":project.path(),
            "spec":{"name":"docs","mcp":{"docs":{"url":mcp.uri(),"enabled":true}}}}),
        )
        .await
        .unwrap();
    let chat = new_chat(&state, project.path()).await;
    let first = run_message(&state, &chat, "use both tools", ApprovalDecision::Chat).await;
    let tools: Vec<&str> = first.approvals.iter().map(|a| a.tool.as_str()).collect();
    assert_eq!(tools.len(), 1, "{tools:?}");
    assert!(
        tools[0].starts_with("mcp_") && !tools[0].starts_with("mcp_start_"),
        "enabling the connection is the consent; only the tool call asks: {tools:?}"
    );
    assert!(tools[0].ends_with("_lookup"), "{tools:?}");
    // The grant for `lookup` covered `other` in the same run, and covers both
    // tools in the chat's next run.
    let calls = |requests: Vec<Request>| {
        requests
            .iter()
            .filter(|r| {
                serde_json::from_slice::<Value>(&r.body).is_ok_and(|b| b["method"] == "tools/call")
            })
            .count()
    };
    assert_eq!(calls(mcp.received_requests().await.unwrap()), 2);
    let second = run_message(
        &state,
        &chat,
        "use both tools again",
        ApprovalDecision::Deny,
    )
    .await;
    assert!(second.approvals.is_empty(), "{:?}", second.approvals);
    assert_eq!(calls(mcp.received_requests().await.unwrap()), 4);
    // Another chat is not covered.
    let other = new_chat(&state, project.path()).await;
    let third = run_message(&state, &other, "use both tools", ApprovalDecision::Deny).await;
    assert_eq!(
        third.approvals.len(),
        2,
        "each denied tool asks in a chat without the grant"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn an_explicit_deny_rule_still_blocks_mcp_startup() {
    let data = tempfile::tempdir().unwrap();
    let project = tempfile::tempdir().unwrap();
    let state = AppState::new_for_test(data.path().join("settings.json"));
    let mcp = MockServer::start().await;
    Mock::given(method("POST"))
        .respond_with(ResponseTemplate::new(500))
        .mount(&mcp)
        .await;
    let llm = mount_script(vec![]).await;
    common::use_mock_llm(&state, &llm).await;
    state
        .save_runtime_configuration(
            "user",
            Some(project.path().to_string_lossy().into_owned()),
            r#"{"approval":{"rules":[{"tool":"mcp_start_*","action":"deny"}]}}"#,
        )
        .unwrap();
    state
        .plugin_action(
            json!({"action":"save","scope":"local","projectRoot":project.path(),
            "spec":{"name":"docs","mcp":{"docs":{"url":mcp.uri(),"enabled":true}}}}),
        )
        .await
        .unwrap();
    let chat = new_chat(&state, project.path()).await;
    let (sink, mut rx) = ChannelSink::channel();
    state
        .send_message(Arc::new(sink), chat.clone(), "hello".to_owned())
        .await
        .unwrap();
    let mut startup_note = None;
    let observed = tokio::time::timeout(Duration::from_secs(60), async {
        let mut approvals = 0;
        while let Some(event) = rx.recv().await {
            match event {
                TestEvent::Approval(_) => approvals += 1,
                TestEvent::Thread(envelope) => match envelope.event {
                    ThreadEvent::ToolFinished { tool, output, .. }
                        if tool.starts_with("mcp_start_") =>
                    {
                        startup_note = Some(output)
                    }
                    ThreadEvent::Finished { .. } | ThreadEvent::Failed { .. } => break,
                    _ => {}
                },
                _ => {}
            }
        }
        approvals
    })
    .await
    .expect("run finished");
    assert_eq!(observed, 0, "a deny rule is silent");
    assert!(
        startup_note
            .unwrap()
            .contains("startup approval was denied"),
        "the model is told the connection is unavailable"
    );
    assert!(mcp.received_requests().await.unwrap().is_empty());
}

// ---- Shared-server CLI end to end -------------------------------------------------

fn cli(dir: &Path, args: &[&str]) -> Value {
    let out = std::process::Command::new(env!("CARGO_BIN_EXE_themis"))
        .arg("--data-dir")
        .arg(dir)
        .args(args)
        .env_remove("OPENCODE_KEY")
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    serde_json::from_slice(&out.stdout).unwrap()
}

/// Sends `text` through the CLI and answers dialogs through the server with
/// `answer`; returns how many dialogs were raised.
async fn cli_run(
    data: &Path,
    events: &mut tokio::net::TcpStream,
    id: &str,
    text: &str,
    answer: &str,
) -> usize {
    let handle = cli(data, &["thread", "send", id, text, "--detach"]);
    let run_id = handle["run_id"].as_str().unwrap().to_owned();
    let mut dialogs = 0;
    tokio::time::timeout(Duration::from_secs(60), async {
        loop {
            let event = Client::next_event(events).await.unwrap();
            if event["name"] == "approval-request" {
                dialogs += 1;
                let request = json!({"threadId": id, "approvalId": event["payload"]["approval_id"], "decision": answer});
                cli(data, &["call", "approve_action", &request.to_string()]);
            }
            if event["name"] == "thread-event"
                && event["payload"]["run_id"] == run_id
                && matches!(event["payload"]["event"]["kind"].as_str(), Some("finished" | "failed"))
            {
                break;
            }
        }
    })
    .await
    .expect("run finished");
    dialogs
}

#[tokio::test(flavor = "multi_thread")]
async fn cli_chat_grant_from_run_one_covers_run_two_of_the_same_chat() {
    if !common::git_available() {
        return;
    }
    let (_repo, project) = common::init_repo();
    let data = tempfile::tempdir().unwrap();
    let state = AppState::new_for_test(data.path().join("settings.json"));
    let provider = mount_script(vec![("make-a", vec![vec!["touch", "a.txt"]])]).await;
    common::use_mock_llm(&state, &provider).await;
    let server = Server::bind(state, data.path()).await.unwrap();
    let task = tokio::spawn(server.run());
    let root = project.to_str().unwrap();
    let id = cli(data.path(), &["thread", "create", root, "test-model"])["id"]
        .as_str()
        .unwrap()
        .to_owned();
    let mut events = Client::new(data.path().to_path_buf())
        .subscribe()
        .await
        .unwrap();

    assert_eq!(
        cli_run(data.path(), &mut events, &id, "make-a one", "chat").await,
        1
    );
    let stored = std::fs::read_to_string(grants_file(data.path(), &id)).unwrap();
    assert!(
        stored.contains("\"version\": 1") && stored.contains("a.txt"),
        "{stored}"
    );
    std::fs::remove_file(project.join("a.txt")).unwrap();
    assert_eq!(
        cli_run(data.path(), &mut events, &id, "make-a two", "deny").await,
        0
    );
    assert!(project.join("a.txt").exists(), "run two reused the grant");

    // Undo: deleting the chat's file makes the next run ask again.
    std::fs::remove_file(grants_file(data.path(), &id)).unwrap();
    assert_eq!(
        cli_run(data.path(), &mut events, &id, "make-a three", "deny").await,
        1
    );
    task.abort();
}

#[tokio::test(flavor = "multi_thread")]
async fn wrapped_destructive_commands_ask_every_time_and_meet_deny_rules() {
    if !common::git_available() {
        return;
    }
    let (_repo, project) = common::init_repo();
    let data = tempfile::tempdir().unwrap();
    let state = AppState::new_for_test(data.path().join("settings.json"));
    let provider = mount_script(vec![
        (
            "wrapped",
            vec![vec!["sh", "-c", "git status && git reset --hard"]],
        ),
        ("sudo-env", vec![vec!["env", "X=1", "rm", "-rf", "build"]]),
    ])
    .await;
    common::use_mock_llm(&state, &provider).await;
    let chat = new_chat(&state, &project).await;

    // A destructive command inside `sh -c` or behind `env` is never grantable
    // and asks every time, even after "Allow in this chat".
    for text in ["wrapped reset", "wrapped reset again", "sudo-env clean"] {
        let observed = run_message(&state, &chat, text, ApprovalDecision::Chat).await;
        assert_eq!(observed.approvals.len(), 1, "{text}");
        assert!(!observed.approvals[0].grantable, "{text}");
    }
    let stored = std::fs::read_to_string(grants_file(data.path(), &chat)).unwrap_or_default();
    assert!(
        !stored.contains("reset") && !stored.contains("rm"),
        "{stored}"
    );

    // A deny rule on the inner command refuses the wrapped call without asking.
    state
        .save_runtime_configuration(
            "user",
            Some(project.to_string_lossy().into_owned()),
            r#"{"approval":{"rules":[{"tool":"shell","command":["git","reset"],"action":"deny"}]}}"#,
        )
        .unwrap();
    let denied = run_message(
        &state,
        &chat,
        "wrapped reset denied",
        ApprovalDecision::Chat,
    )
    .await;
    assert!(denied.approvals.is_empty(), "{:?}", denied.approvals);
    assert_eq!(denied.tools_failed, 1);
}
