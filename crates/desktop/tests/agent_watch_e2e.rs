//! Agent watches: check-command gating, no-approval agent management in the
//! default Custom mode, and the expiry, run-limit, failure and chat backstops.
//! The real `AppState` runs headless against a scripted mock provider.

use std::sync::Arc;
use std::time::Duration;

use serde_json::{json, Value};
use themis_desktop::sink::{ChannelSink, EventSink, TestEvent};
use themis_desktop::state::AppState;
use themis_desktop::types::{
    AutomationInput, AutomationOrigin, ProviderKind, ThreadEvent, WatchSettings,
};
use tokio::sync::mpsc::UnboundedReceiver;
use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, Request, ResponseTemplate};

const DEADLINE: Duration = Duration::from_secs(60);
const PAST: &str = "2001-01-01T00:00:00Z";

fn tool_call(name: &str, arguments: &Value) -> Value {
    json!({"choices":[{"message":{"role":"assistant","content":null,"tool_calls":[{
        "id":"call_1","type":"function","function":{"name":name,"arguments":arguments.to_string()}}]}}]})
}

fn text(answer: &str) -> Value {
    json!({"choices":[{"message":{"role":"assistant","content":answer},"finish_reason":"stop"}]})
}

/// Plain model: always answers with final text.
async fn plain_provider() -> MockServer {
    let server = MockServer::builder().start().await;
    Mock::given(method("POST"))
        .and(path("/chat/completions"))
        .respond_with(ResponseTemplate::new(200).set_body_json(text("looked at it")))
        .mount(&server)
        .await;
    server
}

async fn model_calls(server: &MockServer) -> usize {
    server.received_requests().await.expect("requests").len()
}

struct Fixture {
    state: AppState,
    _data: tempfile::TempDir,
    project: tempfile::TempDir,
    thread: String,
}

async fn fixture(server: &MockServer) -> Fixture {
    let data = tempfile::tempdir().unwrap();
    let project = tempfile::tempdir().unwrap();
    let state = AppState::new_for_test(data.path().join("settings.json"));
    state
        .set_secret("go".into(), "fixture-key".into())
        .await
        .unwrap();
    state.set_go_base_url_override(Some(server.uri()));
    let thread = state
        .create_thread(
            project.path().to_string_lossy().into(),
            ProviderKind::Go,
            Some("fixture".into()),
        )
        .await
        .unwrap();
    Fixture {
        state,
        _data: data,
        project,
        thread: thread.id,
    }
}

fn input(fixture: &Fixture, watch: WatchSettings) -> AutomationInput {
    AutomationInput {
        name: "Watch status".into(),
        project_root: fixture.project.path().to_string_lossy().into(),
        target_thread_id: Some(fixture.thread.clone()),
        provider: ProviderKind::Go,
        model: String::new(),
        reasoning_effort: None,
        skill_ids: vec![],
        interval_mins: 2,
        schedule: None,
        task: "Report the status".into(),
        enabled: true,
        watch,
    }
}

fn check(argv: &[&str]) -> WatchSettings {
    WatchSettings {
        check_command: Some(argv.iter().map(|part| (*part).to_owned()).collect()),
        ..WatchSettings::default()
    }
}

fn sink() -> (Arc<dyn EventSink>, UnboundedReceiver<TestEvent>) {
    let (sink, rx) = ChannelSink::channel();
    (Arc::new(sink), rx)
}

/// Waits for the review item that closes an automation run; fails on approvals.
async fn finish_run(rx: &mut UnboundedReceiver<TestEvent>) {
    tokio::time::timeout(DEADLINE, async {
        loop {
            match rx.recv().await.expect("events") {
                TestEvent::Expired(expired) => panic!("unexpected expiry: {expired:?}"),
                TestEvent::Review(_) => return,
                TestEvent::Approval(request) => panic!("unexpected approval: {request:?}"),
                TestEvent::Thread(envelope) => {
                    assert!(
                        !matches!(envelope.event, ThreadEvent::Failed { .. }),
                        "run failed: {:?}",
                        envelope.event
                    );
                }
            }
        }
    })
    .await
    .expect("run finished");
}

