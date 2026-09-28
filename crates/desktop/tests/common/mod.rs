use std::path::{Path, PathBuf};

use serde_json::{json, Value};
use themis_desktop::state::AppState;
use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, Request, ResponseTemplate};

pub(super) fn git_available() -> bool {
    std::process::Command::new("git")
        .arg("--version")
        .output()
        .is_ok_and(|out| out.status.success())
}

pub(super) fn git(repo: &Path, args: &[&str]) {
    let status = std::process::Command::new("git")
        .args(args)
        .current_dir(repo)
        .status()
        .expect("run git");
    assert!(status.success(), "git {args:?} failed");
}

pub(super) fn init_repo() -> (tempfile::TempDir, PathBuf) {
    let temp = tempfile::tempdir().expect("tempdir");
    let root = temp.path().join("repo");
    std::fs::create_dir(&root).expect("mkdir");
    git(&root, &["init", "-q"]);
    git(&root, &["config", "user.email", "themis@test"]);
    git(&root, &["config", "user.name", "themis"]);
    git(&root, &["config", "core.autocrlf", "false"]);
    std::fs::write(root.join("README.md"), "# repo\n").expect("write");
    git(&root, &["add", "."]);
    git(&root, &["commit", "-qm", "init"]);
    (temp, root)
}

pub(super) fn tool_call_body() -> Value {
    json!({
        "choices": [{
            "message": {
                "role": "assistant",
                "content": null,
                "tool_calls": [{
                    "id": "call_1",
                    "type": "function",
                    "function": {
                        "name": "write_file",
                        "arguments": "{\"file_path\": \"hello.txt\", \"content\": \"hi from agent\\n\", \"append\": false}"
                    }
                }]
            }
        }]
    })
}

pub(super) fn final_text_body() -> Value {
    json!({
        "choices": [{
            "message": {"role": "assistant", "content": "done writing the file"},
            "finish_reason": "stop"
        }]
    })
}

pub(super) async fn mount_scripted_llm() -> MockServer {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/chat/completions"))
        .respond_with(|request: &Request| {
            let body: Value = serde_json::from_slice(&request.body).unwrap_or(Value::Null);
            let has_tool_result = body["messages"]
                .as_array()
                .is_some_and(|messages| messages.iter().any(|message| message["role"] == "tool"));
            let payload = if has_tool_result {
                final_text_body()
            } else {
                tool_call_body()
            };
            ResponseTemplate::new(200).set_body_json(payload)
        })
        .mount(&server)
        .await;
    server
}

#[allow(dead_code)]
pub(super) async fn use_mock_llm(state: &AppState, server: &MockServer) {
    state
        .set_secret("go".to_owned(), "test-key".to_owned())
        .await
        .expect("store key");
    state.set_go_base_url_override(Some(server.uri()));
}
