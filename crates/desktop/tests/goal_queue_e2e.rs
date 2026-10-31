//! Prompt queue, steering and goal mode through the real app state, with a
//! scripted model (wiremock). The goal check is told apart from agent turns by
//! its system prompt.

use std::sync::atomic::{AtomicUsize, Ordering};
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
const CHECK_MARKER: &str = "You decide whether an agent has reached a goal";

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

fn is_checklist(body: &Value) -> bool {
    body["messages"][0]["content"]
        .as_str()
        .is_some_and(|text| text.contains("what a goal asks for"))
}

fn is_check(body: &Value) -> bool {
    body["messages"][0]["content"]
        .as_str()
        .is_some_and(|text| text.contains(CHECK_MARKER))
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
    // A goal check that started before the pause finishes against this server.
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
    let server = MockServer::start().await;
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
async fn a_goal_runs_turn_after_turn_until_the_check_finds_it_reached() {
    let server = MockServer::start().await;
    let checks = Arc::new(AtomicUsize::new(0));
    let seen_checks = Arc::clone(&checks);
    Mock::given(method("POST"))
        .respond_with(move |request: &Request| {
            let body = body(request);
            if is_check(&body) {
                return if seen_checks.fetch_add(1, Ordering::SeqCst) == 0 {
                    reply(r#"{"verdict":"not_yet","reason":"Counter is 1; needs 2.","progress":"counter 1"}"#)
                } else {
                    reply(r#"{"verdict":"reached","reason":"Counter is 2.","progress":"counter 2"}"#)
                };
            }
            if mentions(&body, "A separate check found the goal reached") {
                return reply("I've reached the goal: the counter is 2.");
            }
            reply("Incremented the counter.")
        })
        .mount(&server)
        .await;
    let (state, _data, _project, thread) = chat(&server).await;
    let (sink, mut rx) = ChannelSink::channel();
    let sink = Arc::new(sink);
    let empty = state
        .send_message(sink.clone(), thread.clone(), "/goal".into())
        .await
        .unwrap_err();
    assert!(empty.contains("Add the goal after /goal"), "{empty}");
    state
        .send_message(
            sink.clone(),
            thread.clone(),
            "/goal  make the counter reach 2".into(),
        )
        .await
        .unwrap();
    assert_eq!(
        state.goal_for(&thread).map(|goal| goal.text),
        Some("make the counter reach 2".into())
    );
    goal_until(&state, &thread, |status| status == GoalStatus::Reached).await;
    let goal = state.goal_for(&thread).unwrap();
    assert_eq!(goal.turns, 2);
    assert_eq!(goal.reason.as_deref(), Some("Counter is 2."));
    assert!(goal.running_since.is_none());
    // The agent says it in its own words; the check posts no note of its own.
    let mut seen = Vec::new();
    until(
        &mut rx,
        &mut seen,
        |event| matches!(event, ThreadEvent::Finished { result, .. } if result == "I've reached the goal: the counter is 2."),
    )
    .await;
    // The verdict is recorded, for the chat to dim the turn; the agent voices it next turn.
    assert!(seen.iter().any(|event| matches!(
        event,
        ThreadEvent::GoalNote { outcome: GoalNoteKind::NotYet, reason, .. }
            if reason == "Counter is 1; needs 2."
    )));
    // The chat carries no announcements of its own around a check.
    assert!(
        !seen.iter().any(|event| matches!(
            event,
            ThreadEvent::GoalNote { outcome: GoalNoteKind::Status, reason, .. }
                if reason.contains("checking") || reason.contains("starting")
        )),
        "{seen:?}"
    );
    // While the check runs, the goal says so, for a passing status line.
    let checking: Vec<bool> = seen
        .iter()
        .filter_map(|event| match event {
            ThreadEvent::GoalChanged { goal: Some(goal) } => Some(goal.checking),
            _ => None,
        })
        .collect();
    assert!(
        checking.windows(2).any(|pair| pair == [true, false]),
        "{checking:?}"
    );
    assert!(!state.goal_for(&thread).unwrap().checking);
    // The second turn was started by the runtime, carrying the check's reason.
    let bodies: Vec<Value> = distinct(
        server
            .received_requests()
            .await
            .unwrap()
            .iter()
            .map(body)
            .collect(),
    );
    let turns: Vec<&Value> = bodies
        .iter()
        .filter(|body| !is_check(body) && !is_checklist(body))
        .collect();
    assert_eq!(turns.len(), 3);
    assert_eq!(
        checks.load(Ordering::SeqCst),
        2,
        "no check after the reached turn"
    );
    assert!(mentions(turns[0], "Active goal for this chat"));
    assert!(mentions(
        turns[1],
        "[Goal — runtime, not from the user] Turn 2."
    ));
    assert!(mentions(turns[1], "Last check: Counter is 1; needs 2."));
    assert!(mentions(turns[1], "what the last check found"));
    // Then one short turn, unchecked, tells the user the goal is reached.
    assert!(mentions(
        turns[2],
        "[Goal — runtime, not from the user] Turn 3."
    ));
    assert!(mentions(
        turns[2],
        "A separate check found the goal reached: Counter is 2."
    ));
    assert_eq!(state.goal_for(&thread).unwrap().turns, 2);
    // The check sees the goal and the answer as fenced data.
    let check = bodies.iter().find(|body| is_check(body)).unwrap();
    assert!(mentions(check, "make the counter reach 2"));
    assert!(mentions(check, "Incremented the counter."));
    // A new message after a reached goal clears it.
    state
        .send_message(sink, thread.clone(), "thanks".into())
        .await
        .unwrap();
    assert!(state.goal_for(&thread).is_none());
    settle(&state, &thread).await;
}

#[tokio::test(flavor = "multi_thread")]
async fn turns_that_only_re_verify_stall_and_the_goal_changes_course_instead_of_stopping() {
    // A live goal looped for ten turns: each re-ran a few checks, the check
    // said "not shown this turn" and phrased its measured fact differently,
    // which counted as progress. The check's own progress judgement decides.
    // A goal never gives up: after three such turns the next one is told to
    // try a different approach, and the goal keeps running.
    let server = MockServer::start().await;
    let checks = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let counted = Arc::clone(&checks);
    Mock::given(method("POST"))
        .respond_with(move |request: &Request| {
            if is_check(&body(request)) {
                let n = counted.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                return reply(&format!(
                    r#"{{"verdict":"not_yet","reason":"Not all requirements shown this turn.","progress":"re-check {n} of the same 34 tests","progressed":{}}}"#,
                    n == 0
                ));
            }
            reply("All requirements still hold; verified in earlier turns.")
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
            "/goal pass every check".into(),
        )
        .await
        .unwrap();
    let changed_course = tokio::time::timeout(DEADLINE, async {
        loop {
            let requests = server.received_requests().await.unwrap();
            if requests
                .iter()
                .map(body)
                .any(|body| !is_check(&body) && mentions(&body, "try a different approach"))
            {
                return;
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
    })
    .await;
    assert!(
        changed_course.is_ok(),
        "goal: {:?}",
        state.goal_for(&thread)
    );
    let goal = state.goal_for(&thread).unwrap();
    assert_eq!(goal.status, GoalStatus::Running);
    assert!(goal.stalls >= 3, "stalls {}", goal.stalls);
    assert!(checks.load(std::sync::atomic::Ordering::SeqCst) >= 4);
    settle(&state, &thread).await;
}

/// Found in the goal soak: a turn that rewrote 172 files was judged "no
/// progress" by a check that looked only for proof of completion. A turn
/// that changed files is never a stall, whatever the check says.
#[tokio::test(flavor = "multi_thread")]
async fn a_turn_that_changed_files_is_never_a_stall() {
    let server = MockServer::start().await;
    let checks = Arc::new(AtomicUsize::new(0));
    let counted = Arc::clone(&checks);
    let writes = Arc::new(AtomicUsize::new(0));
    Mock::given(method("POST"))
        .respond_with(move |request: &Request| {
            let body = body(request);
            if is_check(&body) {
                counted.fetch_add(1, Ordering::SeqCst);
                return reply(
                    r#"{"verdict":"not_yet","reason":"No passing check shown.","progressed":false}"#,
                );
            }
            let tool_done = body["messages"]
                .as_array()
                .and_then(|messages| messages.last())
                .is_some_and(|message| message["role"] == "tool");
            if tool_done {
                return reply("Wrote the next file.");
            }
            let n = writes.fetch_add(1, Ordering::SeqCst);
            let arguments = json!({"file_path": format!("out/{n}.md"), "content": format!("item {n}")});
            ResponseTemplate::new(200).set_body_json(json!({"choices":[{"message":{"role":"assistant","content":null,"tool_calls":[{"id":format!("w{n}"),"type":"function","function":{"name":"write_file","arguments":arguments.to_string()}}]},"finish_reason":"tool_calls"}]}))
        })
        .mount(&server)
        .await;
    let (state, _data, project, thread) = chat(&server).await;
    // Changed files are read from Git, as in a real project.
    let git = std::process::Command::new("git")
        .args(["init", "-q"])
        .current_dir(project.path())
        .status()
        .unwrap();
    assert!(git.success());
    state
        .set_thread_approval_mode(
            thread.clone(),
            themis_core::configuration::ApprovalMode::Yolo,
        )
        .await
        .unwrap();
    let (sink, _rx) = ChannelSink::channel();
    let sink: Arc<dyn themis_desktop::sink::EventSink> = Arc::new(sink);
    state
        .send_message(
            sink.clone(),
            thread.clone(),
            "/goal write every file".into(),
        )
        .await
        .unwrap();
    let checked = tokio::time::timeout(DEADLINE, async {
        while checks.load(Ordering::SeqCst) < 4 {
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
    })
    .await;
    assert!(checked.is_ok(), "goal: {:?}", state.goal_for(&thread));
    settle(&state, &thread).await;
    let goal = state.goal_for(&thread).unwrap();
    assert_eq!(goal.stalls, 0, "every turn wrote a file");
    let requests = server.received_requests().await.unwrap();
    assert!(!requests
        .iter()
        .map(body)
        .any(|body| !is_check(&body) && mentions(&body, "try a different approach")));
}

#[tokio::test(flavor = "multi_thread")]
async fn a_running_goal_saves_its_elapsed_time_during_a_turn() {
    // A live goal showed only the current turn's time: elapsed time was saved
    // at turn boundaries, so each app restart mid-turn lost the whole turn.
    let server = MockServer::start().await;
    let released = Gate::default();
    let gate = released.clone();
    Mock::given(method("POST"))
        .respond_with(move |request: &Request| {
            if is_check(&body(request)) {
                return reply(r#"{"verdict":"reached","reason":"Done.","progress":"done"}"#);
            }
            gate.wait();
            reply("Worked on it.")
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
async fn pause_holds_the_goal_resume_continues_and_stalls_never_stop_it() {
    let server = MockServer::start().await;
    // The first turn's answer waits for the test to pause the goal, so the
    // pause always lands while that turn runs, however slow the machine.
    let released = Gate::default();
    let gate = released.clone();
    Mock::given(method("POST"))
        .respond_with(move |request: &Request| {
            if is_check(&body(request)) {
                return reply(
                    r#"{"verdict":"not_yet","reason":"Still 300 ms.","progress":"p95 300 ms"}"#,
                );
            }
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
    // The running turn finishes; no further turn and no check starts while
    // paused. (One turn can make more than one request: a stream attempt and
    // its plain retry; so count goal turns and checks, not requests.)
    let turns_and_checks = |requests: &[Request]| {
        requests
            .iter()
            .map(body)
            .filter(|body| is_check(body) || mentions(body, "[Goal — runtime, not from the user]"))
            .count()
    };
    tokio::time::sleep(Duration::from_millis(2500)).await;
    let requests = server.received_requests().await.unwrap();
    assert_eq!(
        turns_and_checks(&requests),
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
                format!(
                    "check={} stream={} last={last}",
                    is_check(&body),
                    body["stream"]
                )
            })
            .collect::<Vec<_>>()
    );
    tokio::time::sleep(Duration::from_millis(500)).await;
    assert_eq!(
        turns_and_checks(&server.received_requests().await.unwrap()),
        0
    );
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
    let checks = |requests: &[Request]| {
        requests
            .iter()
            .filter(|request| is_check(&body(request)))
            .count()
    };
    let kept_going = tokio::time::timeout(DEADLINE, async {
        while checks(&server.received_requests().await.unwrap()) < 5 {
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
    })
    .await;
    assert!(kept_going.is_ok(), "goal: {:?}", state.goal_for(&thread));
    let goal = state.goal_for(&thread).unwrap();
    assert_eq!(goal.status, GoalStatus::Running);
    assert!(goal.stalls >= 3, "stalls {}", goal.stalls);
    // Editing the goal gives it a fresh start.
    let edited = state
        .edit_goal(
            sink.clone(),
            thread.clone(),
            "speed up p95 with a cache".into(),
        )
        .await
        .unwrap();
    assert_eq!(edited.stalls, 0);
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
    let server = MockServer::start().await;
    let quota = Arc::new(Mutex::new(true));
    let out_of_quota = Arc::clone(&quota);
    Mock::given(method("POST"))
        .respond_with(move |request: &Request| {
            let body = body(request);
            if *out_of_quota.lock().unwrap() {
                return ResponseTemplate::new(429)
                    .set_body_string("monthly usage limit reached for your plan");
            }
            if is_check(&body) {
                reply(r#"{"verdict":"reached","reason":"Done.","progress":"done"}"#)
            } else {
                reply("Finished the work.")
            }
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
        .rfind(|body| !is_check(body))
        .unwrap();
    assert_eq!(last_turn["model"], "other-model");
    settle(&state, &thread).await;
}

#[tokio::test(flavor = "multi_thread")]
async fn a_running_goal_resumes_when_the_app_starts_again() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .respond_with(|request: &Request| {
            if is_check(&body(request)) {
                reply(r#"{"verdict":"reached","reason":"Done after restart."}"#)
            } else {
                reply("Re-checked and finished.")
            }
        })
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
        .find(|body| !is_check(body))
        .unwrap();
    assert!(mentions(&first, "interrupted when Themis closed"));
    settle(&restarted, &thread).await;
}

#[tokio::test(flavor = "multi_thread")]
async fn the_agent_sets_a_goal_with_its_tool_and_turns_follow() {
    let server = MockServer::start().await;
    let checks = Arc::new(AtomicUsize::new(0));
    let seen_checks = Arc::clone(&checks);
    Mock::given(method("POST"))
        .respond_with(move |request: &Request| {
            let body = body(request);
            if is_check(&body) {
                // The run that set the goal is checked too; it did no work yet.
                return if seen_checks.fetch_add(1, Ordering::SeqCst) == 0 {
                    reply(r#"{"verdict":"not_yet","reason":"No test has run yet."}"#)
                } else {
                    reply(r#"{"verdict":"reached","reason":"All green."}"#)
                };
            }
            let used_tool = body["messages"]
                .as_array()
                .is_some_and(|messages| messages.iter().any(|m| m["role"] == "tool"));
            if mentions(&body, "make it a goal") && !used_tool && !mentions(&body, "[Goal — runtime") {
                ResponseTemplate::new(200).set_body_json(json!({"choices":[{"message":{"role":"assistant","content":null,"tool_calls":[{"id":"g","type":"function","function":{"name":"set_goal","arguments":"{\"goal\":\"get every test green\"}"}}]},"finish_reason":"tool_calls"}]}))
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
    assert_eq!(checks.load(Ordering::SeqCst), 2);
    settle(&state, &thread).await;
}

/// Found in a live run: a message's run finished the goal and said so, yet
/// only goal turns were checked, so a turn redid the verification and another
/// announced the goal reached: three messages saying the same thing. Every run
/// is checked now, and an answer that already tells the user needs no
/// announcement.
#[tokio::test(flavor = "multi_thread")]
async fn a_message_run_that_finishes_the_goal_is_checked_and_nothing_repeats_it() {
    let server = MockServer::start().await;
    let gate = Gate::default();
    let held = gate.clone();
    let checks = Arc::new(AtomicUsize::new(0));
    let seen_checks = Arc::clone(&checks);
    let agent_calls = Arc::new(AtomicUsize::new(0));
    let calls = Arc::clone(&agent_calls);
    Mock::given(method("POST"))
        .respond_with(move |request: &Request| {
            let body = body(request);
            if is_check(&body) {
                seen_checks.fetch_add(1, Ordering::SeqCst);
                return reply(r#"{"verdict":"reached","reason":"PASS4 shows four lines.","progressed":true,"told_user":true}"#);
            }
            if calls.fetch_add(1, Ordering::SeqCst) == 0 {
                held.wait();
                reply("Wrote three lines.")
            } else {
                reply("Added yellow; PASS4 shows all four lines, so the goal is done.")
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
            "/goal colors.txt lists the colors".into(),
        )
        .await
        .unwrap();
    state
        .enqueue_message(
            sink.clone(),
            thread.clone(),
            "Also add yellow.".into(),
            None,
            vec![],
        )
        .await
        .unwrap();
    gate.open();
    goal_until(&state, &thread, |status| status == GoalStatus::Reached).await;
    settle(&state, &thread).await;
    assert_eq!(checks.load(Ordering::SeqCst), 1);
    let bodies = distinct(
        server
            .received_requests()
            .await
            .unwrap()
            .iter()
            .map(body)
            .collect(),
    );
    let turns: Vec<&Value> = bodies
        .iter()
        .filter(|body| !is_check(body) && !is_checklist(body))
        .collect();
    assert_eq!(turns.len(), 2, "no goal turn and no announcement followed");
    assert!(!turns.iter().any(|body| mentions(body, "[Goal — runtime")));
    assert_eq!(state.goal_for(&thread).unwrap().turns, 1);
}

fn calls(name: &str, arguments: Value) -> ResponseTemplate {
    ResponseTemplate::new(200).set_body_json(json!({"choices":[{"message":{"role":"assistant","content":null,"tool_calls":[{"id":"call_1","type":"function","function":{"name":name,"arguments":arguments.to_string()}}]},"finish_reason":"tool_calls"}]}))
}

/// Found in a live run: asked to stop the goal, the agent had no way to and said
/// it had, and the next turn started anyway.
#[tokio::test(flavor = "multi_thread")]
async fn asked_to_stop_the_agent_pauses_the_goal_and_no_check_or_turn_follows() {
    let server = MockServer::start().await;
    let gate = Gate::default();
    let held = gate.clone();
    let checks = Arc::new(AtomicUsize::new(0));
    let seen_checks = Arc::clone(&checks);
    Mock::given(method("POST"))
        .respond_with(move |request: &Request| {
            let body = body(request);
            if is_check(&body) {
                seen_checks.fetch_add(1, Ordering::SeqCst);
                return reply(r#"{"verdict":"not_yet","reason":"Keep going."}"#);
            }
            let sent = body["messages"].to_string();
            if sent.contains("pause") && sent.contains("\"status\"") {
                reply("The goal is paused.")
            } else if sent.contains("Please stop the goal") {
                calls("set_goal", json!({"pause": true}))
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
    assert_eq!(checks.load(Ordering::SeqCst), 0);
}

/// What a check found proven survives into the next check and turn, which see it, and the
/// next turn knows which files the work is in.
#[tokio::test(flavor = "multi_thread")]
async fn proof_and_files_carry_from_turn_to_turn() {
    let server = MockServer::start().await;
    let checks = Arc::new(AtomicUsize::new(0));
    let seen_checks = Arc::clone(&checks);
    let later_check = Arc::new(Mutex::new(String::new()));
    let seen_later_check = Arc::clone(&later_check);
    let later_turn = Arc::new(Mutex::new(String::new()));
    let seen_later_turn = Arc::clone(&later_turn);
    Mock::given(method("POST"))
        .respond_with(move |request: &Request| {
            let body = body(request);
            let sent = body["messages"].to_string();
            if is_check(&body) {
                return if seen_checks.fetch_add(1, Ordering::SeqCst) == 0 {
                    reply(r#"{"verdict":"not_yet","reason":"The health part isn't shown yet.","next":"Run check_health.py next.","requirements":[{"what":"Score rises","proven":true,"how":"0 to 10 after a real click"},{"what":"Health drops","proven":false}]}"#)
                } else {
                    *seen_later_check.lock().unwrap() = sent;
                    reply(r#"{"verdict":"reached","reason":"Both are shown.","requirements":[{"what":"Score rises","proven":true,"how":"0 to 10 after a real click"},{"what":"Health drops","proven":true,"how":"100 to 90"}]}"#)
                };
            }
            if mentions(&body, "A separate check found the goal reached") {
                return reply("Done.");
            }
            if sent.contains("Turn 2.") {
                *seen_later_turn.lock().unwrap() = sent;
                return reply("Checked health.");
            }
            if sent.contains("score.txt") {
                return reply("Wrote the score.");
            }
            calls("write_file", json!({"file_path": "score.txt", "content": "10"}))
        })
        .mount(&server)
        .await;
    let (state, _data, project, thread) = chat(&server).await;
    // Changed files are read from Git, as in a real project.
    let git = std::process::Command::new("git")
        .args(["init", "-q"])
        .current_dir(project.path())
        .status()
        .unwrap();
    assert!(git.success());
    state
        .set_thread_approval_mode(
            thread.clone(),
            themis_core::configuration::ApprovalMode::Yolo,
        )
        .await
        .unwrap();
    let (sink, mut rx) = ChannelSink::channel();
    let sink = Arc::new(sink);
    state
        .send_message(sink.clone(), thread.clone(), "/goal finish the game".into())
        .await
        .unwrap();
    goal_until(&state, &thread, |status| status == GoalStatus::Reached).await;
    let goal = state.goal_for(&thread).unwrap();
    assert_eq!(goal.proof.len(), 2, "{goal:?}");
    assert!(goal.proof.iter().all(|item| item.proven), "{goal:?}");
    assert_eq!(goal.files, vec!["score.txt".to_owned()], "{goal:?}");
    let turn = later_turn.lock().unwrap().clone();
    assert!(turn.contains("Score rises"), "{turn}");
    assert!(turn.contains("0 to 10 after a real click"), "{turn}");
    assert!(turn.contains("score.txt"), "{turn}");
    assert!(turn.contains("Run check_health.py next."), "{turn}");
    let check = later_check.lock().unwrap().clone();
    assert!(check.contains("0 to 10 after a real click"), "{check}");
    let mut seen = Vec::new();
    until(
        &mut rx,
        &mut seen,
        |event| matches!(event, ThreadEvent::Finished { result, .. } if result == "Done."),
    )
    .await;
    let notes: Vec<_> = seen
        .iter()
        .filter_map(|event| match event {
            ThreadEvent::GoalNote { reason, .. } => Some(reason.clone()),
            _ => None,
        })
        .collect();
    assert!(
        notes
            .iter()
            .any(|note| note.contains("health part isn't shown yet")),
        "{notes:?}"
    );
    // The person sees what the agent is told: what's wrong, what to change, what's left.
    assert!(
        notes
            .iter()
            .any(|note| note
                .contains("\n\nRun check_health.py next.\n\nStill pending: Health drops")),
        "{notes:?}"
    );
}

/// When the turn's answer already told the person, no closing turn runs, but the chat still
/// shows how the check came out.
#[tokio::test(flavor = "multi_thread")]
async fn a_goal_reached_without_a_closing_turn_still_says_so_in_the_chat() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .respond_with(|request: &Request| {
            let body = body(request);
            if is_check(&body) {
                return reply(r#"{"verdict":"reached","reason":"Score and health both change with real input.","told_user":true}"#);
            }
            reply("The game works: score and health change with real input.")
        })
        .mount(&server)
        .await;
    let (state, _data, _project, thread) = chat(&server).await;
    let (sink, mut rx) = ChannelSink::channel();
    state
        .send_message(
            Arc::new(sink),
            thread.clone(),
            "/goal finish the game".into(),
        )
        .await
        .unwrap();
    goal_until(&state, &thread, |status| status == GoalStatus::Reached).await;
    let mut seen = Vec::new();
    until(&mut rx, &mut seen, |event| {
        matches!(event, ThreadEvent::GoalNote { reason, .. } if reason.starts_with("Complete."))
    })
    .await;
    let note = seen
        .iter()
        .find_map(|event| match event {
            ThreadEvent::GoalNote { reason, .. } if reason.starts_with("Complete.") => {
                Some(reason.clone())
            }
            _ => None,
        })
        .unwrap();
    assert!(note.contains("Score and health both change"), "{note}");
}

/// The checklist is live: it appears when the goal is set, takes a steered requirement at once,
/// and follows an edited goal, and refreshing it can never complete an item.
#[tokio::test(flavor = "multi_thread")]
async fn the_checklist_follows_the_goal_as_it_changes() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .respond_with(move |request: &Request| {
            let body = body(request);
            let sent = body["messages"].to_string();
            if is_check(&body) {
                return reply(r#"{"verdict":"not_yet","reason":"Keep going."}"#);
            }
            if body["messages"][0]["content"].as_str().is_some_and(|text| text.contains("what a goal asks for")) {
                return if sent.contains("in Rust") {
                    reply(r#"{"requirements":[{"what":"Written in Rust","proven":true}]}"#)
                } else if sent.contains("Also add a README") {
                    reply(r#"{"requirements":[{"what":"The game runs"},{"what":"A README exists","proven":true}]}"#)
                        .set_delay(std::time::Duration::from_secs(1))
                } else {
                    reply(r#"{"requirements":[{"what":"The game runs"}]}"#)
                };
            }
            // Long enough to stay running through the test, without blocking the mock server.
            reply("Working on it.").set_delay(std::time::Duration::from_secs(6))
        })
        .mount(&server)
        .await;
    let (state, _data, _project, thread) = chat(&server).await;
    let (sink, _rx) = ChannelSink::channel();
    let sink = Arc::new(sink);
    async fn checklist_until(
        state: &AppState,
        thread: &str,
        done: impl Fn(&[(String, bool)]) -> bool,
    ) -> Vec<(String, bool)> {
        for _ in 0..200 {
            let list: Vec<_> = state
                .goal_for(thread)
                .map(|goal| goal.proof)
                .unwrap_or_default()
                .into_iter()
                .map(|item| (item.what, item.proven))
                .collect();
            if done(&list) {
                return list;
            }
            tokio::time::sleep(std::time::Duration::from_millis(25)).await;
        }
        panic!("checklist never got there: {:?}", state.goal_for(thread));
    }
    state
        .send_message(sink.clone(), thread.clone(), "/goal make the game".into())
        .await
        .unwrap();
    // Set: listed before any check has run.
    checklist_until(&state, &thread, |list| {
        list == [("The game runs".to_owned(), false)]
    })
    .await;

    let request = state
        .enqueue_message(
            sink.clone(),
            thread.clone(),
            "Also add a README".into(),
            None,
            vec![],
        )
        .await
        .unwrap();
    state
        .steer_queued_message(sink.clone(), thread.clone(), request.id)
        .await
        .unwrap();
    // Steered: pending at once, in the person's words, before the refresh's reply arrives.
    let now = state.goal_for(&thread).unwrap().proof;
    assert!(
        now.iter()
            .any(|item| item.what == "Also add a README" && !item.proven),
        "{now:?}"
    );
    // Refreshed: tidied into the list, and the model's "proven" is ignored.
    let list = checklist_until(&state, &thread, |list| {
        list.iter().any(|item| item.0 == "A README exists")
    })
    .await;
    assert_eq!(
        list,
        [
            ("The game runs".to_owned(), false),
            ("A README exists".to_owned(), false)
        ]
    );

    // Edited: the list follows the new goal, and dropped items go.
    state
        .edit_goal(sink.clone(), thread.clone(), "make the game in Rust".into())
        .await
        .unwrap();
    let list = checklist_until(&state, &thread, |list| {
        list.iter().any(|item| item.0 == "Written in Rust")
    })
    .await;
    assert_eq!(list, [("Written in Rust".to_owned(), false)]);
    state
        .remove_goal(sink.clone(), thread.clone())
        .await
        .unwrap();
    settle(&state, &thread).await;
}

/// A failed turn waits with a countdown the chat can show, and leaves no note behind that
/// would go out of date.
#[tokio::test(flavor = "multi_thread")]
async fn a_failed_turn_counts_down_instead_of_leaving_a_note() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .respond_with(|request: &Request| {
            if is_checklist(&body(request)) {
                return reply(r#"{"requirements":[{"what":"The thing"}]}"#);
            }
            ResponseTemplate::new(400).set_body_string("bad request")
        })
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

/// While a long turn runs, the checklist moves as evidence comes in. These passes only ever
/// update the list: even a check saying "reached" can't end the goal in the middle of a turn.
#[tokio::test(flavor = "multi_thread")]
async fn the_checklist_moves_during_a_long_turn_and_cannot_end_the_goal() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .respond_with(|request: &Request| {
            let body = body(request);
            let sent = body["messages"].to_string();
            if is_checklist(&body) {
                return reply(r#"{"requirements":[{"what":"Notes exist"}]}"#);
            }
            if is_check(&body) {
                return reply(r#"{"verdict":"reached","reason":"Done.","told_user":true,"requirements":[{"what":"Notes exist","proven":true,"how":"written"}]}"#);
            }
            if sent.contains("notes.txt") {
                // The turn stays open a while after the work that can be checked.
                return reply("All written.").set_delay(std::time::Duration::from_secs(5));
            }
            calls("write_file", json!({"file_path": "notes.txt", "content": "hello"}))
        })
        .mount(&server)
        .await;
    let (state, _data, project, thread) = chat(&server).await;
    std::fs::create_dir_all(project.path().join(".themis")).unwrap();
    std::fs::write(
        project.path().join(".themis/config.jsonc"),
        r#"{"goal":{"checklist_refresh_seconds":1}}"#,
    )
    .unwrap();
    state
        .set_thread_approval_mode(
            thread.clone(),
            themis_core::configuration::ApprovalMode::Yolo,
        )
        .await
        .unwrap();
    let (sink, _rx) = ChannelSink::channel();
    let sink = Arc::new(sink);
    state
        .send_message(sink.clone(), thread.clone(), "/goal write the notes".into())
        .await
        .unwrap();
    let mut moved = None;
    for _ in 0..160 {
        let goal = state.goal_for(&thread).unwrap();
        if goal.proof.iter().any(|item| item.proven) {
            moved = Some(goal);
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(25)).await;
    }
    let goal = moved.expect("the checklist never moved during the turn");
    assert_eq!(goal.status, GoalStatus::Running, "{goal:?}");
    goal_until(&state, &thread, |status| status == GoalStatus::Reached).await;
}

/// Asked in the chat to remove the goal, the agent removes it: no goal is left, and
/// no check or further turn follows.
#[tokio::test(flavor = "multi_thread")]
async fn asked_to_remove_the_goal_the_agent_removes_it() {
    let server = MockServer::start().await;
    let gate = Gate::default();
    let held = gate.clone();
    let checks = Arc::new(AtomicUsize::new(0));
    let seen_checks = Arc::clone(&checks);
    Mock::given(method("POST"))
        .respond_with(move |request: &Request| {
            let body = body(request);
            if is_check(&body) {
                seen_checks.fetch_add(1, Ordering::SeqCst);
                return reply(r#"{"verdict":"not_yet","reason":"Keep going."}"#);
            }
            let sent = body["messages"].to_string();
            if sent.contains("removed") && sent.contains("\"status\"") {
                reply("The goal is removed.")
            } else if sent.contains("Please remove the goal") {
                calls("set_goal", json!({"remove": true}))
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
    let ask = state
        .enqueue_message(
            sink.clone(),
            thread.clone(),
            "Please remove the goal".into(),
            None,
            vec![],
        )
        .await
        .unwrap();
    state
        .steer_queued_message(sink.clone(), thread.clone(), ask.id)
        .await
        .unwrap();
    gate.open();
    for _ in 0..200 {
        if state.goal_for(&thread).is_none() {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    }
    settle(&state, &thread).await;
    assert!(state.goal_for(&thread).is_none());
    assert_eq!(checks.load(Ordering::SeqCst), 0);
}

/// A goal keeps going until it is reached: the agent can't talk itself out of a
/// hard one by pausing it.
#[tokio::test(flavor = "multi_thread")]
async fn the_agent_cannot_pause_a_goal_nobody_asked_to_stop() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .respond_with(|request: &Request| {
            let body = body(request);
            if is_check(&body) {
                return reply(r#"{"verdict":"reached","reason":"Done.","told_user":true}"#);
            }
            let used_tool = body["messages"]
                .as_array()
                .is_some_and(|messages| messages.iter().any(|m| m["role"] == "tool"));
            if used_tool {
                reply("Carrying on, and it's done.")
            } else {
                calls("set_goal", json!({"pause": true}))
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

/// Found in a live run: the agent reworded the goal with `set_goal` and its
/// elapsed time and turn count started again from zero.
#[tokio::test(flavor = "multi_thread")]
async fn rewording_an_open_goal_keeps_its_time_and_turns() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .respond_with(|request: &Request| {
            let body = body(request);
            if is_check(&body) {
                return reply(r#"{"verdict":"reached","reason":"Four lines.","told_user":true}"#);
            }
            let used_tool = body["messages"]
                .as_array()
                .is_some_and(|messages| messages.iter().any(|m| m["role"] == "tool"));
            if used_tool {
                reply("Goal reworded to four lines; the file has them.")
            } else {
                std::thread::sleep(Duration::from_millis(600));
                ResponseTemplate::new(200).set_body_json(json!({"choices":[{"message":{"role":"assistant","content":null,"tool_calls":[{"id":"g","type":"function","function":{"name":"set_goal","arguments":"{\"goal\":\"colors.txt has four lines\"}"}}]},"finish_reason":"tool_calls"}]}))
            }
        })
        .mount(&server)
        .await;
    let (state, _data, _project, thread) = chat(&server).await;
    let (sink, _rx) = ChannelSink::channel();
    state
        .send_message(
            Arc::new(sink),
            thread.clone(),
            "/goal colors.txt has three lines".into(),
        )
        .await
        .unwrap();
    goal_until(&state, &thread, |status| status == GoalStatus::Reached).await;
    let goal = state.goal_for(&thread).unwrap();
    assert_eq!(goal.text, "colors.txt has four lines");
    assert_eq!(goal.turns, 1, "the /goal run stays turn 1");
    assert!(
        goal.active_ms >= 550,
        "time before the rewording is kept: {} ms",
        goal.active_ms
    );
    settle(&state, &thread).await;
}

/// Found in a live run: the work happened in a queued message's run, the next
/// goal turn only reported it, and the check (seeing that turn alone) asked the
/// user. Evidence now spans every run since the previous check, and "waiting
/// for you" needs a question in the answer.
#[tokio::test(flavor = "multi_thread")]
async fn the_check_sees_work_done_since_the_last_check_and_never_waits_without_a_question() {
    let server = MockServer::start().await;
    let gate = Gate::default();
    let held = gate.clone();
    let checks = Arc::new(AtomicUsize::new(0));
    let seen_checks = Arc::clone(&checks);
    Mock::given(method("POST"))
        .respond_with(move |request: &Request| {
            let body = body(request);
            if is_check(&body) {
                return if seen_checks.fetch_add(1, Ordering::SeqCst) == 0 {
                    reply(r#"{"verdict":"needs_user","reason":"No fresh evidence this turn."}"#)
                } else {
                    reply(r#"{"verdict":"reached","reason":"Listing shown."}"#)
                };
            }
            let tool_done = body["messages"]
                .as_array()
                .is_some_and(|messages| messages.iter().any(|m| m["role"] == "tool"));
            let last_user = body["messages"]
                .as_array()
                .and_then(|messages| messages.iter().rev().find(|m| m["role"] == "user"))
                .and_then(|m| m["content"].as_str())
                .unwrap_or_default()
                .to_owned();
            if last_user.contains("list the folder") && !tool_done {
                ResponseTemplate::new(200).set_body_json(json!({"choices":[{"message":{"role":"assistant","content":null,"tool_calls":[{"id":"l","type":"function","function":{"name":"list_dir","arguments":"{\"path\":\".\"}"}}]},"finish_reason":"tool_calls"}]}))
            } else if last_user.contains("list the folder") {
                reply("Listed the folder.")
            } else if last_user.contains("/goal") {
                held.wait();
                reply("Starting.")
            } else {
                reply("Already done earlier.")
            }
        })
        .mount(&server)
        .await;
    let (state, _data, _project, thread) = chat(&server).await;
    state
        .set_thread_approval_mode(
            thread.clone(),
            themis_core::configuration::ApprovalMode::Yolo,
        )
        .await
        .unwrap();
    let (sink, _rx) = ChannelSink::channel();
    let sink: Arc<dyn themis_desktop::sink::EventSink> = Arc::new(sink);
    state
        .send_message(
            sink.clone(),
            thread.clone(),
            "/goal show the folder listing".into(),
        )
        .await
        .unwrap();
    state
        .enqueue_message(
            sink.clone(),
            thread.clone(),
            "list the folder".into(),
            None,
            vec![],
        )
        .await
        .unwrap();
    gate.open();
    // "Waiting for you" without a question went on as not yet, to the end.
    goal_until(&state, &thread, |status| status == GoalStatus::Reached).await;
    assert_eq!(checks.load(Ordering::SeqCst), 2);
    let first_check = server
        .received_requests()
        .await
        .unwrap()
        .iter()
        .map(body)
        .find(is_check)
        .unwrap();
    assert!(
        mentions(&first_check, "list_dir"),
        "the queued run's command is evidence for the turn after it"
    );
    settle(&state, &thread).await;
}

/// Review findings: a queued message refused before any run starts (a bare
/// `/goal`) stays in the queue, held, with its reason in the chat, and the
/// queue moves on; a steer with no run to receive it is refused and the message
/// stays queued.
#[tokio::test(flavor = "multi_thread")]
async fn unsendable_queued_messages_are_recorded_and_the_queue_moves_on() {
    let server = MockServer::start().await;
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

/// Review findings: a `/goal` whose run cannot start still continues (it
/// retries as a failed turn instead of staying "running" with no turn), and a
/// goal check that fails does not count as a turn without progress.
/// A model that keeps refusing pauses the goal instead of retrying for ever, says why in its
/// status, and adds no note to the chat. Takes the two retry waits (30 s and 60 s, jittered).
#[tokio::test(flavor = "multi_thread")]
async fn a_goal_pauses_after_its_turns_keep_failing() {
    let server = MockServer::start().await;
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

#[tokio::test(flavor = "multi_thread")]
async fn a_goal_whose_turn_cannot_start_retries_and_a_failed_check_is_not_a_stall() {
    let server = MockServer::start().await;
    let checks = Arc::new(AtomicUsize::new(0));
    let seen_checks = Arc::clone(&checks);
    Mock::given(method("POST"))
        .respond_with(move |request: &Request| {
            if is_check(&body(request)) {
                // The first check fails twice (no decision); the next decides.
                return if seen_checks.fetch_add(1, Ordering::SeqCst) < 2 {
                    ResponseTemplate::new(500).set_body_string("checker down")
                } else {
                    reply(r#"{"verdict":"reached","reason":"Done."}"#)
                };
            }
            reply("Worked on it.")
        })
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
    // The next turn carries the failed check's note.
    let continued = tokio::time::timeout(DEADLINE, async {
        loop {
            let requests = server.received_requests().await.unwrap();
            if requests.iter().map(body).any(|body| {
                !is_check(&body) && mentions(&body, "Last check: The goal check could not decide")
            }) {
                return;
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
    })
    .await;
    assert!(continued.is_ok(), "goal: {:?}", state.goal_for(&thread));
    assert_eq!(
        state.goal_for(&thread).unwrap().stalls,
        0,
        "a failed check is not a stall"
    );
    goal_until(&state, &thread, |status| status == GoalStatus::Reached).await;
    settle(&state, &thread).await;
}

/// Third review: an error that belongs to the chat (its project's
/// configuration became invalid) holds the first message with its reason and
/// leaves the rest waiting in order, with one note instead of one per message.
#[tokio::test(flavor = "multi_thread")]
async fn a_chat_wide_error_holds_one_message_and_keeps_the_rest_in_order() {
    let server = MockServer::start().await;
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
    let server = MockServer::start().await;
    let checks = Arc::new(AtomicUsize::new(0));
    let seen_checks = Arc::clone(&checks);
    Mock::given(method("POST"))
        .respond_with(move |request: &Request| {
            if is_check(&body(request)) {
                return if seen_checks.fetch_add(1, Ordering::SeqCst) == 0 {
                    reply(r#"{"verdict":"not_yet","reason":"One more step.","progress":"step 1"}"#)
                } else {
                    reply(r#"{"verdict":"reached","reason":"Done.","progress":"step 2"}"#)
                };
            }
            if mentions(&body(request), "A separate check found the goal reached") {
                return reply("Both steps are done, so the goal is reached.");
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
        r#"{"schema_version":1,"context_token_budget":64000}"#,
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

/// A request steered in while a goal runs is part of the goal: the goal keeps
/// it and the goal check is shown it. A goal turn and an answer to the
/// agent's question are not requests.
#[tokio::test(flavor = "multi_thread")]
async fn a_request_steered_into_a_goal_reaches_the_goal_check() {
    let server = MockServer::start().await;
    let gate = Gate::default();
    let held = gate.clone();
    let checks = Arc::new(Mutex::new(Vec::new()));
    let seen = Arc::clone(&checks);
    let agent_calls = Arc::new(AtomicUsize::new(0));
    let calls = Arc::clone(&agent_calls);
    Mock::given(method("POST"))
        .respond_with(move |request: &Request| {
            let body = body(request);
            if is_check(&body) {
                seen.lock().unwrap().push(body);
                return reply(r#"{"verdict":"reached","reason":"The report and the CSV export are done.","progress":"","progressed":true}"#);
            }
            if calls.fetch_add(1, Ordering::SeqCst) == 0 {
                held.wait();
                reply("Report written.")
            } else {
                reply("Report written and exported as CSV.")
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
            "/goal Write the monthly report.".into(),
        )
        .await
        .unwrap();
    let steer = state
        .enqueue_message(
            sink.clone(),
            thread.clone(),
            "Also export CSV.".into(),
            None,
            vec![],
        )
        .await
        .unwrap();
    state
        .steer_queued_message(sink.clone(), thread.clone(), steer.id)
        .await
        .unwrap();
    gate.open();
    goal_until(&state, &thread, |status| status == GoalStatus::Reached).await;

    let goal = state.goal_for(&thread).unwrap();
    assert_eq!(goal.added, ["Also export CSV."]);
    let checks = distinct(checks.lock().unwrap().clone());
    let input = checks[0]["messages"][1]["content"].as_str().unwrap();
    assert!(
        input.contains("Requests the user added while this goal ran."),
        "{input}"
    );
    assert!(input.contains("1. Also export CSV."), "{input}");
    assert!(
        !input.contains("[Goal"),
        "a goal turn is not a request: {input}"
    );
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
    let server = MockServer::start().await;
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
    let server = MockServer::start().await;
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
    let server = MockServer::start().await;
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
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .respond_with(|request: &Request| {
            let body = body(request);
            if is_checklist(&body) {
                return reply(r#"{"requirements":[{"what":"The thing"}]}"#);
            }
            if is_check(&body) {
                return reply(r#"{"verdict":"not_yet","reason":"Keep going."}"#);
            }
            reply("Hi.")
        })
        .mount(&server)
        .await;
    let (state, _data, _project, thread) = chat(&server).await;
    let (sink, _rx) = ChannelSink::channel();
    let sink = Arc::new(sink);
    let last_sent = |bodies: Vec<wiremock::Request>| {
        bodies
            .iter()
            .map(body)
            .rfind(|body| !is_check(body) && !is_checklist(body))
            .unwrap()["messages"]
            .to_string()
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