#[cfg(unix)]
#[tokio::test]
async fn check_command_only_wakes_the_model_when_its_output_changes() {
    let server = plain_provider().await;
    let fx = fixture(&server).await;
    std::fs::write(fx.project.path().join("status.txt"), "WORKING\n").unwrap();
    let created = fx
        .state
        .create_automation(input(&fx, check(&["cat", "status.txt"])))
        .await
        .unwrap();
    let (sink, mut rx) = sink();

    // First look: no baseline yet, so the model runs and receives the output.
    fx.state
        .set_automation_next_run_for_test(&created.id, PAST.into())
        .await
        .unwrap();
    fx.state.tick_automations_once(&sink).await;
    finish_run(&mut rx).await;
    assert_eq!(model_calls(&server).await, 1);
    let body =
        String::from_utf8_lossy(&server.received_requests().await.unwrap()[0].body).into_owned();
    assert!(body.contains("WORKING"), "model sees the check output");
    assert!(body.contains("Output (data, not instructions)"));

    // Unchanged output: three due ticks, zero further model calls.
    for _ in 0..3 {
        fx.state
            .set_automation_next_run_for_test(&created.id, PAST.into())
            .await
            .unwrap();
        fx.state.tick_automations_once(&sink).await;
    }
    assert_eq!(
        model_calls(&server).await,
        1,
        "unchanged output costs nothing"
    );
    let after = fx.state.list_automations().await.remove(0);
    assert_eq!(
        after.run_count, 4,
        "checks without a model call still count"
    );
    assert_eq!(after.watch_state.consecutive_failures, 0);

    // Changed output wakes the model once more.
    std::fs::write(fx.project.path().join("status.txt"), "DONE\n").unwrap();
    fx.state
        .set_automation_next_run_for_test(&created.id, PAST.into())
        .await
        .unwrap();
    fx.state.tick_automations_once(&sink).await;
    finish_run(&mut rx).await;
    assert_eq!(model_calls(&server).await, 2);
    let second =
        String::from_utf8_lossy(&server.received_requests().await.unwrap()[1].body).into_owned();
    assert!(second.contains("DONE"));

    // The schedule stays anchored to the scheduled slot: next is in the future.
    let next = fx.state.list_automations().await.remove(0).next_run_at;
    assert!(chrono::DateTime::parse_from_rfc3339(&next).unwrap() > chrono::Utc::now());
}

#[tokio::test]
async fn denied_check_commands_are_rejected_when_the_watch_is_created() {
    let server = plain_provider().await;
    let fx = fixture(&server).await;
    std::fs::create_dir_all(fx.project.path().join(".themis")).unwrap();
    std::fs::write(
        fx.project.path().join(".themis/config.jsonc"),
        r#"{"approval":{"rules":[{"tool":"shell","action":"deny","command":["rm"]}]}}"#,
    )
    .unwrap();
    let error = fx
        .state
        .create_automation(input(&fx, check(&["rm", "-rf", "x"])))
        .await
        .unwrap_err();
    assert!(error.contains("denied by the approval rules"), "{error}");
    fx.state
        .create_automation(input(&fx, check(&["cat", "status.txt"])))
        .await
        .unwrap();
}

