//! The built-in `git` tool talking to a remote, through a real run (scripted provider) in
//! Custom: a push is asked about at the network level and "Allow in this chat" covers the next
//! ordinary push, while a force push is asked about every time and never covered. The remote is
//! a local bare repository; nothing leaves the machine.
#![cfg(unix)]

use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

use serde_json::{json, Value};
use themis_desktop::sink::{ChannelSink, TestEvent};
use themis_desktop::state::AppState;
use themis_desktop::types::{ApprovalDecision, ProviderKind, ThreadEvent};
use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, Request, ResponseTemplate};

#[allow(dead_code)]
mod common;

/// A provider that makes the scripted `git` calls, one per round, then answers.
async fn mount_git(calls: Vec<Vec<&'static str>>) -> MockServer {
    let server = MockServer::builder().start().await;
    Mock::given(method("POST"))
        .and(path("/chat/completions"))
        .respond_with(move |request: &Request| {
            let body: Value = serde_json::from_slice(&request.body).unwrap_or(Value::Null);
            let done = body["messages"]
                .as_array()
                .map(|m| m.iter().filter(|m| m["role"] == "tool").count())
                .unwrap_or(0);
            let payload = match calls.get(done) {
                Some(args) => json!({"choices":[{"message":{"role":"assistant","content":null,
                    "tool_calls":[{"id":format!("call_{done}"),"type":"function","function":{
                        "name":"git","arguments":json!({"args": args}).to_string()}}]}}]}),
                None => json!({"choices":[{"message":{"role":"assistant","content":"done"},
                    "finish_reason":"stop"}]}),
            };
            ResponseTemplate::new(200).set_body_json(payload)
        })
        .mount(&server)
        .await;
    server
}

fn git(dir: &Path, args: &[&str]) -> String {
    let output = std::process::Command::new("git")
        .current_dir(dir)
        .args(args)
        .output()
        .expect("git");
    String::from_utf8_lossy(&output.stdout).trim().to_owned()
}

/// Sends a message, answers each approval with `answer`, and returns the approvals asked (as
/// JSON) and whether each git call succeeded.
async fn run(state: &AppState, thread: &str, answer: ApprovalDecision) -> (Vec<Value>, Vec<bool>) {
    let (sink, mut rx) = ChannelSink::channel();
    state
        .send_message(Arc::new(sink), thread.to_owned(), "push it".into())
        .await
        .unwrap();
    let (mut asked, mut results) = (Vec::new(), Vec::new());
    let finished = tokio::time::timeout(Duration::from_secs(120), async {
        while let Some(event) = rx.recv().await {
            match event {
                TestEvent::Approval(request) => {
                    asked.push(serde_json::to_value(&request).unwrap());
                    state
                        .approve_action(thread.to_owned(), request.approval_id.clone(), answer)
                        .await
                        .unwrap();
                }
                TestEvent::Thread(envelope) => match envelope.event {
                    ThreadEvent::ToolFinished { tool, ok, .. } if tool == "git" => results.push(ok),
                    ThreadEvent::Finished { .. } => break,
                    ThreadEvent::Failed { error, .. } => panic!("run failed: {error}"),
                    _ => {}
                },
                _ => {}
            }
        }
    })
    .await;
    assert!(finished.is_ok(), "run did not finish");
    (asked, results)
}

#[tokio::test(flavor = "multi_thread")]
async fn the_git_tool_pushes_with_approval_and_a_force_push_asks_every_time() {
    if !common::git_available() {
        return;
    }
    let (_repo, project) = common::init_repo();
    let remote = tempfile::tempdir().unwrap();
    git(remote.path(), &["init", "-q", "--bare"]);
    git(
        &project,
        &["remote", "add", "origin", &remote.path().to_string_lossy()],
    );
    let branch = git(&project, &["rev-parse", "--abbrev-ref", "HEAD"]);
    let data = tempfile::tempdir().unwrap();
    let state = AppState::new_for_test(data.path().join("settings.json"));
    let provider = mount_git(vec![vec!["push", "origin", "HEAD"]]).await;
    common::use_mock_llm(&state, &provider).await;
    let thread = state
        .create_thread(
            project.to_string_lossy().into_owned(),
            ProviderKind::Go,
            Some("test-model".to_owned()),
        )
        .await
        .unwrap()
        .id;

    // A push asks at the network level; "Allow in this chat" lets it through.
    let (asked, results) = run(&state, &thread, ApprovalDecision::Chat).await;
    assert_eq!(asked.len(), 1, "{asked:#?}");
    assert_eq!(asked[0]["risk"], "network", "{}", asked[0]);
    assert_eq!(asked[0]["grantable"], true);
    assert_eq!(results, [true]);
    let pushed = git(remote.path(), &["rev-parse", &branch]);
    assert_eq!(
        pushed,
        git(&project, &["rev-parse", "HEAD"]),
        "the remote has the commit"
    );

    // The grant covers the next ordinary push in the chat: nothing is asked.
    std::fs::write(project.join("more.txt"), "more\n").unwrap();
    git(&project, &["add", "."]);
    git(&project, &["commit", "-qm", "more"]);
    let (asked, results) = run(&state, &thread, ApprovalDecision::Deny).await;
    assert!(asked.is_empty(), "{asked:#?}");
    assert_eq!(results, [true]);

    // A force push is asked about every time, despite the grant, and is never grantable.
    let provider = mount_git(vec![vec!["push", "--force", "origin", "HEAD"]]).await;
    common::use_mock_llm(&state, &provider).await;
    let (asked, results) = run(&state, &thread, ApprovalDecision::Deny).await;
    assert_eq!(asked.len(), 1, "{asked:#?}");
    assert_eq!(asked[0]["risk"], "destructive");
    assert_eq!(asked[0]["grantable"], false);
    assert_eq!(results, [false]);
}
