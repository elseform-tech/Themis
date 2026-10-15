//! Command-scoped shell approvals through the shared app server, the `themis`
//! CLI (unattended) and the interactive bridge (allow-in-this-chat grants).
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
async fn chat_grant_is_scoped_to_the_approved_command() {
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
    let mut scopes = Vec::new();
    let mut commands = Vec::new();
    let outcome = tokio::time::timeout(Duration::from_secs(60), async {
        while let Some(event) = rx.recv().await {
            match event {
                TestEvent::Approval(request) => {
                    prompts.push(request.summary.clone());
                    scopes.push(request.grant_scope.clone());
                    commands.push(request.command.clone());
                    state
                        .approve_action(
                            thread.id.clone(),
                            request.approval_id.clone(),
                            ApprovalDecision::Chat,
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
    // The approval event tells the UI what the grant covers: the exact argv
    // for non-subcommand programs, as structured tokens.
    let argvs: Vec<Vec<String>> = scopes
        .iter()
        .map(|s| s.as_ref().unwrap().argv.clone())
        .collect();
    assert_eq!(argvs, [vec!["echo", "one"], vec!["touch", "other.txt"]]);
    assert!(scopes
        .iter()
        .all(|s| !s.as_ref().unwrap().covers_further_args));
}

#[tokio::test(flavor = "multi_thread")]
async fn approval_event_carries_the_exact_argv_not_a_joined_summary() {
    if !common::git_available() {
        return;
    }
    let (_repo, project) = common::init_repo();
    let dir = tempfile::tempdir().unwrap();
    let state = AppState::new_for_test(dir.path().join("settings.json"));
    // One argument contains a space and one a bidi override: both must survive intact.
    let provider = mount_shell_script(vec![vec!["echo", "-rf /", "x\u{202e}y"]]).await;
    common::use_mock_llm(&state, &provider).await;
    let thread = state
        .create_thread(
            project.to_string_lossy().into_owned(),
            ProviderKind::Go,
            Some("test-model".to_owned()),
        )
        .await
        .unwrap();
    let (sink, mut rx) = ChannelSink::channel();
    state
        .send_message(Arc::new(sink), thread.id.clone(), "run it".into())
        .await
        .unwrap();
    let mut requests = Vec::new();
    tokio::time::timeout(Duration::from_secs(60), async {
        while let Some(event) = rx.recv().await {
            match event {
                TestEvent::Approval(request) => {
                    requests.push(request.clone());
                    state
                        .approve_action(
                            thread.id.clone(),
                            request.approval_id.clone(),
                            ApprovalDecision::Once,
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
    .await
    .expect("run finished");
    assert_eq!(requests.len(), 1, "{requests:?}");
    let command = requests[0].command.as_ref().expect("structured command");
    assert_eq!(command.argv, ["echo", "-rf /", "x\u{202e}y"]);
    assert!(!command.background);
}

#[tokio::test(flavor = "multi_thread")]
async fn allowing_git_push_does_not_allow_a_forced_push_unattended() {
    if !common::git_available() {
        return;
    }
    let (_repo, project) = common::init_repo();
    let remote = tempfile::tempdir().unwrap();
    common::git(remote.path(), &["init", "-q", "--bare"]);
    common::git(
        &project,
        &["remote", "add", "origin", remote.path().to_str().unwrap()],
    );
    let dir = tempfile::tempdir().unwrap();
    let state = AppState::new_for_test(dir.path().join("settings.json"));
    let provider = mount_shell_script(vec![
        vec!["git", "push", "origin", "HEAD:refs/heads/plain"],
        vec!["git", "push", "--force", "origin", "HEAD:refs/heads/forced"],
        vec!["git", "push", "origin", "+HEAD:refs/heads/plus"],
    ])
    .await;
    common::use_mock_llm(&state, &provider).await;
    let server = Server::bind(state, dir.path()).await.unwrap();
    let task = tokio::spawn(server.run());
    let root = project.to_str().unwrap();
    let rules =
        r#"{"approval":{"rules":[{"tool":"shell","command":["git","push"],"action":"allow"}]}}"#;
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
        .args(["thread", "send", id, "push three branches"])
        .stdin(std::process::Stdio::null())
        .env_remove("OPENCODE_KEY")
        .output()
        .unwrap();
    assert!(
        sent.status.success(),
        "{}",
        String::from_utf8_lossy(&sent.stderr)
    );
    let has_branch = |name: &str| {
        std::process::Command::new("git")
            .args(["rev-parse", "--verify", "-q", &format!("refs/heads/{name}")])
            .current_dir(remote.path())
            .status()
            .unwrap()
            .success()
    };
    assert!(has_branch("plain"), "the allowed plain push must run");
    assert!(
        !has_branch("forced"),
        "--force must not ride on the git push allow rule"
    );
    assert!(
        !has_branch("plus"),
        "a +refspec force push must not ride on the allow rule"
    );
    assert!(String::from_utf8_lossy(&sent.stderr).contains("Tool approval denied"));
    task.abort();
}

#[tokio::test(flavor = "multi_thread")]
async fn allowing_git_checkout_does_not_allow_discarding_local_edits_unattended() {
    if !common::git_available() {
        return;
    }
    let (_repo, project) = common::init_repo();
    std::fs::write(project.join("README.md"), "precious local edit\n").unwrap();
    let dir = tempfile::tempdir().unwrap();
    let state = AppState::new_for_test(dir.path().join("settings.json"));
    let provider = mount_shell_script(vec![
        vec!["git", "checkout", "-b", "feature"],
        vec!["git", "checkout", "--", "README.md"],
        vec!["git", "checkout", "."],
    ])
    .await;
    common::use_mock_llm(&state, &provider).await;
    let server = Server::bind(state, dir.path()).await.unwrap();
    let task = tokio::spawn(server.run());
    let root = project.to_str().unwrap();
    let rules = r#"{"approval":{"rules":[{"tool":"shell","command":["git","checkout"],"action":"allow"}]}}"#;
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
        .args(["thread", "send", id, "switch branch and revert README"])
        .stdin(std::process::Stdio::null())
        .env_remove("OPENCODE_KEY")
        .output()
        .unwrap();
    assert!(
        sent.status.success(),
        "{}",
        String::from_utf8_lossy(&sent.stderr)
    );
    let branch = std::process::Command::new("git")
        .args(["rev-parse", "--abbrev-ref", "HEAD"])
        .current_dir(&project)
        .output()
        .unwrap();
    assert_eq!(String::from_utf8_lossy(&branch.stdout).trim(), "feature");
    assert_eq!(
        std::fs::read_to_string(project.join("README.md")).unwrap(),
        "precious local edit\n",
        "checkout -- / . must not discard edits on a plain checkout allow rule"
    );
    assert!(String::from_utf8_lossy(&sent.stderr).contains("Tool approval denied"));
    task.abort();
}