#[cfg(unix)]
#[tokio::test]
async fn backstops_remove_or_pause_with_a_reason() {
    let server = plain_provider().await;
    let fx = fixture(&server).await;
    let (sink, _rx) = sink();
    // Allow rules in the user's runtime config let these agent checks run unattended.
    std::fs::write(
        fx._data.path().join("runtime.jsonc"),
        r#"{"approval":{"rules":[
            {"tool":"shell","action":"allow","command":["cat"]},
            {"tool":"shell","action":"allow","command":["definitely-not-a-program-xyz"]}]}}"#,
    )
    .unwrap();

    let expired = fx
        .state
        .create_automation_from(
            input(
                &fx,
                WatchSettings {
                    expires_at: Some(
                        (chrono::Utc::now() + chrono::Duration::seconds(1)).to_rfc3339(),
                    ),
                    ..WatchSettings::default()
                },
            ),
            AutomationOrigin::Agent,
        )
        .await
        .unwrap();
    let limited = fx
        .state
        .create_automation_from(
            input(
                &fx,
                WatchSettings {
                    max_runs: Some(1),
                    check_command: Some(vec!["cat".into(), "status.txt".into()]),
                    ..WatchSettings::default()
                },
            ),
            AutomationOrigin::Agent,
        )
        .await
        .unwrap();
    let failing = fx
        .state
        .create_automation_from(
            input(&fx, check(&["definitely-not-a-program-xyz"])),
            AutomationOrigin::Agent,
        )
        .await
        .unwrap();
    let user_failing = fx
        .state
        .create_automation(input(&fx, check(&["definitely-not-a-program-xyz"])))
        .await
        .unwrap();
    assert_eq!(limited.watch.max_runs, Some(1));

    let deadline = std::time::Instant::now() + Duration::from_secs(10);
    while chrono::DateTime::parse_from_rfc3339(expired.watch.expires_at.as_ref().unwrap()).unwrap()
        > chrono::Utc::now()
    {
        assert!(std::time::Instant::now() < deadline, "expiry never passed");
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    // Agent watches always carry an expiry and a run limit by default.
    assert!(failing.watch.expires_at.is_some() && failing.watch.max_runs == Some(60));

    for _ in 0..3 {
        for id in [&limited.id, &failing.id, &user_failing.id] {
            // A backstop may already have removed it once its last run finished.
            if let Err(error) = fx
                .state
                .set_automation_next_run_for_test(id, PAST.into())
                .await
            {
                assert!(error.contains("unknown automation"), "{error}");
            }
        }
        fx.state.tick_automations_once(&sink).await;
    }
    fx.state.tick_automations_once(&sink).await;

    let tasks = fx.state.get_thread_tasks(&fx.thread).await.unwrap();
    let reason = |id: &str| {
        tasks
            .removed
            .iter()
            .find(|item| item.automation_id == id)
            .map(|item| item.reason.clone())
    };
    assert_eq!(reason(&expired.id).as_deref(), Some("expired"));
    assert_eq!(reason(&failing.id).as_deref(), Some("failed 3 times"));
    assert!(tasks
        .removed
        .iter()
        .all(|item| item.origin == AutomationOrigin::Agent));
    // The run-limited check hit its single run (first look starts the model).
    assert!(model_calls(&server).await <= 2);
    // A user's own automation is paused, never deleted.
    let kept: Vec<_> = tasks
        .automations
        .iter()
        .filter(|item| item.id == user_failing.id)
        .collect();
    assert_eq!(kept.len(), 1);
    assert!(!kept[0].enabled);
    assert_eq!(kept[0].stopped_reason.as_deref(), Some("failed 3 times"));
    // Reasons survive a restart.
    let restarted = AppState::new_for_test(fx._data.path().join("settings.json"));
    let after_restart = restarted.get_thread_tasks(&fx.thread).await.unwrap();
    assert_eq!(after_restart.removed.len(), tasks.removed.len());
}

#[tokio::test]
async fn deleting_the_chat_removes_agent_watches_and_pauses_user_automations() {
    let server = plain_provider().await;
    let fx = fixture(&server).await;
    let (sink, _rx) = sink();
    let user = fx
        .state
        .create_automation(input(&fx, WatchSettings::default()))
        .await
        .unwrap();
    fx.state
        .create_automation_from(
            input(&fx, WatchSettings::default()),
            AutomationOrigin::Agent,
        )
        .await
        .unwrap();
    fx.state.discard_thread(fx.thread.clone()).await.unwrap();
    fx.state.tick_automations_once(&sink).await;
    let left = fx.state.list_automations().await;
    assert_eq!(left.len(), 1, "the agent's watch is gone");
    assert_eq!(left[0].id, user.id);
    assert!(!left[0].enabled);
    assert_eq!(
        left[0].watch_state.stopped_reason.as_deref(),
        Some("chat deleted")
    );
    assert_eq!(model_calls(&server).await, 0);
}

#[cfg(unix)]
#[tokio::test]
async fn a_changed_check_that_cannot_start_the_model_is_not_rerun_every_tick() {
    let server = plain_provider().await;
    let fx = fixture(&server).await;
    let (sink, _rx) = sink();
    // A new-chat automation with a reasoning effort fails before spawning
    // (no model catalog in this fixture): the fire step errors after the check ran.
    let mut watch = input(
        &fx,
        check(&["sh", "-c", "echo x >> count.txt; echo changed"]),
    );
    watch.target_thread_id = None;
    watch.model = "fixture".into();
    watch.reasoning_effort = Some("high".into());
    let created = fx.state.create_automation(watch).await.unwrap();
    fx.state
        .set_automation_next_run_for_test(&created.id, PAST.into())
        .await
        .unwrap();
    for _ in 0..4 {
        fx.state.tick_automations_once(&sink).await;
    }
    let runs = std::fs::read_to_string(fx.project.path().join("count.txt")).unwrap();
    assert_eq!(runs.lines().count(), 1, "the check ran once for this slot");
    let after = fx.state.list_automations().await.remove(0);
    assert_eq!(after.run_count, 1);
    assert!(chrono::DateTime::parse_from_rfc3339(&after.next_run_at).unwrap() > chrono::Utc::now());
}

