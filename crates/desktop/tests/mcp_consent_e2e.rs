//! Enabling an MCP connection is the consent to start it, but only when a
//! person enabled that exact definition. Definitions that arrive any other way
//! (another program writing the registry, an unapproved change) never start by themselves. That an agent never
//! switches a command-based server on, in any mode, is in `agent_cli_risk_e2e`.
#![cfg(unix)]

use std::sync::Arc;
use std::time::Duration;

use serde_json::{json, Value};
use themis_core::plugins::{PluginSpec, PluginStore};
use themis_desktop::sink::{ChannelSink, TestEvent};
use themis_desktop::state::AppState;
use themis_desktop::types::{ApprovalDecision, ApprovalRequest, ProviderKind, ThreadEvent};
use wiremock::matchers::method;
use wiremock::{Mock, MockServer, Request, ResponseTemplate};

#[allow(dead_code)]
mod common;

async fn mcp_server() -> MockServer {
    let mcp = MockServer::builder().start().await;
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

/// Waits for the background start to settle (blocked starts fail at once). The app gives a start
/// up to 30 seconds before it fails it, so the wait outlasts that: a start that is merely slow
/// (a busy machine copying a large plugin or spawning a process) settles here, and one that
/// never does still fails the test.
async fn settled(state: &AppState, root: &std::path::Path) -> Value {
    let deadline = tokio::time::Instant::now() + Duration::from_secs(45);
    loop {
        let shown = state
            .plugin_action(json!({"action":"mcp_status","projectRoot":root,"connect":true}))
            .await
            .unwrap();
        if shown["global:docs:mcp:docs"]["status"] != "connecting" {
            return shown;
        }
        assert!(
            tokio::time::Instant::now() < deadline,
            "start did not settle within 45 s: {shown}"
        );
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
}

fn status(state_value: &Value) -> String {
    state_value["global:docs:mcp:docs"]["reason"]
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

/// A plugin with as many text files as the registry takes can be enabled like any other: seven
/// catalog plugins (cloudflare and sentry among them) ship 360 to 800.
#[tokio::test(flavor = "multi_thread")]
async fn enabling_a_server_of_a_plugin_with_many_files_starts_it() {
    let data = tempfile::tempdir().unwrap();
    let project = tempfile::tempdir().unwrap();
    let root = project.path().canonicalize().unwrap();
    let state = AppState::new_for_test(data.path().join("settings.json"));
    let mcp = mcp_server().await;
    let mut spec = url_spec(&mcp.uri(), false);
    for index in 0..1000 {
        spec.files
            .insert(format!("skills/x/{index}.md"), "x".into());
    }
    PluginStore::new(data.path().to_owned(), Some(root.clone()))
        .save("global", spec, None)
        .unwrap();
    state
        .plugin_action(json!({"action":"set_component_enabled","scope":"global","projectRoot":root,"name":"docs","kind":"mcp","id":"docs","enabled":true}))
        .await
        .unwrap();
    let shown = settled(&state, &root).await;
    assert!(!status(&shown).contains("Needs your approval"), "{shown}");
    assert!(requests_to(&mcp).await > 0, "it never started: {shown}");
}

#[tokio::test(flavor = "multi_thread")]
async fn definitions_that_a_person_did_not_enable_never_start() {
    let data = tempfile::tempdir().unwrap();
    let project = tempfile::tempdir().unwrap();
    let root = project.path().canonicalize().unwrap();
    let state = AppState::new_for_test(data.path().join("settings.json"));
    let mcp = mcp_server().await;
    // The app already keeps its consent file (as a running app does), so the first-read seeding
    // of servers already on (the upgrade path) does not cover what comes next.
    std::fs::write(
        data.path().join("mcp_consent.json"),
        r#"{"version":1,"approved":{}}"#,
    )
    .unwrap();
    // Another program (any non-user writer) leaves an enabled definition in the registry.
    PluginStore::new(data.path().to_owned(), Some(root.clone()))
        .save("global", url_spec(&mcp.uri(), true), None)
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
        .plugin_action(json!({"action":"set_component_enabled","scope":"global","projectRoot":root,"name":"docs","kind":"mcp","id":"docs","enabled":true}))
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
        .save("global", url_spec(&other.uri(), true), Some(&revision))
        .unwrap();
    let shown = settled(&state, &root).await;
    assert!(status(&shown).contains("Needs your approval"), "{shown}");
    assert_eq!(requests_to(&other).await, 0);
}
