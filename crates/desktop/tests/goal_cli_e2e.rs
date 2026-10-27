//! Goal mode and the prompt queue through the real shared server and the
//! `themis` CLI, plus a model switch mid-run (no foreign reasoning replayed).

use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

use serde_json::{json, Value};
use themis_desktop::sink::ChannelSink;
use themis_desktop::{
    server::Server,
    state::AppState,
    types::{GoalStatus, ProviderKind},
};
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

/// Holds a scripted answer until the test opens it, so actions that must land
/// while a run is going (queue, steer, pause, switch) do so however slow the
/// machine is.
#[derive(Clone, Default)]
struct Gate(Arc<std::sync::atomic::AtomicBool>);

impl Gate {
    fn open(&self) {
        self.0.store(true, std::sync::atomic::Ordering::SeqCst);
    }

    fn wait(&self) {
        let waited = std::time::Instant::now();
        while !self.0.load(std::sync::atomic::Ordering::SeqCst)
            && waited.elapsed() < Duration::from_secs(60)
        {
            std::thread::sleep(Duration::from_millis(10));
        }
    }
}

/// Leaves the chat quiet before the test ends: runs live on the app's shared
/// runtime and would outlive the test, and a later test's mock server can get
/// this one's port. Pausing also cancels any goal retry timer.
async fn settle(state: &AppState, thread_id: &str) {
    let sink: Arc<dyn themis_desktop::sink::EventSink> = Arc::new(ChannelSink::channel().0);
    if state
        .goal_for(thread_id)
        .is_some_and(|goal| !matches!(goal.status, GoalStatus::Paused | GoalStatus::Reached))
    {
        let _ = state
            .pause_goal(Arc::clone(&sink), thread_id.to_owned())
            .await;
    }
    for message in state
        .list_queue(thread_id.to_owned())
        .await
        .unwrap_or_default()
    {
        let _ = state
            .remove_queued_message(Arc::clone(&sink), thread_id.to_owned(), message.id)
            .await;
    }
    let deadline = std::time::Instant::now() + Duration::from_secs(120);
    while state
        .get_thread(thread_id)
        .await
        .is_ok_and(|thread| thread.running)
        && std::time::Instant::now() < deadline
    {
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    // A goal check that started before the pause finishes against this server.
    tokio::time::sleep(Duration::from_millis(300)).await;
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
    let gate = Gate::default();
    let held = gate.clone();
    Mock::given(method("POST"))
        .respond_with(move |request: &Request| {
            if is_check(request) {
                reply(r#"{"verdict":"reached","reason":"The README has a title.","progress":"title present"}"#)
            } else {
                held.wait();
                reply("Added a title.")
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
        gate.open();
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
        // Removing the goal clears it; the chat has none afterwards.
        assert_eq!(
            call(&data_dir, "remove_goal", json!({"threadId": thread_id})),
            Value::Null
        );
        assert_eq!(
            call(&data_dir, "get_goal", json!({"threadId": thread_id})),
            Value::Null
        );
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
    settle(&state, &thread.id).await;
}

#[tokio::test(flavor = "multi_thread")]
async fn a_model_switch_mid_run_applies_next_turn_without_foreign_reasoning() {
    let llm = MockServer::start().await;
    let gate = Gate::default();
    let held = gate.clone();
    Mock::given(method("POST"))
        .respond_with(move |request: &Request| {
            let body: Value = request.body_json().unwrap();
            let has_tool_result = body["messages"]
                .as_array()
                .is_some_and(|messages| messages.iter().any(|m| m["role"] == "tool"));
            if body["model"] == "first-model" && !has_tool_result {
                held.wait();
                ResponseTemplate::new(200)
                    .set_body_json(json!({"choices":[{"message":{"role":"assistant","content":null,"reasoning_content":"FIRST-MODEL-PRIVATE-REASONING","tool_calls":[{"id":"l","type":"function","function":{"name":"list_dir","arguments":"{\"path\":\".\"}"}}]},"finish_reason":"tool_calls"}]}))
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
    gate.open();
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
    settle(&state, &thread.id).await;
}

/// `goal.check_model` (here from the project's config) sends the goal check to that
/// model; the turns stay on the chat's model.
#[tokio::test(flavor = "multi_thread")]
async fn the_goal_check_uses_goal_check_model_and_turns_use_the_chats_model() {
    let llm = MockServer::start().await;
    Mock::given(method("POST"))
        .respond_with(|request: &Request| {
            if is_check(request) {
                reply(r#"{"verdict":"reached","reason":"Done.","progress":"done"}"#)
            } else {
                reply("Did it.")
            }
        })
        .mount(&llm)
        .await;
    let data = tempfile::tempdir().unwrap();
    let project = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(project.path().join(".themis")).unwrap();
    std::fs::write(
        project.path().join(".themis/config.jsonc"),
        r#"{"goal":{"check_model":"check-model"}}"#,
    )
    .unwrap();
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
            json!({"threadId": thread_id, "text": "/goal do the thing"}),
        );
        let deadline = std::time::Instant::now() + Duration::from_secs(60);
        while call(&data_dir, "get_goal", json!({"threadId": thread_id}))["status"] != "reached" {
            assert!(std::time::Instant::now() < deadline, "goal never reached");
            std::thread::sleep(Duration::from_millis(200));
        }
    })
    .await
    .unwrap();
    let requests = llm.received_requests().await.unwrap();
    let model = |request: &Request| {
        request.body_json::<Value>().unwrap_or(Value::Null)["model"]
            .as_str()
            .unwrap_or_default()
            .to_owned()
    };
    let checks: Vec<String> = requests.iter().filter(|r| is_check(r)).map(model).collect();
    let turns: Vec<String> = requests
        .iter()
        .filter(|r| !is_check(r))
        .map(model)
        .collect();
    assert!(
        !checks.is_empty() && checks.iter().all(|m| m == "check-model"),
        "{checks:?}"
    );
    assert!(
        !turns.is_empty() && turns.iter().all(|m| m == "test-model"),
        "{turns:?}"
    );
    settle(&state, &thread.id).await;
    task.abort();
}

/// Through the CLI: a request steered in while a goal runs is kept on the goal
/// (`get_goal` shows it under `added`) and shown to the goal check.
#[tokio::test(flavor = "multi_thread")]
async fn cli_a_steer_during_a_goal_is_added_to_it() {
    let llm = MockServer::start().await;
    let gate = Gate::default();
    let held = gate.clone();
    Mock::given(method("POST"))
        .respond_with(move |request: &Request| {
            if is_check(request) {
                reply(r#"{"verdict":"reached","reason":"Title and license line are there.","progress":""}"#)
            } else {
                held.wait();
                reply("Added a title and a license line.")
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
        let queued = call(
            &data_dir,
            "enqueue_message",
            json!({"threadId": thread_id, "text": "also add a license line"}),
        );
        call(
            &data_dir,
            "steer_queued_message",
            json!({"threadId": thread_id, "messageId": queued["id"]}),
        );
        let goal = call(&data_dir, "get_goal", json!({"threadId": thread_id}));
        assert_eq!(goal["added"], json!(["also add a license line"]), "{goal}");
        gate.open();
        let deadline = std::time::Instant::now() + Duration::from_secs(60);
        while call(&data_dir, "get_goal", json!({"threadId": thread_id}))["status"] != "reached" {
            assert!(std::time::Instant::now() < deadline, "goal never reached");
            std::thread::sleep(Duration::from_millis(200));
        }
    })
    .await
    .unwrap();
    let requests = llm.received_requests().await.unwrap();
    let check = requests
        .iter()
        .find(|request| is_check(request))
        .map(|request| request.body_json::<Value>().unwrap())
        .unwrap();
    let input = check["messages"][1]["content"].as_str().unwrap();
    assert!(input.contains("1. also add a license line"), "{input}");
    task.abort();
    settle(&state, &thread.id).await;
}

/// A queued message's run that finishes the goal is checked, and an answer
/// that already says so gets no extra goal turn or announcement.
#[tokio::test(flavor = "multi_thread")]
async fn cli_a_message_that_finishes_the_goal_is_not_repeated() {
    let llm = MockServer::start().await;
    let gate = Gate::default();
    let held = gate.clone();
    Mock::given(method("POST"))
        .respond_with(move |request: &Request| {
            if is_check(request) {
                reply(r#"{"verdict":"reached","reason":"The title is there.","told_user":true}"#)
            } else {
                held.wait();
                reply("Added the title; the goal is done.")
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
        call(
            &data_dir,
            "enqueue_message",
            json!({"threadId": thread_id, "text": "use the title Ledgerly"}),
        );
        gate.open();
        let deadline = std::time::Instant::now() + Duration::from_secs(60);
        while call(&data_dir, "get_goal", json!({"threadId": thread_id}))["status"] != "reached" {
            assert!(std::time::Instant::now() < deadline, "goal never reached");
            std::thread::sleep(Duration::from_millis(200));
        }
    })
    .await
    .unwrap();
    settle(&state, &thread.id).await;
    let requests = llm.received_requests().await.unwrap();
    let mut agent: Vec<Value> = requests
        .iter()
        .filter(|request| !is_check(request))
        .map(|request| request.body_json::<Value>().unwrap())
        .collect();
    agent.dedup();
    assert_eq!(
        agent.len(),
        2,
        "the /goal run and the message, nothing after"
    );
    assert!(!agent
        .iter()
        .any(|body| body.to_string().contains("[Goal — runtime")));
    task.abort();
}
