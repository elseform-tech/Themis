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

const DEADLINE: Duration = Duration::from_secs(60);
const CHECK_MARKER: &str = "You decide whether an agent has reached a goal";

fn reply(text: &str) -> ResponseTemplate {
    ResponseTemplate::new(200).set_body_json(json!({
        "choices": [{"message": {"role": "assistant", "content": text}, "finish_reason": "stop"}]
    }))
}

fn body(request: &Request) -> Value {
    request.body_json().unwrap_or(Value::Null)
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

/// Waits until the chat's goal satisfies `done`.
async fn goal_until(state: &AppState, thread_id: &str, done: impl Fn(GoalStatus) -> bool) {
    tokio::time::timeout(DEADLINE, async {
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
    .await
    .expect("goal state before the deadline");
}

#[tokio::test(flavor = "multi_thread")]
async fn queued_messages_wait_steers_reach_the_run_and_the_queue_drains() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .respond_with(|request: &Request| {
            let body = body(request);
            if mentions(&body, "Use tabs.") && !mentions(&body, "second message") {
                reply("Adjusted to tabs.")
            } else if mentions(&body, "second message") {
                reply("Second done.")
            } else {
                // Slow enough for the test to queue and steer while it runs.
                reply("First done.").set_delay(Duration::from_millis(1500))
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
    let bodies: Vec<Value> = server
        .received_requests()
        .await
        .unwrap()
        .iter()
        .map(body)
        .collect();
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
        .steer_queued_message(sink, thread, idle.id)
        .await
        .err();
    // The idle chat sent "after" at once, so it is no longer queued.
    assert!(error.is_some());
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
    let mut seen = Vec::new();
    until(&mut rx, &mut seen, |event| {
        matches!(
            event,
            ThreadEvent::GoalNote {
                outcome: GoalNoteKind::Reached,
                ..
            }
        )
    })
    .await;
    assert!(seen.iter().any(|event| matches!(event, ThreadEvent::GoalNote { outcome: GoalNoteKind::NotYet, reason } if reason == "Counter is 1; needs 2.")));
    // The second turn was started by the runtime, carrying the check's reason.
    let bodies: Vec<Value> = server
        .received_requests()
        .await
        .unwrap()
        .iter()
        .map(body)
        .collect();
    let turns: Vec<&Value> = bodies.iter().filter(|body| !is_check(body)).collect();
    assert_eq!(turns.len(), 2);
    assert!(mentions(turns[0], "Active goal for this chat"));
    assert!(mentions(
        turns[1],
        "[Goal — runtime, not from the user] Turn 2."
    ));
    assert!(mentions(turns[1], "Last check: Counter is 1; needs 2."));
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
}

#[tokio::test(flavor = "multi_thread")]
async fn pause_holds_the_goal_resume_continues_and_stalls_pause_it() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .respond_with(|request: &Request| {
            if is_check(&body(request)) {
                reply(r#"{"verdict":"not_yet","reason":"Still 300 ms.","progress":"p95 300 ms"}"#)
            } else {
                // Long enough that the pause lands while the first turn runs,
                // even on a slow CI machine.
                reply("Tried another cache setting.").set_delay(Duration::from_millis(1200))
            }
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
    // The running turn finishes; no further turn (and no check) starts while paused.
    tokio::time::sleep(Duration::from_millis(2500)).await;
    let before = server.received_requests().await.unwrap().len();
    assert_eq!(before, 1, "only the first turn ran");
    tokio::time::sleep(Duration::from_millis(500)).await;
    assert_eq!(server.received_requests().await.unwrap().len(), 1);
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
    // Every turn makes no progress and changes no file: after three it is stuck.
    goal_until(&state, &thread, |status| status == GoalStatus::Stuck).await;
    let goal = state.goal_for(&thread).unwrap();
    assert!(goal
        .reason
        .unwrap()
        .starts_with("No progress over the last 3 turns"));
    assert_eq!(goal.stalls, 3);
    let checks = server
        .received_requests()
        .await
        .unwrap()
        .iter()
        .filter(|request| is_check(&body(request)))
        .count();
    // The first measurement counts as progress; three unchanged ones follow.
    assert_eq!(checks, 4);
    // Editing the goal and resuming gives it a fresh start.
    state
        .edit_goal(
            sink.clone(),
            thread.clone(),
            "speed up p95 with a cache".into(),
        )
        .await
        .unwrap();
    state
        .resume_goal(sink.clone(), thread.clone())
        .await
        .unwrap();
    assert_eq!(state.goal_for(&thread).unwrap().status, GoalStatus::Running);
    state.pause_goal(sink, thread).await.unwrap();
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
    // Time with the app closed did not count.
    assert!(goal.active_ms < 60_000, "{}", goal.active_ms);
    let first = server
        .received_requests()
        .await
        .unwrap()
        .iter()
        .map(body)
        .find(|body| !is_check(body))
        .unwrap();
    assert!(mentions(&first, "interrupted when Themis closed"));
}

#[tokio::test(flavor = "multi_thread")]
async fn the_agent_sets_a_goal_with_its_tool_and_turns_follow() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .respond_with(|request: &Request| {
            let body = body(request);
            if is_check(&body) {
                return reply(r#"{"verdict":"reached","reason":"All green."}"#);
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
}

/// Found in a live run: the work happened in a queued message's run, the next
/// goal turn only reported it, and the check (seeing that turn alone) asked the
/// user. Evidence now spans every run since the previous check, and "waiting
/// for you" needs a question in the answer.
#[tokio::test(flavor = "multi_thread")]
async fn the_check_sees_work_done_since_the_last_check_and_never_waits_without_a_question() {
    let server = MockServer::start().await;
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
                reply("Starting.").set_delay(Duration::from_millis(1200))
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
    let (sink, mut rx) = ChannelSink::channel();
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
    goal_until(&state, &thread, |status| status == GoalStatus::Reached).await;
    let mut seen = Vec::new();
    until(&mut rx, &mut seen, |event| {
        matches!(
            event,
            ThreadEvent::GoalNote {
                outcome: GoalNoteKind::Reached,
                ..
            }
        )
    })
    .await;
    let outcomes: Vec<_> = seen
        .iter()
        .filter_map(|event| match event {
            ThreadEvent::GoalNote { outcome, .. } => Some(*outcome),
            _ => None,
        })
        .collect();
    assert_eq!(outcomes, [GoalNoteKind::NotYet, GoalNoteKind::Reached]);
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
}

/// Review findings: a queued message refused before any run starts (a bare
/// `/goal`) stays in the queue, held, with its reason in the chat, and the
/// queue moves on; a steer with no run to receive it is refused and the message
/// stays queued.
#[tokio::test(flavor = "multi_thread")]
async fn unsendable_queued_messages_are_recorded_and_the_queue_moves_on() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .respond_with(|request: &Request| {
            if mentions(&body(request), "first") && !mentions(&body(request), "after the bad one") {
                reply("First done.").set_delay(Duration::from_millis(800))
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
}

/// Review findings: a `/goal` whose run cannot start still continues (it
/// retries as a failed turn instead of staying "running" with no turn), and a
/// goal check that fails does not count as a turn without progress.
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
    let (sink, mut rx) = ChannelSink::channel();
    let sink: Arc<dyn themis_desktop::sink::EventSink> = Arc::new(sink);
    // No key yet: the goal's first run fails to start.
    let error = state
        .send_message(sink.clone(), thread.clone(), "/goal finish the work".into())
        .await
        .unwrap_err();
    assert!(!error.is_empty());
    let mut seen = Vec::new();
    until(&mut rx, &mut seen, |event| {
        matches!(
            event,
            ThreadEvent::GoalNote {
                outcome: GoalNoteKind::Failed,
                ..
            }
        )
    })
    .await;
    let goal = state.goal_for(&thread).unwrap();
    assert_eq!(goal.status, GoalStatus::Running);
    assert_eq!(
        goal.failures, 1,
        "counted as a failed turn, retried after a wait"
    );
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
    until(&mut rx, &mut seen, |event| {
        matches!(event, ThreadEvent::GoalNote { outcome: GoalNoteKind::NotYet, reason } if reason.contains("could not decide"))
    })
    .await;
    assert_eq!(
        state.goal_for(&thread).unwrap().stalls,
        0,
        "a failed check is not a stall"
    );
    goal_until(&state, &thread, |status| status == GoalStatus::Reached).await;
}

/// Third review: an error that belongs to the chat (its project's
/// configuration became invalid) holds the first message with its reason and
/// leaves the rest waiting in order, with one note instead of one per message.
#[tokio::test(flavor = "multi_thread")]
async fn a_chat_wide_error_holds_one_message_and_keeps_the_rest_in_order() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .respond_with(|_: &Request| reply("Done.").set_delay(Duration::from_millis(800)))
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
    let mut seen = Vec::new();
    until(&mut rx, &mut seen, |event| {
        matches!(event, ThreadEvent::Failed { error } if error.starts_with("Queued message not sent"))
    })
    .await;
    tokio::time::sleep(Duration::from_millis(300)).await;
    let queue = state.list_queue(thread.clone()).await.unwrap();
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
}
