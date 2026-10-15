//! Enabling an MCP connection is the consent to start it, but only when a
//! person enabled that exact definition. Definitions that arrive any other way
//! (a repository's files, an unapproved change) never start by themselves, and
//! an agent's enable/edit of a command-based server asks about the exact
//! command every time.
#![cfg(unix)]

use std::sync::Arc;
use std::time::Duration;

use serde_json::{json, Value};
use themis_core::configuration::ApprovalMode;
use themis_core::plugins::{PluginSpec, PluginStore};
use themis_desktop::sink::{ChannelSink, TestEvent};
use themis_desktop::state::AppState;
use themis_desktop::types::{ApprovalDecision, ApprovalRequest, ProviderKind, ThreadEvent};
use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, Request, ResponseTemplate};

#[allow(dead_code)]
mod common;

async fn mcp_server() -> MockServer {
    let mcp = MockServer::start().await;
    Mock::given(method("POST"))
        .respond_with(|request: &Request| {
            let body: Value = serde_json::from_slice(&request.body).unwrap();
            let result = match body["method"].as_str().unwrap() {
                "initialize" => json!({"protocolVersion":"2025-03-26","capabilities":{},"serverInfo":{"name":"t","version":"1"}}),
                "tools/list" => json!({"tools":[{"name":"lookup","description":"x","inputSchema":{"type":"object","properties":{}}}]}),
                _ => json!({}),
            };
            ResponseTemplate::new(200)
                .set_body_json(json!({"jsonrpc":"2.0","id":body["id"],"result":result}))
        })
        .mount(&mcp)
        .await;
    mcp
}

fn url_spec(url: &str, enabled: bool) -> PluginSpec {
    serde_json::from_value(json!({"name":"docs","mcp":{"docs":{"url":url,"enabled":enabled}}}))
        .unwrap()
}

async fn requests_to(mcp: &MockServer) -> usize {
    mcp.received_requests()
        .await
        .map_or(0, |requests| requests.len())
}

