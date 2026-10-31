//! Shared server + CLI run: provider-reported usage calibrates the context estimate
//! and a context-length overflow is recovered by one retry, over the same live AppState.

use std::path::Path;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::Duration;

use serde_json::{json, Value};
use themis_desktop::server::{Client, Server};
use themis_desktop::state::AppState;
use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, Request, ResponseTemplate};

#[allow(dead_code)]
mod common;

fn cli(data_dir: &Path, words: &[&str]) -> Value {
    let output = std::process::Command::new(env!("CARGO_BIN_EXE_themis"))
        .arg("--data-dir")
        .arg(data_dir)
        .env_remove("OPENCODE_KEY")
        .args(words)
        .output()
        .expect("run CLI");
    assert!(
        output.status.success(),
        "CLI {words:?} failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    serde_json::from_slice(&output.stdout).expect("CLI JSON output")
}

#[tokio::test(flavor = "multi_thread")]
async fn cli_run_reports_calibration_and_recovers_from_one_context_overflow() {
    if !common::git_available() {
        eprintln!("skipping: git binary not found");
        return;
    }
    let (_repo_dir, root_path) = common::init_repo();
    let data_dir = tempfile::tempdir().unwrap();
    let state = AppState::new_for_test(data_dir.path().join("settings.json"));
    let provider = MockServer::builder().start().await;
    let ordinary = Arc::new(AtomicUsize::new(0));
    let seen = ordinary.clone();
    Mock::given(method("POST"))
        .and(path("/chat/completions"))
        .respond_with(move |request: &Request| {
            let body: Value = serde_json::from_slice(&request.body).unwrap();
            if body["messages"][0]["content"]
                .as_str()
                .is_some_and(|text| text.contains("You summarize agent context"))
            {
                let mut summary = common::final_text_body();
                summary["choices"][0]["message"]["content"] = "Goal/constraints: write hello.txt. Confirmed findings: none yet. Completed changes/tests: none. Unresolved work: write hello.txt. Next action: write it.".into();
                return ResponseTemplate::new(200).set_body_json(summary);
            }
            if seen.fetch_add(1, Ordering::SeqCst) == 0 {
                return ResponseTemplate::new(400).set_body_json(json!({"error": {
                    "code": "context_length_exceeded",
                    "message": "This model's maximum context length is 8192 tokens."
                }}));
            }
            let has_tool_result = body["messages"]
                .as_array()
                .is_some_and(|messages| messages.iter().any(|message| message["role"] == "tool"));
            let mut payload = if has_tool_result {
                common::final_text_body()
            } else {
                common::tool_call_body()
            };
            let prompt = request.body.len() / 2;
            payload["usage"] = json!({"prompt_tokens": prompt, "completion_tokens": 5,
                "total_tokens": prompt + 5});
            ResponseTemplate::new(200).set_body_json(payload)
        })
        .mount(&provider)
        .await;
    common::use_mock_llm(&state, &provider).await;
    let task = tokio::spawn(Server::bind(state, data_dir.path()).await.unwrap().run());

    let root = root_path.to_str().unwrap();
    let thread = cli(data_dir.path(), &["thread", "create", root, "test-model"]);
    let id = thread["id"].as_str().unwrap();
    let mut events = Client::new(data_dir.path().to_path_buf())
        .subscribe()
        .await
        .unwrap();
    let handle = cli(
        data_dir.path(),
        &["thread", "send", id, "write hello.txt", "--detach"],
    );
    let run_id = handle["run_id"].as_str().unwrap();
    let mut budgets = Vec::new();
    let terminal = tokio::time::timeout(Duration::from_secs(60), async {
        loop {
            let event = Client::next_event(&mut events).await.expect("event");
            if event["name"] == "approval-request" {
                let approval_id = event["payload"]["approval_id"].as_str().unwrap();
                let request =
                    json!({"threadId": id, "approvalId": approval_id, "decision": "once"});
                cli(
                    data_dir.path(),
                    &["call", "approve_action", &request.to_string()],
                );
            }
            if event["name"] == "thread-event" && event["payload"]["run_id"] == run_id {
                let inner = &event["payload"]["event"];
                match inner["kind"].as_str().unwrap_or_default() {
                    "context_budget" => budgets.push(inner["budget"].clone()),
                    "finished" | "failed" => break inner.clone(),
                    _ => {}
                }
            }
        }
    })
    .await
    .expect("terminal event");
    assert_eq!(terminal["kind"], "finished", "{terminal}");
    assert_eq!(
        ordinary.load(Ordering::SeqCst),
        4,
        "overflow, one retry, final answer, spec check"
    );
    assert_eq!(
        std::fs::read_to_string(root_path.join("hello.txt")).unwrap(),
        "hi from agent\n",
        "the write ran exactly once"
    );
    assert!(budgets.len() >= 2, "{budgets:?}");
    assert!(budgets[0]["calibration_ratio_milli"].is_null());
    let last = budgets.last().unwrap();
    assert!(last["calibration_ratio_milli"].as_u64().unwrap() >= 500);
    assert!(last["provider_input_tokens"].as_u64().unwrap() > 0);
    assert!(last["raw_estimated_tokens"].as_u64().unwrap() > 0);
    task.abort();
}
