//! Read-only `git` tool calls through the shared app server and the unattended `themis` CLI,
//! which refuses anything that would ask: `git status` runs, while `git diff --output=...`
//! (a flag that writes a file) still needs approval and so is refused.
#![cfg(unix)]

use serde_json::{json, Value};
use std::path::Path;
use themis_desktop::server::Server;
use themis_desktop::state::AppState;
use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, Request, ResponseTemplate};

#[allow(dead_code)]
mod common;

/// Mock provider that makes the scripted `git` calls one per round, then answers.
async fn mount_git_script(calls: Vec<Vec<&'static str>>) -> MockServer {
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
                None => json!({"choices":[{"message":{"role":"assistant","content":"finished"},
                    "finish_reason":"stop"}]}),
            };
            ResponseTemplate::new(200).set_body_json(payload)
        })
        .mount(&server)
        .await;
    server
}

fn cli(data_dir: &Path, args: &[&str]) -> std::process::Output {
    std::process::Command::new(env!("CARGO_BIN_EXE_themis"))
        .arg("--data-dir")
        .arg(data_dir)
        .args(args)
        .stdin(std::process::Stdio::null())
        .env_remove("OPENCODE_KEY")
        .output()
        .unwrap()
}

#[tokio::test(flavor = "multi_thread")]
async fn unattended_git_status_runs_but_a_diff_that_writes_a_file_is_refused() {
    if !common::git_available() {
        return;
    }
    let (_repo, project) = common::init_repo();
    std::fs::write(project.join("untracked-marker.txt"), "x").unwrap();
    let dir = tempfile::tempdir().unwrap();
    let state = AppState::new_for_test(dir.path().join("settings.json"));
    let provider = mount_git_script(vec![
        vec!["status", "--short"],
        vec!["diff", "--output=out.patch"],
    ])
    .await;
    common::use_mock_llm(&state, &provider).await;
    let task = tokio::spawn(Server::bind(state, dir.path()).await.unwrap().run());
    let root = project.to_str().unwrap();
    let created = cli(
        dir.path(),
        &[
            "call",
            "create_thread",
            &json!({"projectRoot": root, "provider": "go", "model": "test-model"}).to_string(),
        ],
    );
    assert!(
        created.status.success(),
        "{}",
        String::from_utf8_lossy(&created.stderr)
    );
    let thread: Value = serde_json::from_slice(&created.stdout).unwrap();
    let id = thread["id"].as_str().unwrap();

    let sent = cli(
        dir.path(),
        &["thread", "send", id, "look at the repository"],
    );
    let stderr = String::from_utf8_lossy(&sent.stderr);
    assert!(sent.status.success(), "{stderr}");

    let history = cli(dir.path(), &["thread", "history", id]);
    let history: Value = serde_json::from_slice(&history.stdout).unwrap();
    let outputs: Vec<String> = history
        .as_array()
        .unwrap()
        .iter()
        .filter(|entry| entry["envelope"]["event"]["kind"] == "tool_finished")
        .map(|entry| entry["envelope"]["event"]["output"].to_string())
        .collect();
    assert!(
        outputs[0].contains("untracked-marker.txt"),
        "git status should have run unattended: {outputs:?}"
    );
    assert!(
        !project.join("out.patch").exists(),
        "a diff that writes a file must not run without approval"
    );
    assert!(stderr.contains("Tool approval denied"), "{stderr}");
    task.abort();
}
