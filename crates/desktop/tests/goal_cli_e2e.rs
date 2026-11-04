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
    // A request that started before the pause finishes against this server.
    tokio::time::sleep(Duration::from_millis(300)).await;
}

fn reply(text: &str) -> ResponseTemplate {
    ResponseTemplate::new(200).set_body_json(json!({
        "choices": [{"message": {"role": "assistant", "content": text}, "finish_reason": "stop"}]
    }))
}

/// Whether the request offers the tool `name`.
fn offers(body: &Value, name: &str) -> bool {
    body["tools"].as_array().is_some_and(|tools| {
        tools
            .iter()
            .any(|tool| tool["function"]["name"] == name || tool["name"] == name)
    })
}

/// Whether the conversation already called the tool `name`.
fn called(body: &Value, name: &str) -> bool {
    body["messages"].as_array().is_some_and(|messages| {
        messages
            .iter()
            .any(|message| message["tool_calls"].to_string().contains(name))
    })
}

fn calls(name: &str, arguments: Value) -> ResponseTemplate {
    ResponseTemplate::new(200).set_body_json(json!({"choices":[{"message":{"role":"assistant","content":null,
        "tool_calls":[{"id":"call_1","type":"function","function":{"name":name,"arguments":arguments.to_string()}}]},
        "finish_reason":"tool_calls"}]}))
}

/// The agent's step once its work is done: it marks the goal complete with evidence for each
/// requirement, then answers with `answer`.
fn completes(body: &Value, answer: &str) -> ResponseTemplate {
    if offers(body, "update_goal") && !called(body, "update_goal") {
        calls(
            "update_goal",
            json!({"status": "complete", "evidence": [{"requirement": "the README has a title", "proof": "head -1 README.md: # Ledgerly"}]}),
        )
    } else {
        reply(answer)
    }
}

fn body(request: &Request) -> Value {
    request.body_json().unwrap_or(Value::Null)
}