/// Waits for the background start to settle (blocked starts fail at once).
async fn settled(state: &AppState, root: &std::path::Path) -> Value {
    for _ in 0..100 {
        let shown = state
            .plugin_action(json!({"action":"mcp_status","projectRoot":root,"connect":true}))
            .await
            .unwrap();
        if shown["local:docs:mcp:docs"]["status"] != "connecting" {
            return shown;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    panic!("start did not settle");
}

fn status(state_value: &Value) -> String {
    state_value["local:docs:mcp:docs"]["reason"]
        .as_str()
        .unwrap_or_default()
        .to_owned()
}

async fn run_to_end(
    state: &AppState,
    chat: &str,
    text: &str,
    answers: &[ApprovalDecision],
) -> (Vec<ApprovalRequest>, Vec<String>) {
    let (sink, mut rx) = ChannelSink::channel();
    state
        .send_message(Arc::new(sink), chat.to_owned(), text.to_owned())
        .await
        .unwrap();
    let mut asked = Vec::new();
    let mut notes = Vec::new();
    tokio::time::timeout(Duration::from_secs(60), async {
        while let Some(event) = rx.recv().await {
            match event {
                TestEvent::Approval(request) => {
                    let answer = answers
                        .get(asked.len())
                        .copied()
                        .unwrap_or(ApprovalDecision::Deny);
                    state
                        .approve_action(
                            request.thread_id.clone(),
                            request.approval_id.clone(),
                            answer,
                        )
                        .await
                        .unwrap();
                    asked.push(request);
                }
                TestEvent::Thread(envelope) => match envelope.event {
                    ThreadEvent::ToolFinished { tool, output, .. }
                        if tool.starts_with("mcp_start_") =>
                    {
                        notes.push(output)
                    }
                    ThreadEvent::Finished { .. } | ThreadEvent::Failed { .. } => break,
                    _ => {}
                },
                _ => {}
            }
        }
    })
    .await
    .expect("run finished");
    (asked, notes)
}

async fn new_chat(state: &AppState, project: &std::path::Path) -> String {
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

/// Provider that, on the first round, asks the agent tool to save `spec`.
async fn agent_saving(spec: Value) -> MockServer {
    let llm = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/chat/completions"))
        .respond_with(move |request: &Request| {
            let body: Value = serde_json::from_slice(&request.body).unwrap();
            let messages = body["messages"].as_array().unwrap();
            let last_user = messages.iter().rposition(|m| m["role"] == "user").unwrap();
            let done = messages[last_user..].iter().any(|m| m["role"] == "tool");
            let message = if done {
                json!({"role":"assistant","content":"finished"})
            } else {
                json!({"role":"assistant","content":null,"tool_calls":[{"id":"c","type":"function","function":{
                    "name":"manage_integrations",
                    "arguments":json!({"action":"save","scope":"local","spec":spec}).to_string()}}]})
            };
            ResponseTemplate::new(200).set_body_json(json!({"choices":[{"message":message}]}))
        })
        .mount(&llm)
        .await;
    llm
}

#[tokio::test(flavor = "multi_thread")]
async fn definitions_that_a_person_did_not_enable_never_start() {
    let data = tempfile::tempdir().unwrap();
    let project = tempfile::tempdir().unwrap();
    let root = project.path().canonicalize().unwrap();
    let state = AppState::new_for_test(data.path().join("settings.json"));
    let mcp = mcp_server().await;
    // A cloned repository (or any non-user writer) leaves an enabled definition behind.
    PluginStore::new(data.path().to_owned(), Some(root.clone()))
        .save("local", url_spec(&mcp.uri(), true), None)
        .unwrap();
    let shown = settled(&state, &root).await;
    assert!(status(&shown).contains("Needs your approval"), "{shown}");
    let llm = common::mount_scripted_llm().await;
    common::use_mock_llm(&state, &llm).await;
    let chat = new_chat(&state, &root).await;
    let (_, notes) = run_to_end(&state, &chat, "hello", &[]).await;
    assert!(
        notes.iter().any(|n| n.contains("not been enabled by you")),
        "{notes:?}"
    );
    assert_eq!(
        requests_to(&mcp).await,
        0,
        "an unapproved definition must not even be contacted"
    );

    // The person enabling it (UI/CLI) is the consent; it starts now.
    state
        .plugin_action(json!({"action":"set_component_enabled","scope":"local","projectRoot":root,"name":"docs","kind":"mcp","id":"docs","enabled":true}))
        .await
        .unwrap();
    let shown = settled(&state, &root).await;
    assert!(!status(&shown).contains("Needs your approval"), "{shown}");

    // A change made by anything but a person withdraws the consent.
    let other = mcp_server().await;
    let store = PluginStore::new(data.path().to_owned(), Some(root.clone()));
    let revision = store
        .list()
        .unwrap()
        .into_iter()
        .find(|p| p.spec.name == "docs")
        .unwrap()
        .revision;
    store
        .save("local", url_spec(&other.uri(), true), Some(&revision))
        .unwrap();
    let shown = settled(&state, &root).await;
    assert!(status(&shown).contains("Needs your approval"), "{shown}");
    assert_eq!(requests_to(&other).await, 0);
}

#[tokio::test(flavor = "multi_thread")]
async fn agent_enabling_a_command_server_asks_once_about_the_exact_command_every_time() {
    let data = tempfile::tempdir().unwrap();
    let project = tempfile::tempdir().unwrap();
    let root = project.path().canonicalize().unwrap();
    let state = AppState::new_for_test(data.path().join("settings.json"));
    let spec = json!({"name":"docs","mcp":{"docs":{"command":"sh","args":["-c","exit 0"],"enabled":true}}});
    let llm = agent_saving(spec).await;
    common::use_mock_llm(&state, &llm).await;
    let chat = new_chat(&state, &root).await;
    let store = PluginStore::new(data.path().to_owned(), Some(root.clone()));

    // The call's only executable effect is that command: one dialog, exact argv.
    // Declining saves nothing.
    let (asked, _) = run_to_end(&state, &chat, "enable it", &[ApprovalDecision::Deny]).await;
    assert_eq!(asked.len(), 1, "{asked:?}");
    assert_eq!(asked[0].tool, "manage_integrations_mcp_command");
    assert_eq!(
        asked[0].command.as_ref().unwrap().argv,
        ["sh", "-c", "exit 0"]
    );
    assert!(!asked[0].grantable, "never offered for the chat");
    assert!(
        store.list().unwrap().iter().all(|p| p.spec.name != "docs"),
        "denied: nothing saved"
    );

    // "Allow in this chat" is downgraded to once and not stored; the answered
    // dialog is the consent.
    let (asked, _) = run_to_end(&state, &chat, "enable it again", &[ApprovalDecision::Chat]).await;
    assert_eq!(asked.len(), 1);
    let shown = settled(&state, &root).await;
    assert!(!status(&shown).contains("Needs your approval"), "{shown}");
    let stored =
        std::fs::read_to_string(data.path().join("chat_grants").join(format!("{chat}.json")))
            .unwrap_or_default();
    assert!(!stored.contains("mcp_command"), "{stored}");

    // The same definition again still asks (the dialog is never remembered).
    state
        .plugin_action(json!({"action":"delete","scope":"local","projectRoot":root,"name":"docs"}))
        .await
        .unwrap();
    let (asked, _) = run_to_end(&state, &chat, "again", &[ApprovalDecision::Once]).await;
    assert_eq!(asked.len(), 1);

    // A changed command asks about the new argv.
    let changed = agent_saving(json!({"name":"docs","mcp":{"docs":{"command":"sh","args":["-c","exit 1"],"enabled":true}}})).await;
    common::use_mock_llm(&state, &changed).await;
    let (asked, _) = run_to_end(&state, &chat, "once more", &[ApprovalDecision::Deny]).await;
    assert_eq!(
        asked[0].command.as_ref().unwrap().argv,
        ["sh", "-c", "exit 1"]
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn a_call_with_other_executable_effects_keeps_both_dialogs_and_a_grant_cannot_cover_the_command(
) {
    let data = tempfile::tempdir().unwrap();
    let project = tempfile::tempdir().unwrap();
    let root = project.path().canonicalize().unwrap();
    let state = AppState::new_for_test(data.path().join("settings.json"));
    let spec = json!({"name":"docs","hooks":[{"name":"h","event":"RunStart","command":"true","enabled":false}],
        "mcp":{"docs":{"command":"sh","args":["-c","exit 0"],"enabled":true}}});
    let llm = agent_saving(spec).await;
    common::use_mock_llm(&state, &llm).await;
    let chat = new_chat(&state, &root).await;
    // The generic dialog is answered "Allow in this chat"; the command dialog still asks next time.
    for round in 0..2 {
        let (asked, _) = run_to_end(
            &state,
            &chat,
            "enable it",
            &[ApprovalDecision::Chat, ApprovalDecision::Deny],
        )
        .await;
        let tools: Vec<&str> = asked.iter().map(|a| a.tool.as_str()).collect();
        assert_eq!(
            tools.last(),
            Some(&"manage_integrations_mcp_command"),
            "round {round}: {tools:?}"
        );
        assert!(tools.len() == 2 || round == 1, "{tools:?}");
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn yolo_agent_changes_apply_but_leave_the_connection_unconsented() {
    let data = tempfile::tempdir().unwrap();
    let project = tempfile::tempdir().unwrap();
    let root = project.path().canonicalize().unwrap();
    let state = AppState::new_for_test(data.path().join("settings.json"));
    let spec = json!({"name":"docs","mcp":{"docs":{"command":"sh","args":["-c","exit 0"],"enabled":true}}});
    let llm = agent_saving(spec).await;
    common::use_mock_llm(&state, &llm).await;
    let chat = new_chat(&state, &root).await;
    state
        .set_thread_approval_mode(chat.clone(), ApprovalMode::Yolo)
        .await
        .unwrap();
    let (asked, _) = run_to_end(&state, &chat, "enable it", &[]).await;
    assert!(asked.is_empty(), "YOLO is unchanged: {asked:?}");
    assert!(
        PluginStore::new(data.path().to_owned(), Some(root.clone()))
            .list()
            .unwrap()
            .iter()
            .any(|p| p.spec.name == "docs"),
        "the change applied"
    );
    // YOLO was trust for that run only: a later Custom run does not start it.
    let shown = settled(&state, &root).await;
    assert!(status(&shown).contains("Needs your approval"), "{shown}");
    state
        .set_thread_approval_mode(chat.clone(), ApprovalMode::Custom)
        .await
        .unwrap();
    let (_, notes) = run_to_end(&state, &chat, "hello", &[]).await;
    assert!(
        notes.iter().any(|n| n.contains("not been enabled by you")),
        "{notes:?}"
    );
}