/// Scripted model: creates a watch on the first user turn and completes it
/// when a watch run (whose prompt carries the watch id) asks.
async fn scripted_agent(with_check: bool) -> MockServer {
    let server = MockServer::builder().start().await;
    Mock::given(method("POST"))
        .and(path("/chat/completions"))
        .respond_with(move |request: &Request| {
            let body: Value = serde_json::from_slice(&request.body).unwrap_or(Value::Null);
            let messages = body["messages"].as_array().cloned().unwrap_or_default();
            let last = messages.last().cloned().unwrap_or(Value::Null);
            let users: Vec<String> = messages
                .iter()
                .filter(|message| message["role"] == "user")
                .filter_map(|message| message["content"].as_str().map(str::to_owned))
                .collect();
            let prompt = users.last().cloned().unwrap_or_default();
            let payload = if last["role"] == "tool" {
                // A real end-of-turn summary: a one-line "done" after tools is asked again.
                text(
                    "Set up the watch on status.txt.\nIt checks every 2 minutes and stops at DONE.",
                )
            } else if let Some(start) = prompt.find("[Watch ") {
                let id: String = prompt[start + 7..]
                    .chars()
                    .take_while(|c| *c != ']')
                    .collect();
                tool_call("manage_automations", &json!({"action":"complete","id":id}))
            } else {
                let mut input = json!({
                    "name":"Watch status.txt","task":"Read status.txt and report it",
                    "interval_mins":2,"stop_when":"status.txt says DONE"});
                if with_check {
                    input["check_command"] = json!(["cat", "status.txt"]);
                }
                tool_call(
                    "manage_automations",
                    &json!({"action":"create","input":input}),
                )
            };
            ResponseTemplate::new(200).set_body_json(payload)
        })
        .mount(&server)
        .await;
    server
}

#[tokio::test]
async fn agent_creates_and_completes_a_watch_without_any_approval_in_custom_mode() {
    let server = scripted_agent(false).await;
    let fx = fixture(&server).await;
    std::fs::write(fx.project.path().join("status.txt"), "WORKING\n").unwrap();
    let (sink, mut rx) = sink();

    // The user asks in plain words; the default mode is Custom.
    fx.state
        .send_message(
            Arc::clone(&sink),
            fx.thread.clone(),
            "watch status.txt".into(),
        )
        .await
        .unwrap();
    tokio::time::timeout(DEADLINE, async {
        loop {
            match rx.recv().await.expect("events") {
                TestEvent::Expired(expired) => panic!("unexpected expiry: {expired:?}"),
                TestEvent::Approval(request) => panic!("agent was asked to approve: {request:?}"),
                TestEvent::Thread(envelope) => match envelope.event {
                    ThreadEvent::Finished { .. } => return,
                    ThreadEvent::Failed { error } => panic!("run failed: {error}"),
                    _ => {}
                },
                TestEvent::Review(_) => {}
            }
        }
    })
    .await
    .expect("chat run finished");
    let watches = fx.state.list_automations().await;
    assert_eq!(watches.len(), 1);
    let watch = &watches[0];
    assert_eq!(watch.origin, AutomationOrigin::Agent);
    assert_eq!(watch.interval_mins, 2);
    assert!(watch.schedule.is_none());
    assert_eq!(watch.target_thread_id.as_deref(), Some(fx.thread.as_str()));
    assert!(watch.watch.expires_at.is_some());
    assert_eq!(watch.watch.max_runs, Some(60));
    assert_eq!(
        watch.watch.stop_when.as_deref(),
        Some("status.txt says DONE")
    );

    // First scheduled look: the model runs, sees the watch id, and completes it.
    fx.state
        .set_automation_next_run_for_test(&watch.id, PAST.into())
        .await
        .unwrap();
    fx.state.tick_automations_once(&sink).await;
    finish_run(&mut rx).await;
    assert!(
        fx.state.list_automations().await.is_empty(),
        "watch completed"
    );
    let tasks = fx.state.get_thread_tasks(&fx.thread).await.unwrap();
    assert_eq!(tasks.removed[0].reason, "completed by agent");
    assert_eq!(tasks.removed[0].origin, AutomationOrigin::Agent);
    assert_eq!(
        tasks.removed[0].stop_when.as_deref(),
        Some("status.txt says DONE")
    );
    // The finished run keeps a stable name and origin after deletion.
    assert_eq!(tasks.recent[0].name, "Watch status.txt");
    assert_eq!(tasks.recent[0].origin, AutomationOrigin::Agent);
}

