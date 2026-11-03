//! Prompt queue, steering and goal mode through the real app state, with a
//! scripted model (wiremock). A goal ends when the scripted agent calls `update_goal`.

use std::sync::{Arc, Mutex};
use std::time::Duration;

use serde_json::{json, Value};
use themis_desktop::sink::{ChannelSink, TestEvent};
use themis_desktop::state::AppState;
use themis_desktop::types::{GoalNoteKind, GoalStatus, ProviderKind, ThreadEvent};
use tokio::sync::mpsc::UnboundedReceiver;
use wiremock::matchers::method;
use wiremock::{Mock, MockServer, Request, ResponseTemplate};

/// Generous: CI runs every test binary at once.
const DEADLINE: Duration = Duration::from_secs(120);

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

fn reply(text: &str) -> ResponseTemplate {
    ResponseTemplate::new(200).set_body_json(json!({
        "choices": [{"message": {"role": "assistant", "content": text}, "finish_reason": "stop"}]
    }))
}

fn body(request: &Request) -> Value {
    request.body_json().unwrap_or(Value::Null)
}

/// Requests with distinct conversations: a stream attempt and its plain retry
/// carry the same messages and count once.
fn distinct(bodies: Vec<Value>) -> Vec<Value> {
    let mut seen = std::collections::HashSet::new();
    bodies
        .into_iter()
        .filter(|body| seen.insert(body["messages"].to_string()))
        .collect()
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

/// The agent's step once its work is done: it marks the goal complete with evidence for each
/// requirement, then answers with `answer`.
fn completes(body: &Value, answer: &str) -> ResponseTemplate {
    if offers(body, "update_goal") && !called(body, "update_goal") {
        calls(
            "update_goal",
            json!({"status": "complete", "evidence": [{"requirement": "the work is done", "proof": "done.txt reads done"}]}),
        )
    } else {
        reply(answer)
    }
}

fn mentions(body: &Value, needle: &str) -> bool {
    body["messages"].as_array().is_some_and(|messages| {
        messages.iter().any(|message| {
            message["content"]
                .as_str()
                .is_some_and(|text| text.contains(needle))
        })
    })
}

async fn chat(server: &MockServer) -> (AppState, tempfile::TempDir, tempfile::TempDir, String) {
    let data = tempfile::tempdir().unwrap();
    let project = tempfile::tempdir().unwrap();
    // These tests cover the goal turn loop and queue; QA turns have their own tests.
    std::fs::write(
        data.path().join("runtime.jsonc"),
        r#"{"goal_qa_turns": false}"#,
    )
    .unwrap();
    let state = AppState::new_for_test(data.path().join("settings.json"));
    state
        .set_secret("go".into(), "synthetic-test-key".into())
        .await
        .unwrap();
    state.set_go_base_url_override(Some(server.uri()));
    let thread = state
        .create_thread(
            project.path().to_string_lossy().into_owned(),
            ProviderKind::Go,
            Some("test-model".into()),
        )
        .await
        .unwrap();
    (state, data, project, thread.id)
}

/// Collects thread events until `done` holds for one of them.
async fn until(
    rx: &mut UnboundedReceiver<TestEvent>,
    seen: &mut Vec<ThreadEvent>,
    done: impl Fn(&ThreadEvent) -> bool,
) {
    tokio::time::timeout(DEADLINE, async {
        while let Some(event) = rx.recv().await {
            if let TestEvent::Thread(envelope) = event {
                let stop = done(&envelope.event);
                seen.push(envelope.event);
                if stop {
                    return;
                }
            }
        }
        panic!("event stream ended");
    })
    .await
    .expect("expected event before the deadline");
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
    let deadline = std::time::Instant::now() + DEADLINE;
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

/// Waits until the chat's goal satisfies `done`.
async fn goal_until(state: &AppState, thread_id: &str, done: impl Fn(GoalStatus) -> bool) {
    let reached = tokio::time::timeout(DEADLINE, async {
        loop {
            if state
                .goal_for(thread_id)
                .is_some_and(|goal| done(goal.status))
            {
                return;
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
    })
    .await;
    if reached.is_err() {
        let running = state.get_thread(thread_id).await.map(|t| t.running);
        panic!(
            "goal state before the deadline; goal: {:?}; running: {running:?}",
            state.goal_for(thread_id)
        );
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn queued_messages_wait_steers_reach_the_run_and_the_queue_drains() {
    let server = MockServer::builder().start().await;
    let gate = Gate::default();
    let held = gate.clone();
    Mock::given(method("POST"))
        .respond_with(move |request: &Request| {
            let body = body(request);
            if mentions(&body, "Use tabs.") && !mentions(&body, "second message") {
                reply("Adjusted to tabs.")
            } else if mentions(&body, "second message") {
                reply("Second done.")
            } else {
                // Answers once the test has queued and steered.
                held.wait();
                reply("First done.")
            }
        })
        .mount(&server)
        .await;
    let (state, data, _project, thread) = chat(&server).await;
    let (sink, mut rx) = ChannelSink::channel();
    let sink = Arc::new(sink);
    state
        .send_message(sink.clone(), thread.clone(), "first".into())
        .await
        .unwrap();

    let steer = state
        .enqueue_message(
            sink.clone(),
            thread.clone(),
            "Use tabs.".into(),
            None,
            vec![],
        )
        .await
        .unwrap();
    let later = state
        .enqueue_message(sink.clone(), thread.clone(), "draft".into(), None, vec![])
        .await
        .unwrap();
    let edited = state
        .update_queued_message(
            sink.clone(),
            thread.clone(),
            later.id.clone(),
            Some("second message".into()),
            None,
        )
        .await
        .unwrap();
    assert_eq!(edited.text, "second message");
    let gone = state
        .enqueue_message(
            sink.clone(),
            thread.clone(),
            "never mind".into(),
            None,
            vec![],
        )
        .await
        .unwrap();
    state
        .remove_queued_message(sink.clone(), thread.clone(), gone.id)
        .await
        .unwrap();
    let queued = state.list_queue(thread.clone()).await.unwrap();
    assert_eq!(
        queued.iter().map(|m| m.text.as_str()).collect::<Vec<_>>(),
        ["Use tabs.", "second message"]
    );
    assert!(data
        .path()
        .join("queues")
        .join(format!("{thread}.json"))
        .exists());
    state
        .steer_queued_message(sink.clone(), thread.clone(), steer.id)
        .await
        .unwrap();
    gate.open();

    let mut seen = Vec::new();
    until(
        &mut rx,
        &mut seen,
        |event| matches!(event, ThreadEvent::Finished { result, .. } if result == "Second done."),
    )
    .await;
    let steered: Vec<_> = seen
        .iter()
        .filter_map(|event| match event {
            ThreadEvent::Steered { text } => Some(text.as_str()),
            _ => None,
        })
        .collect();
    assert_eq!(steered, ["Use tabs."]);
    let finished: Vec<_> = seen
        .iter()
        .filter_map(|event| match event {
            ThreadEvent::Finished { result, .. } => Some(result.as_str()),
            _ => None,
        })
        .collect();
    assert_eq!(finished, ["Adjusted to tabs.", "Second done."]);
    assert!(seen.iter().any(
        |event| matches!(event, ThreadEvent::UserMessage { text, .. } if text == "second message")
    ));
    // The steer reached the model once, before the queued message was sent.
    let bodies: Vec<Value> = distinct(
        server
            .received_requests()
            .await
            .unwrap()
            .iter()
            .map(body)
            .collect(),
    );
    assert_eq!(
        bodies
            .iter()
            .filter(|b| mentions(b, "Use tabs.") && !mentions(b, "second message"))
            .count(),
        1
    );
    assert!(state.list_queue(thread.clone()).await.unwrap().is_empty());
    assert!(!data
        .path()
        .join("queues")
        .join(format!("{thread}.json"))
        .exists());
    // Durable history keeps the steer as the user's words.
    let history = serde_json::to_string(&state.get_thread_history(&thread).await.unwrap()).unwrap();
    assert!(history.contains("\"steered\""));
    // A steer needs a running run.
    let idle = state
        .enqueue_message(sink.clone(), thread.clone(), "after".into(), None, vec![])
        .await
        .unwrap();
    let error = state
        .steer_queued_message(sink, thread.clone(), idle.id)
        .await
        .err();
    // The idle chat sent "after" at once, so it is no longer queued.
    assert!(error.is_some());
    settle(&state, &thread).await;
}

#[tokio::test(flavor = "multi_thread")]
async fn a_running_goal_saves_its_elapsed_time_during_a_turn() {
    // A live goal showed only the current turn's time: elapsed time was saved
    // at turn boundaries, so each app restart mid-turn lost the whole turn.
    let server = MockServer::builder().start().await;
    let released = Gate::default();
    let gate = released.clone();
    Mock::given(method("POST"))
        .respond_with(move |request: &Request| {
            gate.wait();
            completes(&body(request), "Worked on it.")
        })
        .mount(&server)
        .await;
    let (state, data, _project, thread) = chat(&server).await;
    let (sink, _rx) = ChannelSink::channel();
    let sink: Arc<dyn themis_desktop::sink::EventSink> = Arc::new(sink);
    state
        .send_message(sink.clone(), thread.clone(), "/goal keep working".into())
        .await
        .unwrap();
    tokio::time::sleep(Duration::from_millis(400)).await;
    state.bank_running_goals(sink.as_ref());
    let saved: themis_desktop::types::Goal = serde_json::from_str(
        &std::fs::read_to_string(data.path().join("goals").join(format!("{thread}.json"))).unwrap(),
    )
    .unwrap();
    assert_eq!(saved.status, GoalStatus::Running);
    assert!(saved.active_ms >= 300, "saved {} ms", saved.active_ms);
    released.open();
    goal_until(&state, &thread, |status| status == GoalStatus::Reached).await;
    settle(&state, &thread).await;
}

#[tokio::test(flavor = "multi_thread")]
async fn pause_holds_the_goal_resume_continues_and_turns_keep_going() {
    let server = MockServer::builder().start().await;
    // The first turn's answer waits for the test to pause the goal, so the
    // pause always lands while that turn runs, however slow the machine.
    let released = Gate::default();
    let gate = released.clone();
    Mock::given(method("POST"))
        .respond_with(move |_: &Request| {
            gate.wait();
            reply("Tried another cache setting.")
        })
        .mount(&server)
        .await;
    let (state, _data, _project, thread) = chat(&server).await;
    let (sink, _rx) = ChannelSink::channel();
    let sink: Arc<dyn themis_desktop::sink::EventSink> = Arc::new(sink);
    state
        .send_message(sink.clone(), thread.clone(), "/goal speed up p95".into())
        .await
        .unwrap();
    let paused = state
        .pause_goal(sink.clone(), thread.clone())
        .await
        .unwrap();
    assert_eq!(paused.status, GoalStatus::Paused);
    released.open();
    // The running turn finishes; no further turn starts while paused. (One turn can make more
    // than one request: a stream attempt and its plain retry; so count goal turns, not
    // requests.)
    let goal_turns = |requests: &[Request]| {
        distinct(requests.iter().map(body).collect())
            .iter()
            .filter(|body| mentions(body, "[Goal — runtime, not from the user]"))
            .count()
    };
    tokio::time::sleep(Duration::from_millis(2500)).await;
    let requests = server.received_requests().await.unwrap();
    assert_eq!(
        goal_turns(&requests),
        0,
        "only the first turn ran; goal now {:?}; requests: {:#?}",
        state.goal_for(&thread),
        requests
            .iter()
            .map(|request| {
                let body = body(request);
                let last = body["messages"]
                    .as_array()
                    .and_then(|messages| messages.last())
                    .map(|message| {
                        message["content"]
                            .to_string()
                            .chars()
                            .take(160)
                            .collect::<String>()
                    })
                    .unwrap_or_default();
                format!("stream={} last={last}", body["stream"])
            })
            .collect::<Vec<_>>()
    );
    tokio::time::sleep(Duration::from_millis(500)).await;
    assert_eq!(goal_turns(&server.received_requests().await.unwrap()), 0);
    let elapsed = state.goal_for(&thread).unwrap().active_ms;
    tokio::time::sleep(Duration::from_millis(300)).await;
    assert_eq!(
        state.goal_for(&thread).unwrap().active_ms,
        elapsed,
        "the clock stops while paused"
    );

    state
        .resume_goal(sink.clone(), thread.clone())
        .await
        .unwrap();
    // Every turn makes no progress and changes no file; the goal keeps going.
    let kept_going = tokio::time::timeout(DEADLINE, async {
        while goal_turns(&server.received_requests().await.unwrap()) < 3 {
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
    })
    .await;
    assert!(kept_going.is_ok(), "goal: {:?}", state.goal_for(&thread));
    let goal = state.goal_for(&thread).unwrap();
    assert_eq!(goal.status, GoalStatus::Running);
    state
        .resume_goal(sink.clone(), thread.clone())
        .await
        .unwrap();
    assert_eq!(state.goal_for(&thread).unwrap().status, GoalStatus::Running);
    state.pause_goal(sink, thread.clone()).await.unwrap();
    settle(&state, &thread).await;
}

#[tokio::test(flavor = "multi_thread")]
async fn a_used_up_quota_pauses_the_goal_and_a_model_switch_resumes_it() {
    let server = MockServer::builder().start().await;
    let quota = Arc::new(Mutex::new(true));
    let out_of_quota = Arc::clone(&quota);
    Mock::given(method("POST"))
        .respond_with(move |request: &Request| {
            let body = body(request);
            if *out_of_quota.lock().unwrap() {
                return ResponseTemplate::new(429)
                    .set_body_string("monthly usage limit reached for your plan");
            }
            completes(&body, "Finished the work.")
        })
        .mount(&server)
        .await;
    let (state, _data, _project, thread) = chat(&server).await;
    let (sink, mut rx) = ChannelSink::channel();
    let sink: Arc<dyn themis_desktop::sink::EventSink> = Arc::new(sink);
    state
        .send_message(sink.clone(), thread.clone(), "/goal finish the work".into())
        .await
        .unwrap();
    goal_until(&state, &thread, |status| status == GoalStatus::Quota).await;
    let mut seen = Vec::new();
    until(&mut rx, &mut seen, |event| {
        matches!(
            event,
            ThreadEvent::GoalNote {
                outcome: GoalNoteKind::Quota,
                ..
            }
        )
    })
    .await;
    *quota.lock().unwrap() = false;
    state
        .set_provider(thread.clone(), ProviderKind::Go, Some("other-model".into()))
        .await
        .unwrap();
    state.goal_model_changed(sink, &thread).await;
    goal_until(&state, &thread, |status| status == GoalStatus::Reached).await;
    let last_turn = server
        .received_requests()
        .await
        .unwrap()
        .iter()
        .map(body)
        .next_back()
        .unwrap();
    assert_eq!(last_turn["model"], "other-model");
    settle(&state, &thread).await;
}

#[tokio::test(flavor = "multi_thread")]
async fn a_running_goal_resumes_when_the_app_starts_again() {
    let server = MockServer::builder().start().await;
    Mock::given(method("POST"))
        .respond_with(|request: &Request| completes(&body(request), "Re-checked and finished."))
        .mount(&server)
        .await;
    let (state, data, _project, thread) = chat(&server).await;
    // As if Themis closed while the goal ran: the goal file says running.
    std::fs::create_dir_all(data.path().join("goals")).unwrap();
    std::fs::write(
        data.path().join("goals").join(format!("{thread}.json")),
        json!({"text":"finish the work","status":"running","started_at":0,"active_ms":5000,"running_since":0,"turns":3}).to_string(),
    )
    .unwrap();
    // The restarted app reads it fresh.
    let restart = std::time::Instant::now();
    let restarted = AppState::new_for_test(data.path().join("settings.json"));
    restarted
        .set_secret("go".into(), "synthetic-test-key".into())
        .await
        .unwrap();
    restarted.set_go_base_url_override(Some(server.uri()));
    drop(state);
    let (sink, _rx) = ChannelSink::channel();
    restarted.set_scheduler_sink(Arc::new(sink));
    goal_until(&restarted, &thread, |status| status == GoalStatus::Reached).await;
    let goal = restarted.goal_for(&thread).unwrap();
    assert_eq!(goal.turns, 4);
    // Time with the app closed did not count: only the 5 s already counted
    // plus the time since the restart (the file says it ran since 1970).
    let since_restart = u64::try_from(restart.elapsed().as_millis()).unwrap();
    assert!(
        goal.active_ms <= 5_000 + since_restart,
        "{} > 5000 + {since_restart}",
        goal.active_ms
    );
    let first = server
        .received_requests()
        .await
        .unwrap()
        .iter()
        .map(body)
        .next()
        .unwrap();
    assert!(mentions(&first, "interrupted when Themis closed"));
    settle(&restarted, &thread).await;
}

#[tokio::test(flavor = "multi_thread")]
async fn the_agent_sets_a_goal_with_its_tool_and_turns_follow() {
    let server = MockServer::builder().start().await;
    Mock::given(method("POST"))
        .respond_with(move |request: &Request| {
            let body = body(request);
            if mentions(&body, "[Goal — runtime") {
                return completes(&body, "All green.");
            }
            let used_tool = body["messages"]
                .as_array()
                .is_some_and(|messages| messages.iter().any(|m| m["role"] == "tool"));
            if mentions(&body, "make it a goal") && !used_tool && !mentions(&body, "[Goal — runtime") {
                ResponseTemplate::new(200).set_body_json(json!({"choices":[{"message":{"role":"assistant","content":null,"tool_calls":[{"id":"g","type":"function","function":{"name":"create_goal","arguments":"{\"objective\":\"get every test green\",\"checklist\":[\"Every test is green\"]}"}}]},"finish_reason":"tool_calls"}]}))
            } else {
                reply("Working on it.")
            }
        })
        .mount(&server)
        .await;
    let (state, _data, _project, thread) = chat(&server).await;
    let (sink, _rx) = ChannelSink::channel();
    state
        .send_message(Arc::new(sink), thread.clone(), "yes, make it a goal".into())
        .await
        .unwrap();
    goal_until(&state, &thread, |status| status == GoalStatus::Reached).await;
    let goal = state.goal_for(&thread).unwrap();
    assert_eq!(goal.text, "get every test green");
    assert_eq!(
        goal.turns, 1,
        "the agent's own run set it; one goal turn followed"
    );
    assert_eq!(goal.evidence.len(), 1, "{goal:?}");
    settle(&state, &thread).await;
}

fn calls(name: &str, arguments: Value) -> ResponseTemplate {
    ResponseTemplate::new(200).set_body_json(json!({"choices":[{"message":{"role":"assistant","content":null,"tool_calls":[{"id":"call_1","type":"function","function":{"name":name,"arguments":arguments.to_string()}}]},"finish_reason":"tool_calls"}]}))
}

/// Found in a live run: asked to stop the goal, the agent had no way to and said
/// it had, and the next turn started anyway.
#[tokio::test(flavor = "multi_thread")]
async fn asked_to_stop_the_agent_pauses_the_goal_and_no_turn_follows() {
    let server = MockServer::builder().start().await;
    let gate = Gate::default();
    let held = gate.clone();
    Mock::given(method("POST"))
        .respond_with(move |request: &Request| {
            let body = body(request);
            let sent = body["messages"].to_string();
            if called(&body, "update_goal") {
                reply("The goal is paused.")
            } else if sent.contains("Please stop the goal") {
                calls("update_goal", json!({"status": "paused"}))
            } else {
                held.wait();
                calls("list_dir", json!({"directory_path": "."}))
            }
        })
        .mount(&server)
        .await;
    let (state, _data, _project, thread) = chat(&server).await;
    let (sink, _rx) = ChannelSink::channel();
    let sink = Arc::new(sink);
    state
        .send_message(
            sink.clone(),
            thread.clone(),
            "/goal finish the thing".into(),
        )
        .await
        .unwrap();
    let stop = state
        .enqueue_message(
            sink.clone(),
            thread.clone(),
            "Please stop the goal".into(),
            None,
            vec![],
        )
        .await
        .unwrap();
    state
        .steer_queued_message(sink.clone(), thread.clone(), stop.id)
        .await
        .unwrap();
    gate.open();
    goal_until(&state, &thread, |status| status == GoalStatus::Paused).await;
    settle(&state, &thread).await;
    let goal = state.goal_for(&thread).unwrap();
    assert_eq!(goal.status, GoalStatus::Paused);
    assert_eq!(goal.turns, 1);
    let goal_turns = distinct(
        server
            .received_requests()
            .await
            .unwrap()
            .iter()
            .map(body)
            .collect(),
    )
    .iter()
    .filter(|body| mentions(body, "[Goal — runtime"))
    .count();
    assert_eq!(goal_turns, 0, "no goal turn followed the pause");
}

/// The goal judges itself: `update_goal` complete with no evidence, or with an entry that
/// names no proof, is refused with the reason in the tool result; with evidence for each
/// requirement it ends the goal, the evidence is kept for the card, and no further turn starts.
#[tokio::test(flavor = "multi_thread")]
async fn complete_needs_evidence_and_then_ends_the_goal() {
    let server = MockServer::builder().start().await;
    Mock::given(method("POST"))
        .respond_with(|request: &Request| {
            let body = body(request);
            let tries = body["messages"]
                .as_array()
                .map_or(0, |messages| {
                    messages
                        .iter()
                        .filter(|message| message["tool_calls"].to_string().contains("update_goal"))
                        .count()
                });
            match tries {
                0 => calls("update_goal", json!({"status": "complete"})),
                1 => calls(
                    "update_goal",
                    json!({"status": "complete", "evidence": [{"requirement": "colors.txt lists four colors", "proof": ""}]}),
                ),
                2 => calls(
                    "update_goal",
                    json!({"status": "complete", "evidence": [
                        {"requirement": "colors.txt lists four colors", "proof": "wc -l colors.txt: 4"},
                        {"requirement": "the test passes", "proof": "python3 check.py: PASS"}
                    ]}),
                ),
                _ => reply("Done: colors.txt has four lines and check.py passes."),
            }
        })
        .mount(&server)
        .await;
    let (state, _data, _project, thread) = chat(&server).await;
    let (sink, mut rx) = ChannelSink::channel();
    state
        .send_message(
            Arc::new(sink),
            thread.clone(),
            "/goal colors.txt lists four colors".into(),
        )
        .await
        .unwrap();
    goal_until(&state, &thread, |status| status == GoalStatus::Reached).await;
    let mut seen = Vec::new();
    until(&mut rx, &mut seen, |event| {
        matches!(event, ThreadEvent::Finished { .. })
    })
    .await;
    settle(&state, &thread).await;
    let refusals: Vec<&String> = seen
        .iter()
        .filter_map(|event| match event {
            ThreadEvent::ToolFinished {
                tool,
                ok: false,
                output,
                ..
            } if tool == "update_goal" => Some(output),
            _ => None,
        })
        .collect();
    assert_eq!(refusals.len(), 2, "{refusals:?}");
    assert!(refusals[0].contains("needs `evidence`"), "{}", refusals[0]);
    assert!(
        refusals[1].contains("entry 1 needs both"),
        "{}",
        refusals[1]
    );
    let goal = state.goal_for(&thread).unwrap();
    assert_eq!(goal.evidence.len(), 2, "{goal:?}");
    assert_eq!(goal.evidence[1].proof, "python3 check.py: PASS");
    assert_eq!(goal.turns, 1, "no goal turn after complete");
    assert!(seen.iter().any(|event| matches!(
        event,
        ThreadEvent::GoalNote {
            outcome: GoalNoteKind::Reached,
            ..
        }
    )));
    let goal_turns = distinct(
        server
            .received_requests()
            .await
            .unwrap()
            .iter()
            .map(body)
            .collect(),
    )
    .iter()
    .filter(|body| mentions(body, "[Goal — runtime"))
    .count();
    assert_eq!(goal_turns, 0);
}

/// A message the person queued while the goal turn ran must be read before the goal can be
/// complete: `update_goal` complete is refused while it waits, and the next goal turn carries it
/// as an "Also asked" line.
#[tokio::test(flavor = "multi_thread")]
async fn complete_waits_for_a_message_the_person_queued_meanwhile() {
    let server = MockServer::builder().start().await;
    Mock::given(method("POST"))
        .respond_with(|request: &Request| {
            let body = body(request);
            let evidence = |extra: bool| {
                let mut entries = vec![
                    json!({"requirement": "calc.py has add", "proof": "python3 test_calc.py: ok"}),
                ];
                if extra {
                    entries.push(
                        json!({"requirement": "also sub", "proof": "python3 test_calc.py: ok"}),
                    );
                }
                json!({"status": "complete", "evidence": entries})
            };
            if called(&body, "update_goal") && mentions(&body, "Also asked: Also add sub") {
                return reply("Done: add and sub both pass.");
            }
            if mentions(&body, "Also asked: Also add sub") {
                return calls("update_goal", evidence(true));
            }
            if called(&body, "update_goal") {
                return reply("Reading the new request next.");
            }
            // The first goal turn takes a moment, so the person's message lands meanwhile.
            calls("update_goal", evidence(false)).set_delay(std::time::Duration::from_millis(800))
        })
        .mount(&server)
        .await;
    let (state, _data, _project, thread) = chat(&server).await;
    let (sink, mut rx) = ChannelSink::channel();
    let sink: Arc<dyn themis_desktop::sink::EventSink> = Arc::new(sink);
    state
        .send_message(sink.clone(), thread.clone(), "/goal calc.py has add".into())
        .await
        .unwrap();
    tokio::time::sleep(std::time::Duration::from_millis(200)).await;
    state
        .enqueue_message(
            sink.clone(),
            thread.clone(),
            "Also add sub".into(),
            None,
            Vec::new(),
        )
        .await
        .unwrap();
    goal_until(&state, &thread, |status| status == GoalStatus::Reached).await;
    let mut seen = Vec::new();
    until(&mut rx, &mut seen, |event| {
        matches!(event, ThreadEvent::Finished { .. })
    })
    .await;
    settle(&state, &thread).await;
    let refusals: Vec<&String> = seen
        .iter()
        .filter_map(|event| match event {
            ThreadEvent::ToolFinished {
                tool,
                ok: false,
                output,
                ..
            } if tool == "update_goal" => Some(output),
            _ => None,
        })
        .collect();
    assert_eq!(refusals.len(), 1, "{refusals:?}");
    assert!(refusals[0].contains("not read yet"), "{}", refusals[0]);
    let goal = state.goal_for(&thread).unwrap();
    assert_eq!(
        goal.evidence.len(),
        2,
        "completed only after the new request: {goal:?}"
    );
}

/// `update_goal` blocked (after the same blocker held for three goal turns) ends the goal's
/// turns with the reason shown; resuming it starts again with a fresh blocked count.
#[tokio::test(flavor = "multi_thread")]
async fn blocked_ends_the_goal_until_the_person_resumes_it() {
    let server = MockServer::builder().start().await;
    Mock::given(method("POST"))
        .respond_with(|request: &Request| {
            let body = body(request);
            let after_tool = body["messages"]
                .as_array()
                .and_then(|messages| messages.last())
                .is_some_and(|message| message["role"] == "tool");
            if mentions(&body, "Count any blocker afresh") {
                return if after_tool {
                    reply("The key arrived; done.")
                } else {
                    calls(
                        "update_goal",
                        json!({"status": "complete", "evidence": [{"requirement": "a test payment", "proof": "sandbox: payment ok"}]}),
                    )
                };
            }
            if called(&body, "update_goal") {
                return reply("I'm blocked: the sandbox needs an API key.");
            }
            // Turns 1 and 2 hit the blocker; turn 3 marks the goal blocked.
            if mentions(&body, "Turn 2.") {
                calls(
                    "update_goal",
                    json!({"status": "blocked", "reason": "The payment sandbox needs an API key only you can create."}),
                )
            } else {
                reply("The payment sandbox refused: no API key.")
            }
        })
        .mount(&server)
        .await;
    let (state, _data, _project, thread) = chat(&server).await;
    let (sink, _rx) = ChannelSink::channel();
    let sink: Arc<dyn themis_desktop::sink::EventSink> = Arc::new(sink);
    state
        .send_message(
            sink.clone(),
            thread.clone(),
            "/goal take a test payment".into(),
        )
        .await
        .unwrap();
    goal_until(&state, &thread, |status| status == GoalStatus::Blocked).await;
    settle(&state, &thread).await;
    let goal = state.goal_for(&thread).unwrap();
    assert_eq!(goal.turns, 2, "{goal:?}");
    assert_eq!(
        goal.reason.as_deref(),
        Some("The payment sandbox needs an API key only you can create.")
    );
    let requests = server.received_requests().await.unwrap().len();
    tokio::time::sleep(Duration::from_millis(800)).await;
    assert_eq!(
        server.received_requests().await.unwrap().len(),
        requests,
        "no turn starts while blocked"
    );
    state.resume_goal(sink, thread.clone()).await.unwrap();
    goal_until(&state, &thread, |status| status == GoalStatus::Reached).await;
    settle(&state, &thread).await;
}

/// A failed turn waits with a countdown the chat can show, and leaves no note behind that
/// would go out of date.
#[tokio::test(flavor = "multi_thread")]
async fn a_failed_turn_counts_down_instead_of_leaving_a_note() {
    let server = MockServer::builder().start().await;
    Mock::given(method("POST"))
        .respond_with(|_: &Request| ResponseTemplate::new(400).set_body_string("bad request"))
        .mount(&server)
        .await;
    let (state, _data, _project, thread) = chat(&server).await;
    let (sink, mut rx) = ChannelSink::channel();
    let sink = Arc::new(sink);
    state
        .send_message(
            sink.clone(),
            thread.clone(),
            "/goal finish the thing".into(),
        )
        .await
        .unwrap();
    for _ in 0..400 {
        if state
            .goal_for(&thread)
            .is_some_and(|goal| goal.retry_at.is_some())
        {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(25)).await;
    }
    let goal = state.goal_for(&thread).unwrap();
    assert!(goal.retry_at.is_some(), "{goal:?}");
    let mut notes = Vec::new();
    while let Ok(event) = rx.try_recv() {
        if let TestEvent::Thread(envelope) = event {
            if let ThreadEvent::GoalNote { reason, .. } = envelope.event {
                notes.push(reason);
            }
        }
    }
    assert!(
        !notes.iter().any(|note| note.contains("try again")),
        "{notes:?}"
    );
    state
        .remove_goal(sink.clone(), thread.clone())
        .await
        .unwrap();
}

/// A goal keeps going until it is reached: the agent can't talk itself out of a
/// hard one by pausing it.
#[tokio::test(flavor = "multi_thread")]
async fn the_agent_cannot_pause_a_goal_nobody_asked_to_stop() {
    let server = MockServer::builder().start().await;
    Mock::given(method("POST"))
        .respond_with(|request: &Request| {
            let body = body(request);
            let sent = body["messages"].to_string();
            if sent.contains("\"complete\"") {
                reply("Carrying on, and it's done.")
            } else if sent.contains("pausing is refused") {
                calls(
                    "update_goal",
                    json!({"status": "complete", "evidence": [{"requirement": "the thing", "proof": "it runs"}]}),
                )
            } else {
                calls("update_goal", json!({"status": "paused"}))
            }
        })
        .mount(&server)
        .await;
    let (state, _data, _project, thread) = chat(&server).await;
    let (sink, mut rx) = ChannelSink::channel();
    state
        .send_message(
            Arc::new(sink),
            thread.clone(),
            "/goal finish the thing".into(),
        )
        .await
        .unwrap();
    goal_until(&state, &thread, |status| status == GoalStatus::Reached).await;
    let mut seen = Vec::new();
    until(
        &mut rx,
        &mut seen,
        |event| matches!(event, ThreadEvent::ToolFinished { ok: false, output, .. } if output.contains("pausing is refused")),
    )
    .await;
    settle(&state, &thread).await;
}

/// Review findings: a queued message refused before any run starts (a bare
/// `/goal`) stays in the queue, held, with its reason in the chat, and the
/// queue moves on; a steer with no run to receive it is refused and the message
/// stays queued.
#[tokio::test(flavor = "multi_thread")]
async fn unsendable_queued_messages_are_recorded_and_the_queue_moves_on() {
    let server = MockServer::builder().start().await;
    let gate = Gate::default();
    let held_answer = gate.clone();
    Mock::given(method("POST"))
        .respond_with(move |request: &Request| {
            if mentions(&body(request), "first") && !mentions(&body(request), "after the bad one") {
                held_answer.wait();
                reply("First done.")
            } else {
                reply("Later done.")
            }
        })
        .mount(&server)
        .await;
    let (state, _data, _project, thread) = chat(&server).await;
    let (sink, mut rx) = ChannelSink::channel();
    let sink: Arc<dyn themis_desktop::sink::EventSink> = Arc::new(sink);
    state
        .send_message(sink.clone(), thread.clone(), "first".into())
        .await
        .unwrap();
    state
        .enqueue_message(sink.clone(), thread.clone(), "/goal".into(), None, vec![])
        .await
        .unwrap();
    let held = state
        .enqueue_message(
            sink.clone(),
            thread.clone(),
            "after the bad one".into(),
            None,
            vec![],
        )
        .await
        .unwrap();
    // Held while "edited": it waits until saved.
    state
        .update_queued_message(
            sink.clone(),
            thread.clone(),
            held.id.clone(),
            None,
            Some(true),
        )
        .await
        .unwrap();
    gate.open();
    let mut seen = Vec::new();
    until(&mut rx, &mut seen, |event| {
        matches!(event, ThreadEvent::Failed { error } if error.starts_with("Queued message not sent"))
    })
    .await;
    // The held message is still waiting; with no run, it cannot be steered.
    tokio::time::sleep(Duration::from_millis(300)).await;
    let refused = state
        .steer_queued_message(sink.clone(), thread.clone(), held.id.clone())
        .await
        .unwrap_err();
    assert!(refused.contains("No run is going"), "{refused}");
    let waiting = state.list_queue(thread.clone()).await.unwrap();
    assert_eq!(
        waiting
            .iter()
            .map(|m| (m.text.as_str(), m.held))
            .collect::<Vec<_>>(),
        [("/goal", true), ("after the bad one", true)],
        "nothing is lost: the unsendable message waits, held, for an edit"
    );
    assert!(
        waiting[0]
            .note
            .as_deref()
            .is_some_and(|note| note.contains("Add the goal after /goal")),
        "the held message says why: {:?}",
        waiting[0].note
    );
    // Saving the edit releases it, and the idle chat sends it.
    state
        .update_queued_message(sink, thread.clone(), held.id, None, Some(false))
        .await
        .unwrap();
    until(
        &mut rx,
        &mut seen,
        |event| matches!(event, ThreadEvent::Finished { result, .. } if result == "Later done."),
    )
    .await;
    let left = state.list_queue(thread.clone()).await.unwrap();
    assert_eq!(
        left.iter().map(|m| m.text.as_str()).collect::<Vec<_>>(),
        ["/goal"]
    );
    let items = serde_json::to_value(state.get_thread_history(&thread).await.unwrap()).unwrap();
    assert!(items.to_string().contains("Queued message not sent"));
    // The message that ran was saved once as the user's.
    let users = items
        .as_array()
        .unwrap()
        .iter()
        .filter(|item| item["kind"] == "user" && item["text"] == "after the bad one")
        .count();
    assert_eq!(users, 1);
    settle(&state, &thread).await;
}

/// A model that keeps refusing pauses the goal instead of retrying for ever, says why in its
/// status, and adds no note to the chat. Takes the two retry waits (30 s and 60 s, jittered).
#[tokio::test(flavor = "multi_thread")]
async fn a_goal_pauses_after_its_turns_keep_failing() {
    let server = MockServer::builder().start().await;
    Mock::given(method("POST"))
        .respond_with(ResponseTemplate::new(400).set_body_string("model broken"))
        .mount(&server)
        .await;
    let data = tempfile::tempdir().unwrap();
    let project = tempfile::tempdir().unwrap();
    let state = AppState::new_for_test(data.path().join("settings.json"));
    state.set_go_base_url_override(Some(server.uri()));
    state
        .set_secret("go".into(), "synthetic-test-key".into())
        .await
        .unwrap();
    let thread = state
        .create_thread(
            project.path().to_string_lossy().into_owned(),
            ProviderKind::Go,
            Some("test-model".into()),
        )
        .await
        .unwrap()
        .id;
    let (sink, mut rx) = ChannelSink::channel();
    let sink: Arc<dyn themis_desktop::sink::EventSink> = Arc::new(sink);
    state
        .send_message(sink.clone(), thread.clone(), "/goal finish the work".into())
        .await
        .unwrap();
    let paused = tokio::time::timeout(Duration::from_secs(240), async {
        loop {
            while rx.try_recv().is_ok() {}
            if state
                .goal_for(&thread)
                .is_some_and(|goal| goal.status == GoalStatus::Paused)
            {
                return;
            }
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
    })
    .await;
    let goal = state.goal_for(&thread).unwrap();
    assert!(paused.is_ok(), "{goal:?}");
    assert_eq!(goal.reason.as_deref(), Some("the model kept failing"));
    assert_eq!(goal.failures, 0, "a resume starts counting again");
}

/// Review findings: a `/goal` whose run cannot start still continues: it retries as a failed
/// turn instead of staying "running" with no turn.
#[tokio::test(flavor = "multi_thread")]
async fn a_goal_whose_turn_cannot_start_retries() {
    let server = MockServer::builder().start().await;
    Mock::given(method("POST"))
        .respond_with(move |request: &Request| completes(&body(request), "Worked on it."))
        .mount(&server)
        .await;
    let data = tempfile::tempdir().unwrap();
    let project = tempfile::tempdir().unwrap();
    let state = AppState::new_for_test(data.path().join("settings.json"));
    state.set_go_base_url_override(Some(server.uri()));
    let thread = state
        .create_thread(
            project.path().to_string_lossy().into_owned(),
            ProviderKind::Go,
            Some("test-model".into()),
        )
        .await
        .unwrap()
        .id;
    let (sink, _rx) = ChannelSink::channel();
    let sink: Arc<dyn themis_desktop::sink::EventSink> = Arc::new(sink);
    // No key yet: the goal's first run fails to start.
    let error = state
        .send_message(sink.clone(), thread.clone(), "/goal finish the work".into())
        .await
        .unwrap_err();
    assert!(!error.is_empty());
    // Counted as a failed turn and retried after a wait, with no note in the chat.
    let counted = tokio::time::timeout(DEADLINE, async {
        while state
            .goal_for(&thread)
            .is_none_or(|goal| goal.failures != 1)
        {
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
    })
    .await;
    let goal = state.goal_for(&thread).unwrap();
    assert!(counted.is_ok(), "{goal:?}");
    assert_eq!(goal.status, GoalStatus::Running);
    // With the key in place, a resume starts the next turn now.
    state
        .set_secret("go".into(), "synthetic-test-key".into())
        .await
        .unwrap();
    state
        .pause_goal(sink.clone(), thread.clone())
        .await
        .unwrap();
    state
        .resume_goal(sink.clone(), thread.clone())
        .await
        .unwrap();
    goal_until(&state, &thread, |status| status == GoalStatus::Reached).await;
    settle(&state, &thread).await;
}

/// Third review: an error that belongs to the chat (its project's
/// configuration became invalid) holds the first message with its reason and
/// leaves the rest waiting in order, with one note instead of one per message.
#[tokio::test(flavor = "multi_thread")]
async fn a_chat_wide_error_holds_one_message_and_keeps_the_rest_in_order() {
    let server = MockServer::builder().start().await;
    let gate = Gate::default();
    let held = gate.clone();
    Mock::given(method("POST"))
        .respond_with(move |_: &Request| {
            held.wait();
            reply("Done.")
        })
        .mount(&server)
        .await;
    let (state, _data, project, thread) = chat(&server).await;
    let (sink, mut rx) = ChannelSink::channel();
    let sink: Arc<dyn themis_desktop::sink::EventSink> = Arc::new(sink);
    state
        .send_message(sink.clone(), thread.clone(), "first".into())
        .await
        .unwrap();
    for text in ["one", "two", "three"] {
        state
            .enqueue_message(sink.clone(), thread.clone(), text.into(), None, vec![])
            .await
            .unwrap();
    }
    // While the run goes, the project's configuration breaks.
    std::fs::create_dir_all(project.path().join(".themis")).unwrap();
    std::fs::write(project.path().join(".themis/config.jsonc"), "{ not json").unwrap();
    gate.open();
    let mut seen = Vec::new();
    until(&mut rx, &mut seen, |event| {
        matches!(event, ThreadEvent::Failed { error } if error.starts_with("Queued message not sent"))
    })
    .await;
    // Wait until the refused second message is back (it is out of the queue
    // while its send is attempted).
    let queue = tokio::time::timeout(DEADLINE, async {
        loop {
            let queue = state.list_queue(thread.clone()).await.unwrap();
            if queue.len() == 3 {
                return queue;
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
    })
    .await
    .expect("the queue settles");
    assert_eq!(
        queue
            .iter()
            .map(|m| (m.text.as_str(), m.held, m.note.is_some()))
            .collect::<Vec<_>>(),
        [
            ("one", true, true),
            ("two", false, false),
            ("three", false, false)
        ]
    );
    let items = serde_json::to_value(state.get_thread_history(&thread).await.unwrap()).unwrap();
    let notes = items
        .as_array()
        .unwrap()
        .iter()
        .filter(|item| item.to_string().contains("Queued message not sent"))
        .count();
    assert_eq!(notes, 1, "one note for one cause");
    settle(&state, &thread).await;
}

/// A goal whose context window is small says so once, on its first turn,
/// and still runs to the end; a later turn does not repeat the note.
#[tokio::test(flavor = "multi_thread")]
async fn a_goal_with_a_small_context_window_warns_once() {
    let server = MockServer::builder().start().await;
    Mock::given(method("POST"))
        .respond_with(move |request: &Request| {
            let body = body(request);
            // The /goal run takes the first step; the next goal turn the second, and ends it.
            if mentions(&body, "[Goal — runtime") {
                return completes(&body, "Both steps are done, so the goal is reached.");
            }
            reply("Did a step.")
        })
        .mount(&server)
        .await;
    let (state, data, _project, thread) = chat(&server).await;
    // The test endpoint reports no window, so the budget is the window. Settings offer 200,000
    // up; a small budget comes from runtime.jsonc, as in evals.
    std::fs::write(
        data.path().join("runtime.jsonc"),
        r#"{"schema_version":1,"context_token_budget":64000,"goal_qa_turns":false}"#,
    )
    .unwrap();
    let (sink, mut rx) = ChannelSink::channel();
    let sink = Arc::new(sink);
    state
        .send_message(sink.clone(), thread.clone(), "/goal take two steps".into())
        .await
        .unwrap();
    let mut seen = Vec::new();
    until(
        &mut rx,
        &mut seen,
        |event| matches!(event, ThreadEvent::Finished { result, .. } if result == "Both steps are done, so the goal is reached."),
    )
    .await;
    let warnings: Vec<_> = seen
        .iter()
        .filter_map(|event| match event {
            ThreadEvent::GoalNote {
                outcome: GoalNoteKind::SmallWindow,
                reason,
            } => Some(reason.as_str()),
            _ => None,
        })
        .collect();
    assert_eq!(
        warnings,
        ["Context window is small (64,000 tokens); long goals may stall."]
    );
    assert_eq!(state.goal_for(&thread).unwrap().turns, 2);
    settle(&state, &thread).await;
}

/// The wire name of an event, the name clients and saved history see.
fn event_kind(event: &ThreadEvent) -> String {
    serde_json::to_value(event).unwrap()["kind"]
        .as_str()
        .unwrap()
        .to_owned()
}

/// Every event kind in the chat's saved history, in order.
async fn saved_kinds(state: &AppState, thread: &str) -> Vec<String> {
    let history = serde_json::to_value(state.get_thread_history(thread).await.unwrap()).unwrap();
    history
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|item| match item["kind"].as_str() {
            Some("event") => item["envelope"]["event"]["kind"].as_str().map(Into::into),
            Some("user") => Some("user".to_owned()),
            _ => None,
        })
        .collect()
}

fn position(kinds: &[String], kind: &str) -> usize {
    kinds
        .iter()
        .position(|seen| seen == kind)
        .unwrap_or_else(|| panic!("no {kind} in {kinds:?}"))
}

fn two_dirs() -> ResponseTemplate {
    let call = |id: &str, dir: &str| {
        json!({"id": id, "type": "function", "function": {"name": "create_dir",
            "arguments": json!({"directory_path": dir}).to_string()}})
    };
    ResponseTemplate::new(200).set_body_json(json!({"choices":[{"message":{"role":"assistant",
        "content":null,"tool_calls":[call("a", "dir_a"), call("b", "dir_b")]},"finish_reason":"tool_calls"}]}))
}

/// A chat in Custom mode inside a Git project, where writes ask first.
async fn asking_chat(
    server: &MockServer,
) -> (AppState, tempfile::TempDir, tempfile::TempDir, String) {
    let (state, data, project, _) = chat(server).await;
    let git = std::process::Command::new("git")
        .args(["init", "-q"])
        .current_dir(project.path())
        .status()
        .unwrap();
    assert!(git.success());
    // A chat notes whether its project uses Git when it is made.
    let thread = state
        .create_thread(
            project.path().to_string_lossy().into_owned(),
            ProviderKind::Go,
            Some("test-model".into()),
        )
        .await
        .unwrap()
        .id;
    state
        .set_thread_approval_mode(
            thread.clone(),
            themis_core::configuration::ApprovalMode::Custom,
        )
        .await
        .unwrap();
    (state, data, project, thread)
}

/// Waits for the next approval box, keeping the thread events that pass by.
async fn next_approval(
    rx: &mut UnboundedReceiver<TestEvent>,
    seen: &mut Vec<ThreadEvent>,
) -> themis_desktop::types::ApprovalRequest {
    let waited = tokio::time::timeout(Duration::from_secs(60), async {
        loop {
            match rx.recv().await.expect("event stream ended") {
                TestEvent::Approval(request) => return request,
                TestEvent::Thread(envelope) => seen.push(envelope.event),
                _ => {}
            }
        }
    })
    .await;
    match waited {
        Ok(request) => request,
        Err(_) => panic!(
            "no approval asked; events: {:?}",
            seen.iter().map(event_kind).collect::<Vec<_>>()
        ),
    }
}

fn after_a_tool(body: &Value) -> bool {
    body["messages"]
        .as_array()
        .and_then(|messages| messages.last())
        .is_some_and(|message| message["role"] == "tool")
}

/// Found in a live goal run: a request steered in while an approval box waited stayed out of
/// sight and out of the agent's reach until the person answered the box.
#[tokio::test(flavor = "multi_thread")]
async fn steering_past_an_approval_shows_at_once_denies_it_and_reaches_the_agent() {
    const STEER: &str = "Please remove the goal";
    let server = MockServer::builder().start().await;
    Mock::given(method("POST"))
        .respond_with(move |request: &Request| {
            let body = body(request);
            if mentions(&body, "try again") {
                if after_a_tool(&body) {
                    reply("Made it.")
                } else {
                    calls("create_dir", json!({"directory_path": "dir_c"}))
                }
            } else if mentions(&body, STEER) {
                reply("Understood.")
            } else {
                two_dirs()
            }
        })
        .mount(&server)
        .await;
    let (state, data, project, thread) = asking_chat(&server).await;
    let (sink, mut rx) = ChannelSink::channel();
    let sink = Arc::new(sink);
    state
        .send_message(sink.clone(), thread.clone(), "make two dirs".into())
        .await
        .unwrap();
    let mut seen = Vec::new();
    next_approval(&mut rx, &mut seen).await;

    // Plain queueing leaves the box waiting.
    let plain = state
        .enqueue_message(
            sink.clone(),
            thread.clone(),
            "for later".into(),
            None,
            vec![],
        )
        .await
        .unwrap();
    tokio::time::sleep(Duration::from_millis(300)).await;
    assert!(!state.pending_approvals().is_empty());
    state
        .remove_queued_message(sink.clone(), thread.clone(), plain.id)
        .await
        .unwrap();

    let steer = state
        .enqueue_message(sink.clone(), thread.clone(), STEER.into(), None, vec![])
        .await
        .unwrap();
    state
        .steer_queued_message(sink.clone(), thread.clone(), steer.id)
        .await
        .unwrap();

    // On screen as soon as it is steered, and no box is left, with no "expired" warning.
    let mut quiet_boxes = 0;
    let mut finished = false;
    let drained = tokio::time::timeout(Duration::from_secs(20), async {
        while !finished {
            match rx.recv().await.expect("event stream ended") {
                TestEvent::Thread(envelope) => {
                    finished = matches!(envelope.event, ThreadEvent::Finished { .. });
                    seen.push(envelope.event);
                }
                TestEvent::Expired(expired) => {
                    assert_eq!(serde_json::to_value(&expired).unwrap()["steered"], true);
                    quiet_boxes += 1;
                }
                TestEvent::Approval(request) => {
                    // Calls running together may both have asked; a later ask must not.
                    assert!(
                        !seen.iter().any(|event| event_kind(event) == "steer_shown"),
                        "asked again before the steer was read: {request:?}"
                    );
                }
                _ => {}
            }
        }
    })
    .await;
    assert!(
        drained.is_ok(),
        "the run stayed blocked; events: {:?}; boxes: {:?}",
        seen.iter().map(event_kind).collect::<Vec<_>>(),
        state.pending_approvals().len()
    );
    assert!(quiet_boxes >= 1, "the box was not taken away");
    assert!(state.pending_approvals().is_empty());
    assert!(!project.path().join("dir_a").exists());
    assert!(!project.path().join("dir_b").exists());

    let kinds: Vec<String> = seen.iter().map(event_kind).collect();
    let count = |kind: &str| kinds.iter().filter(|seen| *seen == kind).count();
    assert_eq!(count("steer_shown"), 1, "{kinds:?}");
    assert_eq!(count("steered"), 1, "{kinds:?}");
    assert!(
        position(&kinds, "steer_shown") < position(&kinds, "steered"),
        "{kinds:?}"
    );

    // Each denied call tells the agent why, and the message comes next to read.
    let requests = server.received_requests().await.unwrap();
    let last = body(requests.last().unwrap());
    let messages = last["messages"].as_array().unwrap();
    let denied: Vec<&Value> = messages.iter().filter(|m| m["role"] == "tool").collect();
    assert_eq!(denied.len(), 2, "{last}");
    for result in &denied {
        let text = result["content"].to_string();
        assert!(text.contains("approval denied"), "{text}");
        assert!(text.contains("sent a new message"), "{text}");
        assert!(!text.contains(themis_core::tools::REFUSAL_NOTE), "{text}");
    }
    let tool_at = messages.iter().rposition(|m| m["role"] == "tool").unwrap();
    let steer_at = messages
        .iter()
        .position(|m| {
            m["content"]
                .as_str()
                .is_some_and(|text| text.contains(STEER))
        })
        .unwrap();
    assert!(tool_at < steer_at, "{last}");

    // The history keeps what the person saw, in order.
    let saved = saved_kinds(&state, &thread).await;
    assert!(
        position(&saved, "steer_shown") < position(&saved, "steered"),
        "{saved:?}"
    );
    assert!(
        position(&saved, "tool_finished") < position(&saved, "steered"),
        "{saved:?}"
    );
    assert!(
        position(&saved, "steered") < position(&saved, "finished"),
        "{saved:?}"
    );

    // Nothing was remembered: the same kind of action asks again, and can then run.
    assert!(!data
        .path()
        .join("chat_grants")
        .join(format!("{thread}.json"))
        .exists());
    state
        .send_message(sink.clone(), thread.clone(), "try again".into())
        .await
        .unwrap();
    let again = next_approval(&mut rx, &mut seen).await;
    state
        .approve_action(
            thread.clone(),
            again.approval_id,
            themis_desktop::types::ApprovalDecision::Once,
        )
        .await
        .unwrap();
    until(
        &mut rx,
        &mut seen,
        |event| matches!(event, ThreadEvent::Finished { result, .. } if result == "Made it."),
    )
    .await;
    assert!(project.path().join("dir_c").is_dir());
    // The model has the steered message once, not once per record of it.
    let requests = server.received_requests().await.unwrap();
    let last = body(requests.last().unwrap());
    let copies = last["messages"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|m| {
            m["content"]
                .as_str()
                .is_some_and(|text| text.contains(STEER))
        })
        .count();
    assert_eq!(copies, 1, "{last}");
    settle(&state, &thread).await;
}

/// A steer the run never read is taken back out of the chat, so it shows once when it
/// goes out as the next message.
#[tokio::test(flavor = "multi_thread")]
async fn a_steer_the_run_never_read_leaves_the_chat_and_comes_back_once() {
    const STEER: &str = "Use tabs instead";
    let server = MockServer::builder().start().await;
    let gate = Gate::default();
    let held = gate.clone();
    Mock::given(method("POST"))
        .respond_with(move |request: &Request| {
            if mentions(&body(request), STEER) {
                reply("Tabs it is.")
            } else {
                held.wait();
                reply("First done.")
            }
        })
        .mount(&server)
        .await;
    let (state, _data, _project, thread) = chat(&server).await;
    let (sink, mut rx) = ChannelSink::channel();
    let sink = Arc::new(sink);
    state
        .send_message(sink.clone(), thread.clone(), "first".into())
        .await
        .unwrap();
    let steer = state
        .enqueue_message(sink.clone(), thread.clone(), STEER.into(), None, vec![])
        .await
        .unwrap();
    state
        .steer_queued_message(sink.clone(), thread.clone(), steer.id)
        .await
        .unwrap();
    state.stop_thread(thread.clone()).await.unwrap();
    gate.open();
    let mut seen = Vec::new();
    until(
        &mut rx,
        &mut seen,
        |event| matches!(event, ThreadEvent::Finished { result, .. } if result == "Tabs it is."),
    )
    .await;
    let kinds: Vec<String> = seen.iter().map(event_kind).collect();
    assert!(!kinds.iter().any(|kind| kind == "steered"), "{kinds:?}");
    let shown = position(&kinds, "steer_shown");
    let returned = position(&kinds, "steer_returned");
    assert!(shown < returned, "{kinds:?}");
    assert!(seen[returned..]
        .iter()
        .any(|event| matches!(event, ThreadEvent::UserMessage { text, .. } if text == STEER)));
    let saved = saved_kinds(&state, &thread).await;
    assert!(
        position(&saved, "steer_shown") < position(&saved, "steer_returned"),
        "{saved:?}"
    );
    settle(&state, &thread).await;
}

/// An answer to a question also travels through the run's inbox, but it is not a steer:
/// the box stays until the person decides.
#[tokio::test(flavor = "multi_thread")]
async fn answering_a_question_leaves_an_approval_waiting() {
    let server = MockServer::builder().start().await;
    Mock::given(method("POST"))
        .respond_with(move |request: &Request| {
            let body = body(request);
            if !after_a_tool(&body) && !mentions(&body, "Answer: Rock") {
                calls(
                    "ask_user",
                    json!({"question": "Which name?", "recommended": "Pebble", "why": "short"}),
                )
            } else if !body["messages"].to_string().contains("dir_a") {
                two_dirs()
            } else {
                reply("Done.")
            }
        })
        .mount(&server)
        .await;
    let (state, _data, project, thread) = asking_chat(&server).await;
    let (sink, mut rx) = ChannelSink::channel();
    let sink = Arc::new(sink);
    state
        .send_message(sink.clone(), thread.clone(), "name it".into())
        .await
        .unwrap();
    let mut seen = Vec::new();
    let first = next_approval(&mut rx, &mut seen).await;
    let question = state
        .list_questions(thread.clone())
        .await
        .unwrap()
        .remove(0);
    state
        .answer_question(sink.clone(), thread.clone(), question.id, "Rock".into())
        .await
        .unwrap();
    tokio::time::sleep(Duration::from_millis(300)).await;
    assert!(
        state
            .pending_approvals()
            .iter()
            .any(|request| request.approval_id == first.approval_id),
        "an answer must not decide an approval"
    );
    state
        .approve_action(
            thread.clone(),
            first.approval_id,
            themis_desktop::types::ApprovalDecision::Once,
        )
        .await
        .unwrap();
    // The next call in the batch still asks: an unread answer does not refuse it.
    let second = next_approval(&mut rx, &mut seen).await;
    state
        .approve_action(
            thread.clone(),
            second.approval_id,
            themis_desktop::types::ApprovalDecision::Once,
        )
        .await
        .unwrap();
    until(&mut rx, &mut seen, |event| {
        matches!(event, ThreadEvent::Finished { .. })
    })
    .await;
    assert!(project.path().join("dir_a").is_dir());
    assert!(project.path().join("dir_b").is_dir());
    settle(&state, &thread).await;
}

/// Found in a live run: "yo" in a chat whose goal was removed sent the agent back to rebuilding
/// everything, because the old goal turns were all it could see. A message after a goal ended
/// or paused says so, and a chat that never had a goal hears nothing about goals.
#[tokio::test(flavor = "multi_thread")]
async fn a_message_after_a_goal_is_told_the_goal_is_not_running() {
    let server = MockServer::builder().start().await;
    Mock::given(method("POST"))
        .respond_with(|_: &Request| reply("Hi."))
        .mount(&server)
        .await;
    let (state, _data, _project, thread) = chat(&server).await;
    let (sink, _rx) = ChannelSink::channel();
    let sink = Arc::new(sink);
    let last_sent = |bodies: Vec<wiremock::Request>| {
        bodies.iter().map(body).next_back().unwrap()["messages"].to_string()
    };
    // A chat that never had a goal hears nothing about one.
    state
        .send_message(sink.clone(), thread.clone(), "hello".into())
        .await
        .unwrap();
    settle(&state, &thread).await;
    let sent = last_sent(server.received_requests().await.unwrap());
    assert!(!sent.contains("No goal is running"), "{sent}");

    state
        .send_message(
            sink.clone(),
            thread.clone(),
            "/goal finish the thing".into(),
        )
        .await
        .unwrap();
    state
        .pause_goal(sink.clone(), thread.clone())
        .await
        .unwrap();
    settle(&state, &thread).await;
    state
        .send_message(sink.clone(), thread.clone(), "yo".into())
        .await
        .unwrap();
    settle(&state, &thread).await;
    let sent = last_sent(server.received_requests().await.unwrap());
    assert!(sent.contains("This chat's goal is paused"), "{sent}");

    state
        .remove_goal(sink.clone(), thread.clone())
        .await
        .unwrap();
    state
        .send_message(sink.clone(), thread.clone(), "yo again".into())
        .await
        .unwrap();
    settle(&state, &thread).await;
    let sent = last_sent(server.received_requests().await.unwrap());
    assert!(sent.contains("No goal is running"), "{sent}");
    assert!(state.goal_for(&thread).is_none());
}
