//! CLI calls the same live AppState as the desktop, including streamed approvals.

use std::path::Path;
use std::time::Duration;

use serde_json::{json, Value};
use themis_desktop::server::{Client, Server};
use themis_desktop::state::AppState;

mod common;

fn cli(data_dir: &Path, words: &[&str]) -> Value {
    let output = std::process::Command::new(env!("CARGO_BIN_EXE_themis"))
        .arg("--data-dir")
        .arg(data_dir)
        .args(words)
        .env_remove("OPENCODE_KEY")
        .output()
        .expect("run CLI");
    assert!(
        output.status.success(),
        "CLI {words:?} failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    serde_json::from_slice(&output.stdout).expect("CLI JSON output")
}

#[test]
fn headless_desktop_process_persists_cli_created_threads() {
    let data_dir = tempfile::tempdir().expect("data dir");
    let project = tempfile::tempdir().expect("project dir");
    let start = || {
        std::process::Command::new(env!("CARGO_BIN_EXE_themis-desktop"))
            .env_remove("OPENCODE_KEY")
            .arg("--themis-server")
            .arg(data_dir.path())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .spawn()
            .expect("start headless desktop server")
    };
    let ready = || {
        for _ in 0..50 {
            if std::process::Command::new(env!("CARGO_BIN_EXE_themis"))
                .arg("--data-dir")
                .arg(data_dir.path())
                .arg("status")
                .output()
                .is_ok_and(|output| output.status.success())
            {
                return;
            }
            std::thread::sleep(Duration::from_millis(100));
        }
        panic!("headless desktop server did not become ready");
    };

    let mut first = start();
    ready();
    let thread = cli(
        data_dir.path(),
        &["thread", "create", project.path().to_str().unwrap()],
    );
    let id = thread["id"].as_str().unwrap().to_owned();
    assert_eq!(cli(data_dir.path(), &["server", "stop"]), Value::Null);
    first.wait().expect("reap first server");
    assert!(!data_dir.path().join("server.json").exists());

    let mut second = start();
    ready();
    assert_eq!(cli(data_dir.path(), &["thread", "get", &id])["id"], id);
    assert_eq!(cli(data_dir.path(), &["server", "stop"]), Value::Null);
    second.wait().expect("reap second server");
}

#[tokio::test(flavor = "multi_thread")]
async fn server_cli_drives_thread_approval_and_persistence() {
    if !common::git_available() {
        eprintln!("skipping: git binary not found");
        return;
    }
    let (_repo_dir, root) = common::init_repo();
    let data_dir = tempfile::tempdir().expect("data dir");
    let state = AppState::new_for_test(data_dir.path().join("settings.json"));
    let provider = common::mount_scripted_llm().await;
    common::use_mock_llm(&state, &provider).await;
    let server = Server::bind(state.clone(), data_dir.path())
        .await
        .expect("bind server");
    let task = tokio::spawn(server.run());

    let project = root.to_str().expect("project path");
    assert_eq!(cli(data_dir.path(), &["status"]), json!("0.1.0"));
    let opened = cli(
        data_dir.path(),
        &[
            "call",
            "open_project",
            &json!({"path": project}).to_string(),
        ],
    );
    assert_eq!(
        opened["root"],
        root.canonicalize().unwrap().to_str().unwrap()
    );
    let thread = cli(
        data_dir.path(),
        &["thread", "create", project, "test-model"],
    );
    let id = thread["id"].as_str().expect("thread ID");
    assert_eq!(
        cli(data_dir.path(), &["thread", "list", project])
            .as_array()
            .unwrap()
            .len(),
        1
    );
    assert_eq!(cli(data_dir.path(), &["thread", "get", id])["id"], id);

    let mut events = Client::new(data_dir.path().to_path_buf())
        .subscribe()
        .await
        .expect("subscribe");
    let handle = cli(
        data_dir.path(),
        &["thread", "send", id, "write hello.txt", "--detach"],
    );
    let run_id = handle["run_id"].as_str().expect("run ID");
    let mut approved = false;
    let terminal = tokio::time::timeout(Duration::from_secs(60), async {
        loop {
            let event = Client::next_event(&mut events).await.expect("event");
            if event["name"] == "approval-request" {
                assert_eq!(event["payload"]["thread_id"], id);
                let approval_id = event["payload"]["approval_id"].as_str().unwrap();
                let request =
                    json!({"threadId": id, "approvalId": approval_id, "decision": "once"});
                assert_eq!(
                    cli(
                        data_dir.path(),
                        &["call", "approve_action", &request.to_string()]
                    ),
                    Value::Null
                );
                approved = true;
            }
            if event["name"] == "thread-event" && event["payload"]["run_id"] == run_id {
                match event["payload"]["event"]["kind"]
                    .as_str()
                    .unwrap_or_default()
                {
                    "finished" | "failed" => break event["payload"]["event"].clone(),
                    _ => {}
                }
            }
        }
    })
    .await
    .expect("terminal event");
    assert!(
        approved,
        "tool approval was not delivered through the server"
    );
    assert_eq!(terminal["kind"], "finished", "{terminal}");
    assert_eq!(
        std::fs::read_to_string(root.join("hello.txt")).unwrap(),
        "hi from agent\n"
    );
    assert!(!cli(data_dir.path(), &["thread", "history", id])
        .as_array()
        .unwrap()
        .is_empty());
    assert!(!cli(
        data_dir.path(),
        &["call", "list_diff", &json!({"threadId": id}).to_string()]
    )
    .is_null());

    task.abort();
}

#[test]
fn help_and_version_work_without_starting_a_server() {
    let dir = tempfile::tempdir().unwrap();
    for option in ["--help", "--version"] {
        let output = std::process::Command::new(env!("CARGO_BIN_EXE_themis"))
            .args(["--data-dir", dir.path().to_str().unwrap(), option])
            .env_remove("OPENCODE_KEY")
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "{option}: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        assert!(!dir.path().join("sessions.sqlite3").exists());
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn streamed_cli_followup_uses_persisted_context_and_skills() {
    use wiremock::matchers::{method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};
    let project = tempfile::tempdir().unwrap();
    let data = tempfile::tempdir().unwrap();
    let state = AppState::new_for_test(data.path().join("settings.json"));
    let provider = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/chat/completions"))
        .respond_with(ResponseTemplate::new(200).set_body_json(common::final_text_body()))
        .mount(&provider)
        .await;
    common::use_mock_llm(&state, &provider).await;
    let server = Server::bind(state, data.path()).await.unwrap();
    let task = tokio::spawn(server.run());
    let thread = cli(
        data.path(),
        &[
            "thread",
            "create",
            project.path().to_str().unwrap(),
            "test-model",
        ],
    );
    let id = thread["id"].as_str().unwrap();
    let skill = cli(data.path(), &["call", "create_skill", &json!({"input": {
        "name": "Writer", "description": "test", "instructions": "Always be concise.",
        "allowed_tools": ["read_file"], "scripts": [{"name": "greet.sh", "content": "echo hi"}]
    }}).to_string()]);
    cli(
        data.path(),
        &[
            "call",
            "set_thread_skills",
            &json!({"threadId": id, "skillIds": [skill["id"]]}).to_string(),
        ],
    );
    for prompt in ["Remember my first request", "What was my first request?"] {
        let output = std::process::Command::new(env!("CARGO_BIN_EXE_themis"))
            .arg("--data-dir")
            .arg(data.path())
            .args(["thread", "send", id, prompt, "--json"])
            .env_remove("OPENCODE_KEY")
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        let events: Vec<Value> = String::from_utf8(output.stdout)
            .unwrap()
            .lines()
            .map(|line| serde_json::from_str(line).unwrap())
            .collect();
        assert!(events
            .iter()
            .any(|event| event["payload"]["event"]["kind"] == "finished"));
    }
    let requests = provider.received_requests().await.unwrap();
    let body: Value = serde_json::from_slice(&requests.last().unwrap().body).unwrap();
    let messages = body["messages"].to_string();
    assert!(messages.contains("Remember my first request"));
    assert!(messages.contains("Always be concise."));
    assert!(project
        .path()
        .join(".themis/skills")
        .join(skill["id"].as_str().unwrap())
        .join("greet.sh")
        .exists());
    assert!(
        cli(data.path(), &["thread", "history", id])
            .as_array()
            .unwrap()
            .len()
            >= 4
    );
    task.abort();
}