#[tokio::test]
async fn an_agent_check_command_needs_the_users_answer_in_a_git_project() {
    if std::process::Command::new("git")
        .arg("--version")
        .output()
        .is_err()
    {
        return;
    }
    let server = scripted_agent(true).await;
    let fx = fixture(&server).await;
    assert!(std::process::Command::new("git")
        .args(["init", "-q"])
        .current_dir(fx.project.path())
        .status()
        .unwrap()
        .success());
    // The repository flag is recorded when a chat is created.
    let git_thread = fx
        .state
        .create_thread(
            fx.project.path().to_string_lossy().into(),
            ProviderKind::Go,
            Some("fixture".into()),
        )
        .await
        .unwrap()
        .id;
    std::fs::write(fx.project.path().join("status.txt"), "WORKING\n").unwrap();
    let (sink, mut rx) = sink();
    fx.state
        .send_message(
            Arc::clone(&sink),
            git_thread.clone(),
            "watch status.txt".into(),
        )
        .await
        .unwrap();
    let mut asked = None;
    tokio::time::timeout(DEADLINE, async {
        loop {
            match rx.recv().await.expect("events") {
                TestEvent::Expired(expired) => panic!("unexpected expiry: {expired:?}"),
                TestEvent::Approval(request) => {
                    asked = Some(request.clone());
                    fx.state
                        .approve_action(
                            request.thread_id,
                            request.approval_id,
                            themis_desktop::types::ApprovalDecision::Deny,
                        )
                        .await
                        .unwrap();
                }
                TestEvent::Thread(envelope) => match envelope.event {
                    ThreadEvent::Finished { .. } => return,
                    ThreadEvent::Failed { error } => panic!("run failed: {error}"),
                    _ => {}
                },
                TestEvent::Review(_) => {}
            }
        }
    })
    .await
    .expect("chat run finished");
    let asked = asked.expect("the user was asked about the exact command");
    assert_eq!(asked.tool, "shell");
    // The exact argv travels as structured tokens, never as a joined string.
    assert_eq!(asked.command.unwrap().argv, ["cat", "status.txt"]);
    assert!(
        !asked.summary.contains("cat status.txt"),
        "{}",
        asked.summary
    );
    assert!(
        fx.state.list_automations().await.is_empty(),
        "denied: nothing was saved"
    );
}

#[tokio::test]
async fn an_agent_check_command_is_refused_like_shell_outside_a_git_project() {
    let server = scripted_agent(true).await;
    let fx = fixture(&server).await;
    std::fs::write(fx.project.path().join("status.txt"), "WORKING\n").unwrap();
    let (sink, mut rx) = sink();
    fx.state
        .send_message(
            Arc::clone(&sink),
            fx.thread.clone(),
            "watch status.txt".into(),
        )
        .await
        .unwrap();
    tokio::time::timeout(DEADLINE, async {
        loop {
            match rx.recv().await.expect("events") {
                TestEvent::Expired(expired) => panic!("unexpected expiry: {expired:?}"),
                TestEvent::Approval(request) => panic!("no prompt expected: {request:?}"),
                TestEvent::Thread(envelope) => match envelope.event {
                    ThreadEvent::Finished { .. } => return,
                    ThreadEvent::Failed { error } => panic!("run failed: {error}"),
                    _ => {}
                },
                TestEvent::Review(_) => {}
            }
        }
    })
    .await
    .expect("chat run finished");
    assert!(fx.state.list_automations().await.is_empty());
}