#[tokio::test(flavor = "multi_thread")]
async fn cli_starts_a_goal_queues_a_message_and_reads_the_goal_back() {
    let llm = MockServer::builder().start().await;
    let gate = Gate::default();
    let held = gate.clone();
    Mock::given(method("POST"))
        .respond_with(move |request: &Request| {
            held.wait();
            let body = body(request);
            // The queued message's run finishes the work; the goal ends there.
            if body.to_string().contains("also add a license line") {
                completes(&body, "Added a title and a license line.")
            } else {
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
                assert_eq!(
                    goal["evidence"][0]["proof"], "head -1 README.md: # Ledgerly",
                    "{goal}"
                );
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
    let turns: Vec<Value> = requests.iter().map(body).collect();
    assert!(turns
        .iter()
        .any(|body| body.to_string().contains("also add a license line")));
    task.abort();
    settle(&state, &thread.id).await;
}

#[tokio::test(flavor = "multi_thread")]
async fn a_model_switch_mid_run_applies_next_turn_without_foreign_reasoning() {
    let llm = MockServer::builder().start().await;
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

/// A CLI call that may fail: its JSON result, or its error text.
fn try_call(data: &Path, command: &str, args: Value) -> Result<Value, String> {
    let output = std::process::Command::new(env!("CARGO_BIN_EXE_themis"))
        .arg("--data-dir")
        .arg(data)
        .args(["call", command, &args.to_string()])
        .env_remove("OPENCODE_KEY")
        .output()
        .expect("CLI");
    if output.status.success() {
        Ok(serde_json::from_slice(&output.stdout).unwrap_or(Value::Null))
    } else {
        Err(String::from_utf8_lossy(&output.stderr).into_owned())
    }
}

/// The text of the last user message a request carries.
fn last_user(body: &Value) -> String {
    body["messages"]
        .as_array()
        .and_then(|messages| messages.iter().rev().find(|m| m["role"] == "user"))
        .and_then(|message| message["content"].as_str())
        .unwrap_or_default()
        .to_owned()
}

/// The description of the tool `name` a request offers.
fn tool_description(body: &Value, name: &str) -> String {
    body["tools"]
        .as_array()
        .into_iter()
        .flatten()
        .find(|tool| tool["function"]["name"] == name)
        .and_then(|tool| tool["function"]["description"].as_str())
        .unwrap_or_default()
        .to_owned()
}

/// A chat on the shared server with a scripted model, driven through the CLI.
struct GoalApp {
    llm: MockServer,
    state: AppState,
    data: tempfile::TempDir,
    _project: tempfile::TempDir,
    thread: String,
    task: tokio::task::JoinHandle<Result<(), String>>,
}

impl GoalApp {
    async fn start(respond: impl Fn(&Value) -> ResponseTemplate + Send + Sync + 'static) -> Self {
        Self::start_with("{}", respond).await
    }

    async fn start_with(
        runtime: &str,
        respond: impl Fn(&Value) -> ResponseTemplate + Send + Sync + 'static,
    ) -> Self {
        let llm = MockServer::builder().start().await;
        Mock::given(method("POST"))
            .respond_with(move |request: &Request| respond(&body(request)))
            .mount(&llm)
            .await;
        let data = tempfile::tempdir().unwrap();
        let project = tempfile::tempdir().unwrap();
        std::fs::write(data.path().join("runtime.jsonc"), runtime).unwrap();
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
            .unwrap()
            .id;
        let server = Server::bind(state.clone(), data.path()).await.unwrap();
        let task = tokio::spawn(server.run());
        Self {
            llm,
            state,
            data,
            _project: project,
            thread,
            task,
        }
    }

    async fn bodies(&self) -> Vec<Value> {
        self.llm
            .received_requests()
            .await
            .unwrap()
            .iter()
            .map(body)
            .collect()
    }

    async fn finish(self) {
        self.task.abort();
        settle(&self.state, &self.thread).await;
    }
}

/// Polls `get_goal` through the CLI until `done` holds.
fn goal_until(data: &Path, thread: &str, what: &str, done: impl Fn(&Value) -> bool) -> Value {
    let deadline = std::time::Instant::now() + Duration::from_secs(60);
    loop {
        let goal = call(data, "get_goal", json!({"threadId": thread}));
        if done(&goal) {
            return goal;
        }
        assert!(std::time::Instant::now() < deadline, "{what}: {goal}");
        std::thread::sleep(Duration::from_millis(100));
    }
}

/// `/goal` creates the goal before any model request (Themis does it, no model decides), and
/// the run it starts is the goal's first turn: its message carries the continuation note with
/// the objective as the user's data, `update_goal` is offered, and no to-do tool or separate
/// check request exists anywhere.
#[tokio::test(flavor = "multi_thread")]
async fn cli_goal_command_creates_the_goal_without_a_model_and_carries_the_continuation() {
    let gate = Gate::default();
    let held = gate.clone();
    let app = GoalApp::start(move |body| {
        held.wait();
        completes(body, "The README has a title.")
    })
    .await;
    let (data, thread) = (app.data.path().to_owned(), app.thread.clone());
    let requests_before = app.llm.received_requests().await.unwrap().len();
    tokio::task::spawn_blocking({
        let (data, thread) = (data.clone(), thread.clone());
        move || {
            call(
                &data,
                "send_message",
                json!({"threadId": thread, "text": "/goal give the README a title"}),
            );
            // The goal exists while the model has not answered anything yet.
            let goal = call(&data, "get_goal", json!({"threadId": thread}));
            assert_eq!(goal["text"], "give the README a title", "{goal}");
            assert_eq!(goal["status"], "running", "{goal}");
        }
    })
    .await
    .unwrap();
    assert_eq!(requests_before, 0);
    gate.open();
    tokio::task::spawn_blocking({
        let (data, thread) = (data.clone(), thread.clone());
        move || {
            goal_until(&data, &thread, "never complete", |goal| {
                goal["status"] == "reached"
            })
        }
    })
    .await
    .unwrap();
    let bodies = app.bodies().await;
    let first = &bodies[0];
    let opening = last_user(first);
    assert!(
        opening.starts_with("/goal give the README a title"),
        "{opening}"
    );
    assert!(
        opening.contains("<objective>\ngive the README a title\n</objective>"),
        "{opening}"
    );
    assert!(opening.contains("Keep the whole objective"), "{opening}");
    assert!(offers(first, "update_goal"));
    // The context meter counts the goal note it carried as its own part, and the parts still
    // add up to what was used.
    let history = tokio::task::spawn_blocking({
        let (data, thread) = (data.clone(), thread.clone());
        move || call(&data, "get_thread_history", json!({"threadId": thread}))
    })
    .await
    .unwrap();
    let budgets: Vec<&Value> = history
        .as_array()
        .unwrap()
        .iter()
        .filter(|item| item["envelope"]["event"]["kind"] == "context_budget")
        .map(|item| &item["envelope"]["event"]["budget"])
        .collect();
    assert!(!budgets.is_empty(), "{history}");
    for budget in &budgets {
        let breakdown = &budget["breakdown"];
        assert!(breakdown["goal"].as_u64().unwrap() > 0, "{budget}");
        let parts: u64 = [
            "system",
            "checkpoint",
            "attachments",
            "conversation",
            "tool_activity",
            "reasoning",
            "goal",
        ]
        .iter()
        .map(|key| breakdown[key].as_u64().unwrap())
        .sum();
        assert_eq!(parts, budget["used_tokens"].as_u64().unwrap(), "{budget}");
    }
    for body in &bodies {
        assert!(!offers(body, "todo_write"), "no to-do tool");
        assert!(
            !body
                .to_string()
                .contains("You decide whether an agent has reached a goal"),
            "no separate goal check"
        );
    }
    app.finish().await;
}

/// Asked in plain words, the agent creates the goal with `create_goal`, whose description says
/// to do so only when the user asks; a second `create_goal` while that goal is unfinished is
/// refused and says why. Goal turns follow until the agent completes it.
#[tokio::test(flavor = "multi_thread")]
async fn cli_a_goal_asked_for_in_words_is_created_with_create_goal_once() {
    let app = GoalApp::start(|body| {
        let opening = last_user(body);
        let tries = body["messages"].as_array().map_or(0, |messages| {
            messages
                .iter()
                .filter(|m| m["tool_calls"].to_string().contains("create_goal"))
                .count()
        });
        if opening.contains("set a goal:") {
            return match tries {
                0 => calls(
                    "create_goal",
                    json!({"objective": "every page has a footer", "checklist": ["Every page has a footer"]}),
                ),
                1 => calls(
                    "create_goal",
                    json!({"objective": "every page has a footer, twice", "checklist": ["Footer twice"]}),
                ),
                _ => reply("The goal is set."),
            };
        }
        completes(body, "Every page has a footer.")
    })
    .await;
    let (data, thread) = (app.data.path().to_owned(), app.thread.clone());
    let goal = tokio::task::spawn_blocking(move || {
        call(
            &data,
            "send_message",
            json!({"threadId": thread, "text": "Please set a goal: every page has a footer"}),
        );
        goal_until(&data, &thread, "never complete", |goal| {
            goal["status"] == "reached"
        })
    })
    .await
    .unwrap();
    assert_eq!(goal["text"], "every page has a footer", "{goal}");
    assert_eq!(
        goal["turns"], 1,
        "one goal turn followed the run that created it"
    );
    let bodies = app.bodies().await;
    let description = tool_description(&bodies[0], "create_goal");
    assert!(
        description.contains("only when the user explicitly asks")
            && description.contains("never infer a goal"),
        "{description}"
    );
    assert!(!offers(&bodies[0], "set_goal"));
    assert!(
        bodies
            .iter()
            .any(|body| body.to_string().contains("already has an unfinished goal")),
        "the second create_goal was refused with its reason"
    );
    app.finish().await;
}

/// What the person sends while the goal runs, queued or steered in, joins the goal word for
/// word as an "Also asked" line, with no model involved; the next goal turn's continuation note
/// carries each line, and the goal shows them.
#[tokio::test(flavor = "multi_thread")]
async fn cli_messages_during_a_goal_are_added_to_it_verbatim() {
    let gate = Gate::default();
    let held = gate.clone();
    let app = GoalApp::start(move |body| {
        let opening = last_user(body);
        if opening.starts_with("/goal") {
            held.wait();
            return reply("Started on the README.");
        }
        if opening.contains("Turn 2.") {
            return completes(body, "Title, license and badge are there.");
        }
        reply("Noted.")
    })
    .await;
    let (data, thread) = (app.data.path().to_owned(), app.thread.clone());
    let steered = "also add a license line\n  (MIT, see `LICENSE`)";
    let queued = "and a build badge, please";
    let goal = tokio::task::spawn_blocking({
        let (data, thread) = (data.clone(), thread.clone());
        move || {
            call(
                &data,
                "send_message",
                json!({"threadId": thread, "text": "/goal give the README a title"}),
            );
            let message = call(
                &data,
                "enqueue_message",
                json!({"threadId": thread, "text": steered}),
            );
            call(
                &data,
                "steer_queued_message",
                json!({"threadId": thread, "messageId": message["id"]}),
            );
            call(
                &data,
                "enqueue_message",
                json!({"threadId": thread, "text": queued}),
            );
            let goal = call(&data, "get_goal", json!({"threadId": thread}));
            assert_eq!(goal["also_asked"], json!([steered]), "{goal}");
            gate.open();
            goal_until(&data, &thread, "never complete", |goal| {
                goal["status"] == "reached"
            })
        }
    })
    .await
    .unwrap();
    assert_eq!(goal["also_asked"], json!([steered, queued]), "{goal}");
    assert_eq!(goal["text"], "give the README a title");
    let bodies = app.bodies().await;
    let turn = bodies
        .iter()
        .find(|body| last_user(body).contains("Turn 2."))
        .expect("goal turn 2");
    let note = last_user(turn);
    assert!(
        note.contains(&format!(
            "give the README a title\nAlso asked: {steered}\nAlso asked: {queued}\n</objective>"
        )),
        "{note}"
    );
    app.finish().await;
}

/// Editing the goal's text on the card (`edit_goal`) gives the next goal turn the "objective
/// updated" note before the continuation, once; the turn after that has only the continuation.
#[tokio::test(flavor = "multi_thread")]
async fn cli_an_edited_goal_tells_the_next_turn_once() {
    let gate = Gate::default();
    let held = gate.clone();
    let app = GoalApp::start(move |body| {
        let opening = last_user(body);
        if opening.starts_with("/goal") {
            held.wait();
        }
        if opening.contains("Turn 3.") {
            return completes(body, "Done with the new objective.");
        }
        reply("Worked on it.")
    })
    .await;
    let (data, thread) = (app.data.path().to_owned(), app.thread.clone());
    tokio::task::spawn_blocking(move || {
        call(
            &data,
            "send_message",
            json!({"threadId": thread, "text": "/goal write three colors"}),
        );
        call(&data, "pause_goal", json!({"threadId": thread}));
        let edited = call(
            &data,
            "edit_goal",
            json!({"threadId": thread, "text": "write four colors"}),
        );
        assert_eq!(edited["text"], "write four colors");
        // Editing a goal that isn't there is refused.
        assert!(try_call(&data, "edit_goal", json!({"threadId": "nope", "text": "x"})).is_err());
        gate.open();
        while call(&data, "get_thread", json!({"threadId": thread}))["running"] == true {
            std::thread::sleep(Duration::from_millis(50));
        }
        call(&data, "resume_goal", json!({"threadId": thread}));
        goal_until(&data, &thread, "never complete", |goal| {
            goal["status"] == "reached"
        });
    })
    .await
    .unwrap();
    let bodies = app.bodies().await;
    let turn = |number: u32| {
        bodies
            .iter()
            .find(|body| last_user(body).contains(&format!("Turn {number}.")))
            .map(last_user)
            .unwrap_or_else(|| panic!("goal turn {number}"))
    };
    let second = turn(2);
    let updated = second
        .find("The user edited the goal")
        .expect("update note");
    let objective = second
        .find("<objective>\nwrite four colors\n</objective>")
        .expect("new objective");
    assert!(updated < objective, "{second}");
    assert!(!turn(3).contains("The user edited the goal"), "only once");
    app.finish().await;
}
