//! Goal mode and the prompt queue through the real shared server and the
//! `themis` CLI, plus a model switch mid-run (no foreign reasoning replayed).

use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

use serde_json::{json, Value};
use themis_desktop::sink::ChannelSink;
use themis_desktop::{server::Server, state::AppState, types::ProviderKind};
use wiremock::matchers::method;
use wiremock::{Mock, MockServer, Request, ResponseTemplate};

const CHECK_MARKER: &str = "You decide whether an agent has reached a goal";

fn call(data: &Path, command: &str, args: Value) -> Value {
    let output = std::process::Command::new(env!("CARGO_BIN_EXE_themis"))
        .arg("--data-dir")
        .arg(data)
        .args(["call", command, &args.to_string()])
        .env_remove("OPENCODE_KEY")
        .output()
        .expect("CLI");
    assert!(
        output.status.success(),
        "{command}: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    serde_json::from_slice(&output.stdout).unwrap_or(Value::Null)
}

fn reply(text: &str) -> ResponseTemplate {
    ResponseTemplate::new(200).set_body_json(json!({
        "choices": [{"message": {"role": "assistant", "content": text}, "finish_reason": "stop"}]
    }))
}

fn is_check(request: &Request) -> bool {
    request
        .body_json::<Value>()
        .ok()
        .and_then(|body| {
            body["messages"][0]["content"]
                .as_str()
                .map(|text| text.contains(CHECK_MARKER))
        })
        .unwrap_or(false)
}

#[tokio::test(flavor = "multi_thread")]
async fn cli_starts_a_goal_queues_a_message_and_reads_the_goal_back() {
    let llm = MockServer::start().await;
    Mock::given(method("POST"))
        .respond_with(|request: &Request| {
            if is_check(request) {
                reply(r#"{"verdict":"reached","reason":"The README has a title.","progress":"title present"}"#)
            } else {
                reply("Added a title.").set_delay(Duration::from_millis(800))
            }
        })
        .mount(&llm)
        .await;
    let data = tempfile::tempdir().unwrap();
    let project = tempfile::tempdir().unwrap();
    let state = AppState::new_for_test(data.path().join("settings.json"));
    state
        .set_secret("go".into(), "synthetic-test-key".into())
        .await
        .unwrap();
    state.set_go_base_url_override(Some(llm.uri()));
    let thread = state
        .create_thread(
            project.path().to_string_lossy().into(),
            ProviderKind::Go,
            Some("test-model".into()),
        )
        .await
        .unwrap();
    let server = Server::bind(state.clone(), data.path()).await.unwrap();
    let task = tokio::spawn(server.run());
    let data_dir = data.path().to_owned();
    let thread_id = thread.id.clone();
    tokio::task::spawn_blocking(move || {
        call(
            &data_dir,
            "send_message",
            json!({"threadId": thread_id, "text": "/goal give the README a title"}),
        );
        let goal = call(&data_dir, "get_goal", json!({"threadId": thread_id}));
        assert_eq!(goal["text"], "give the README a title");
        assert_eq!(goal["status"], "running");
        // While the goal's turn runs, a message waits in the queue.
        let queued = call(
            &data_dir,
            "enqueue_message",
            json!({"threadId": thread_id, "text": "also add a license line"}),
        );
        assert_eq!(queued["text"], "also add a license line");
        let listed = call(&data_dir, "list_queue", json!({"threadId": thread_id}));
        assert_eq!(listed.as_array().unwrap().len(), 1, "{listed}");
        let deadline = std::time::Instant::now() + Duration::from_secs(60);
        loop {
            let goal = call(&data_dir, "get_goal", json!({"threadId": thread_id}));
            let queue = call(&data_dir, "list_queue", json!({"threadId": thread_id}));
            if goal["status"] == "reached" && queue.as_array().is_some_and(Vec::is_empty) {
                assert_eq!(goal["reason"], "The README has a title.");
                break;
            }
            assert!(
                std::time::Instant::now() < deadline,
                "goal never reached: {goal}"
            );
            std::thread::sleep(Duration::from_millis(200));
        }
        // Pausing or resuming a reached goal is refused.
        let refused = std::process::Command::new(env!("CARGO_BIN_EXE_themis"))
            .arg("--data-dir")
            .arg(&data_dir)
            .args([
                "call",
                "resume_goal",
                &json!({"threadId": thread_id}).to_string(),
            ])
            .output()
            .unwrap();
        assert!(!refused.status.success());
    })
    .await
    .unwrap();
    // The queued message ran after the goal turn, before the goal continued.
    let requests = llm.received_requests().await.unwrap();
    let turns: Vec<Value> = requests
        .iter()
        .filter(|request| !is_check(request))
        .map(|request| request.body_json().unwrap())
        .collect();
    assert!(turns
        .iter()
        .any(|body| body.to_string().contains("also add a license line")));
    task.abort();
}

#[tokio::test(flavor = "multi_thread")]
async fn a_model_switch_mid_run_applies_next_turn_without_foreign_reasoning() {
    let llm = MockServer::start().await;
    Mock::given(method("POST"))
        .respond_with(|request: &Request| {
            let body: Value = request.body_json().unwrap();
            let has_tool_result = body["messages"]
                .as_array()
                .is_some_and(|messages| messages.iter().any(|m| m["role"] == "tool"));
            if body["model"] == "first-model" && !has_tool_result {
                ResponseTemplate::new(200)
                    .set_body_json(json!({"choices":[{"message":{"role":"assistant","content":null,"reasoning_content":"FIRST-MODEL-PRIVATE-REASONING","tool_calls":[{"id":"l","type":"function","function":{"name":"list_dir","arguments":"{\"path\":\".\"}"}}]},"finish_reason":"tool_calls"}]}))
                    .set_delay(Duration::from_millis(600))
            } else {
                reply("Listed.")
            }
        })
        .mount(&llm)
        .await;
    let data = tempfile::tempdir().unwrap();
    let project = tempfile::tempdir().unwrap();
    let state = AppState::new_for_test(data.path().join("settings.json"));
    state
        .set_secret("go".into(), "synthetic-test-key".into())
        .await
        .unwrap();
    state.set_go_base_url_override(Some(llm.uri()));
    let thread = state
        .create_thread(
            project.path().to_string_lossy().into(),
            ProviderKind::Go,
            Some("first-model".into()),
        )
        .await
        .unwrap();
    state
        .set_thread_approval_mode(
            thread.id.clone(),
            themis_core::configuration::ApprovalMode::Yolo,
        )
        .await
        .unwrap();
    let (sink, _rx) = ChannelSink::channel();
    let sink = Arc::new(sink);
    state
        .send_message(sink.clone(), thread.id.clone(), "list the folder".into())
        .await
        .unwrap();
    // Switch while the first run is still going.
    let switched = state
        .set_provider(
            thread.id.clone(),
            ProviderKind::Go,
            Some("second-model".into()),
        )
        .await
        .unwrap();
    assert_eq!(switched.model, "second-model");
    let deadline = std::time::Instant::now() + Duration::from_secs(30);
    while state.get_thread(&thread.id).await.unwrap().running {
        assert!(std::time::Instant::now() < deadline);
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    state
        .send_message(sink, thread.id.clone(), "and again".into())
        .await
        .unwrap();
    let bodies = || async {
        llm.received_requests()
            .await
            .unwrap()
            .iter()
            .map(|request| request.body_json::<Value>().unwrap())
            .collect::<Vec<_>>()
    };
    let second = |body: &Value| body.to_string().contains("and again");
    while !bodies().await.iter().any(second) {
        assert!(std::time::Instant::now() < deadline);
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    // The first run kept its model to the end (both its requests, plus any
    // plain retry of a stream attempt); the next turn used the new one.
    let all = bodies().await;
    let (next, first): (Vec<&Value>, Vec<&Value>) = all.iter().partition(|body| second(body));
    assert!(first.len() >= 2);
    assert!(first.iter().all(|body| body["model"] == "first-model"));
    assert!(next.iter().all(|body| body["model"] == "second-model"));
    assert!(
        next.iter()
            .all(|body| !body.to_string().contains("FIRST-MODEL-PRIVATE-REASONING")),
        "the new model never sees the old model's reasoning"
    );
}
