//! Command-scoped shell approvals through the shared app server, the `themis`
//! CLI (unattended) and the interactive bridge (allow-for-this-run grants).
//! Shell calls are portable (`git`-free `echo`/`touch` are avoided on Windows
//! by gating on unix).
#![cfg(unix)]

use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

use serde_json::{json, Value};
use themis_desktop::server::Server;
use themis_desktop::sink::{ChannelSink, TestEvent};
use themis_desktop::state::AppState;
use themis_desktop::types::{ApprovalDecision, ProviderKind, ThreadEvent};
use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, Request, ResponseTemplate};

#[allow(dead_code)]
mod common;

/// Mock provider that issues the scripted `shell` calls one per round, then
/// a final text answer once every call has produced a tool message.
async fn mount_shell_script(calls: Vec<Vec<&'static str>>) -> MockServer {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/chat/completions"))
        .respond_with(move |request: &Request| {
            let body: Value = serde_json::from_slice(&request.body).unwrap_or(Value::Null);
            let done = body["messages"]
                .as_array()
                .map(|m| m.iter().filter(|m| m["role"] == "tool").count())
                .unwrap_or(0);
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

fn cli_call(data_dir: &Path, method: &str, args: Value) -> Value {
    use std::io::Write;
    let mut child = std::process::Command::new(env!("CARGO_BIN_EXE_themis"))
        .arg("--data-dir")
        .arg(data_dir)
        .env_remove("OPENCODE_KEY")
        .args(["call", method, "-"])
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .expect("run CLI");
    child
        .stdin
        .take()
        .unwrap()
        .write_all(args.to_string().as_bytes())
        .unwrap();
    let output = child.wait_with_output().unwrap();
    assert!(
        output.status.success(),
        "CLI {method} failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    serde_json::from_slice(&output.stdout).expect("CLI JSON output")
}

#[tokio::test(flavor = "multi_thread")]
async fn unattended_cli_runs_allowed_command_prefix_and_denies_other_shell_commands() {
    if !common::git_available() {
        return;
    }
    let (_repo, project) = common::init_repo();
    let dir = tempfile::tempdir().unwrap();
    let state = AppState::new_for_test(dir.path().join("settings.json"));
    let provider = mount_shell_script(vec![
        vec!["touch", "allowed.txt"],
        vec!["touch", "denied.txt"],
    ])
    .await;
    common::use_mock_llm(&state, &provider).await;
    let server = Server::bind(state, dir.path()).await.unwrap();
    let task = tokio::spawn(server.run());
    let root = project.to_str().unwrap();
    let rules = r#"{"approval":{"rules":[{"tool":"shell","command":["touch","allowed.txt"],"action":"allow"}]}}"#;
    cli_call(
        dir.path(),
        "save_runtime_configuration",
        json!({"scope":"user","projectRoot":root,"json":rules}),
    );
    let thread = cli_call(
        dir.path(),
        "create_thread",
        json!({"projectRoot":root,"provider":"go","model":"test-model"}),
    );
    let id = thread["id"].as_str().unwrap();
    let sent = std::process::Command::new(env!("CARGO_BIN_EXE_themis"))
        .arg("--data-dir")
        .arg(dir.path())
        .args(["thread", "send", id, "touch two files"])
        .stdin(std::process::Stdio::null())
        .env_remove("OPENCODE_KEY")
        .output()
        .unwrap();
    assert!(
        sent.status.success(),
        "{}",
        String::from_utf8_lossy(&sent.stderr)
    );
    assert!(
        project.join("allowed.txt").exists(),
        "allowed prefix must run"
    );
    assert!(
        !project.join("denied.txt").exists(),
        "non-matching shell command must not run unattended"
    );
    assert!(String::from_utf8_lossy(&sent.stderr).contains("Tool approval denied"));
    task.abort();
}

#[tokio::test(flavor = "multi_thread")]
async fn allow_for_this_run_grant_is_scoped_to_the_approved_command() {
    if !common::git_available() {
        return;
    }
    let (_repo, project) = common::init_repo();
    let dir = tempfile::tempdir().unwrap();
    let state = AppState::new_for_test(dir.path().join("settings.json"));
    let provider = mount_shell_script(vec![
        vec!["echo", "one"],
        vec!["echo", "one"],
        vec!["touch", "other.txt"],
    ])
    .await;
    common::use_mock_llm(&state, &provider).await;
    let root = project.to_string_lossy().into_owned();
    let thread = state
        .create_thread(root, ProviderKind::Go, Some("test-model".to_owned()))
        .await
        .unwrap();
    let (sink, mut rx) = ChannelSink::channel();
    state
        .send_message(Arc::new(sink), thread.id.clone(), "run commands".into())
        .await
        .unwrap();
    let mut prompts = Vec::new();
    let outcome = tokio::time::timeout(Duration::from_secs(60), async {
        while let Some(event) = rx.recv().await {
            match event {
                TestEvent::Approval(request) => {
                    prompts.push(request.summary.clone());
                    state
                        .approve_action(
                            thread.id.clone(),
                            request.approval_id.clone(),
                            ApprovalDecision::Always,
                        )
                        .await
                        .unwrap();
                }
                TestEvent::Thread(envelope)
                    if matches!(
                        envelope.event,
                        ThreadEvent::Finished { .. } | ThreadEvent::Failed { .. }
                    ) =>
                {
                    break
                }
                _ => {}
            }
        }
    })
    .await;
    assert!(outcome.is_ok(), "run did not finish");
    // `echo one` is asked once and then covered by its grant; `touch` is a
    // different program, so it prompts again.
    assert_eq!(prompts.len(), 2, "{prompts:?}");
    assert!(prompts[0].contains("echo"), "{prompts:?}");
    assert!(prompts[1].contains("touch"), "{prompts:?}");
    assert!(project.join("other.txt").exists());
}
