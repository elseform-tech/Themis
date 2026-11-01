//! A plugin's background hook reporting back (Claude's `asyncRewake`), through the real shared
//! server and the `themis` CLI with a scripted model: what it reports reaches the agent labelled
//! as the plugin's, it starts a turn in an idle chat only within `hooks.max_wakes`, a hook that
//! reports every turn doesn't loop, the person's message resets the count, and YOLO only waits.
#![cfg(unix)]

use std::path::Path;
use std::time::{Duration, Instant};

use serde_json::{json, Value};
use themis_desktop::{server::Server, state::AppState, types::ProviderKind};
use wiremock::matchers::method;
use wiremock::{Mock, MockServer, ResponseTemplate};

const LABEL: &str = "ran in the background and reported";
const WAKE: &str = "[Plugin hook \u{2014} runtime, not from the user]";

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

fn until(what: &str, mut done: impl FnMut() -> bool) {
    let deadline = Instant::now() + Duration::from_secs(60);
    while !done() {
        assert!(Instant::now() < deadline, "{what}");
        std::thread::sleep(Duration::from_millis(100));
    }
}

struct Chat {
    llm: MockServer,
    data: tempfile::TempDir,
    _project: tempfile::TempDir,
    thread: String,
    serving: tokio::task::JoinHandle<()>,
}

/// A chat whose project has a plugin with one reporting `Stop` hook. Hooks may run without asking
/// (an allow rule in `user_config`), so nothing waits on a person.
async fn chat(command: &str, user_config: &str) -> Chat {
    let llm = MockServer::builder().start().await;
    Mock::given(method("POST"))
        // Slow enough that a test can stop a turn in flight.
        .respond_with(ResponseTemplate::new(200).set_delay(Duration::from_millis(400)).set_body_json(json!({
            "choices": [{"message": {"role": "assistant", "content": "Done."}, "finish_reason": "stop"}]
        })))
        .mount(&llm)
        .await;
    let data = tempfile::tempdir().unwrap();
    let project = tempfile::tempdir().unwrap();
    std::process::Command::new("git")
        .args(["init", "-q"])
        .current_dir(project.path())
        .status()
        .expect("git");
    std::fs::write(data.path().join("runtime.jsonc"), user_config).unwrap();
    let state = AppState::new_for_test(data.path().join("settings.json"));
    state
        .set_secret("go".into(), "synthetic-test-key".into())
        .await
        .unwrap();
    state.set_go_base_url_override(Some(llm.uri()));
    state
        .plugin_action(json!({"action": "save", "scope": "local", "projectRoot": project.path(), "spec": {
            "name": "review",
            "hooks": [{
                "name": "reviewer", "event": "Stop", "command": command, "enabled": true,
                "background": true, "exit_two_blocks": true, "timeout_seconds": 30,
                "rewake": {"message": "Findings of the review:", "summary": "Review found issues"}
            }],
            "files": {}
        }}))
        .await
        .unwrap();
    let thread = state
        .create_thread(
            project.path().to_string_lossy().into(),
            ProviderKind::Go,
            Some("test-model".into()),
        )
        .await
        .unwrap()
        .id;
    let server = Server::bind(state, data.path()).await.unwrap();
    let serving = tokio::spawn(async move {
        let _ = server.run().await;
    });
    Chat {
        llm,
        data,
        _project: project,
        thread,
        serving,
    }
}

const ALLOW_HOOKS: &str = r#"{"approval":{"rules":[{"tool":"hook_*","action":"allow"}]}}"#;

fn send(data: &Path, thread: &str, text: &str) {
    call(
        data,
        "send_message",
        json!({"threadId": thread, "text": text}),
    );
}

fn idle(data: &Path, thread: &str) -> bool {
    call(data, "get_thread", json!({"threadId": thread}))["running"] == false
}

/// The last user message of every request the model got, and each request's whole text.
async fn requests(llm: &MockServer) -> Vec<String> {
    llm.received_requests()
        .await
        .unwrap()
        .iter()
        .map(|request| {
            let body: Value = serde_json::from_slice(&request.body).unwrap();
            body["messages"].to_string()
        })
        .collect()
}

#[tokio::test(flavor = "multi_thread")]
async fn a_report_wakes_an_idle_chat_labelled_and_a_hook_that_always_reports_stops_at_the_limit() {
    // Every turn's end reports again: only the limit stops this.
    let chat = chat("echo found a thing >&2; exit 2", ALLOW_HOOKS).await;
    let (dir, thread) = (chat.data.path().to_owned(), chat.thread.clone());
    tokio::task::spawn_blocking(move || send(&dir, &thread, "first message"))
        .await
        .unwrap();
    // The person's turn and three wakes, and then quiet.
    for _ in 0..600 {
        if chat.llm.received_requests().await.unwrap().len() >= 4 {
            break;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    let (dir, thread) = (chat.data.path().to_owned(), chat.thread.clone());
    tokio::task::spawn_blocking(move || until("still running", || idle(&dir, &thread)))
        .await
        .unwrap();
    tokio::time::sleep(Duration::from_secs(2)).await;
    let seen = requests(&chat.llm).await;
    assert_eq!(seen.len(), 4, "the person's turn and max_wakes: {seen:#?}");
    assert!(!seen[0].contains(LABEL), "{}", seen[0]);
    // A woken turn's message says what it is, the plugin's words fenced in the note.
    assert!(seen[1].contains(WAKE), "{}", seen[1]);
    assert!(seen[1].contains("Review found issues"), "{}", seen[1]);
    assert!(seen[1].contains(LABEL), "{}", seen[1]);
    assert!(seen[1].contains("not from the person"), "{}", seen[1]);
    assert!(
        seen[1].contains("Findings of the review:\\n\\nfound a thing"),
        "{}",
        seen[1]
    );

    // The note the limit held back waits, and the person's next message brings it and resets
    // the count, so the hook reports on again until the limit.
    let (dir, thread) = (chat.data.path().to_owned(), chat.thread.clone());
    tokio::task::spawn_blocking(move || {
        send(&dir, &thread, "second message");
    })
    .await
    .unwrap();
    for _ in 0..600 {
        if chat.llm.received_requests().await.unwrap().len() >= 8 {
            break;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    let (dir, thread) = (chat.data.path().to_owned(), chat.thread.clone());
    tokio::task::spawn_blocking(move || until("still running", || idle(&dir, &thread)))
        .await
        .unwrap();
    tokio::time::sleep(Duration::from_secs(2)).await;
    let seen = requests(&chat.llm).await;
    assert_eq!(seen.len(), 8, "{seen:#?}");
    assert!(seen[4].contains("second message"), "{}", seen[4]);
    assert!(seen[4].contains(LABEL), "the waiting note: {}", seen[4]);
    chat.serving.abort();
}

#[tokio::test(flavor = "multi_thread")]
async fn with_max_wakes_zero_the_report_waits_for_the_next_turn() {
    let config =
        r#"{"approval":{"rules":[{"tool":"hook_*","action":"allow"}]},"hooks":{"max_wakes":0}}"#;
    let chat = chat("echo found a thing >&2; exit 2", config).await;
    let (dir, thread) = (chat.data.path().to_owned(), chat.thread.clone());
    tokio::task::spawn_blocking(move || {
        send(&dir, &thread, "first message");
        until("still running", || idle(&dir, &thread));
    })
    .await
    .unwrap();
    tokio::time::sleep(Duration::from_secs(2)).await;
    assert_eq!(chat.llm.received_requests().await.unwrap().len(), 1);
    let (dir, thread) = (chat.data.path().to_owned(), chat.thread.clone());
    tokio::task::spawn_blocking(move || {
        send(&dir, &thread, "second message");
        until("still running", || idle(&dir, &thread));
    })
    .await
    .unwrap();
    let seen = requests(&chat.llm).await;
    assert_eq!(seen.len(), 2, "{seen:#?}");
    assert!(seen[1].contains(LABEL), "{}", seen[1]);
    assert!(!seen[1].contains(WAKE), "{}", seen[1]);
    chat.serving.abort();
}

#[tokio::test(flavor = "multi_thread")]
async fn in_yolo_a_report_never_starts_a_turn_and_waits_for_the_next() {
    let chat = chat("echo found a thing >&2; exit 2", "{}").await;
    let (dir, thread) = (chat.data.path().to_owned(), chat.thread.clone());
    tokio::task::spawn_blocking(move || {
        call(
            &dir,
            "set_thread_approval_mode",
            json!({"threadId": thread, "mode": "yolo"}),
        );
        send(&dir, &thread, "first message");
        until("still running", || idle(&dir, &thread));
    })
    .await
    .unwrap();
    tokio::time::sleep(Duration::from_secs(2)).await;
    assert_eq!(chat.llm.received_requests().await.unwrap().len(), 1);
    let (dir, thread) = (chat.data.path().to_owned(), chat.thread.clone());
    tokio::task::spawn_blocking(move || {
        send(&dir, &thread, "second message");
        until("still running", || idle(&dir, &thread));
    })
    .await
    .unwrap();
    let seen = requests(&chat.llm).await;
    assert_eq!(seen.len(), 2, "{seen:#?}");
    assert!(seen[1].contains(LABEL), "{}", seen[1]);
    chat.serving.abort();
}

#[tokio::test(flavor = "multi_thread")]
async fn after_the_persons_stop_a_report_starts_nothing_until_they_write() {
    // Waits for a file, so it finishes when the test says: after the second turn was stopped.
    let chat = chat(
        "until [ -f go ]; do sleep 0.1; done; echo found a thing >&2; exit 2",
        ALLOW_HOOKS,
    )
    .await;
    let (dir, thread) = (chat.data.path().to_owned(), chat.thread.clone());
    let go = chat._project.path().join("go");
    tokio::task::spawn_blocking(move || {
        send(&dir, &thread, "first message");
        until("still running", || idle(&dir, &thread));
        send(&dir, &thread, "second message");
        call(&dir, "stop_thread", json!({"threadId": thread}));
        until("still running", || idle(&dir, &thread));
        std::fs::write(go, "").unwrap();
    })
    .await
    .unwrap();
    tokio::time::sleep(Duration::from_secs(2)).await;
    let before = requests(&chat.llm).await.len();
    assert!(before <= 2, "{before}");
    assert!(!requests(&chat.llm)
        .await
        .iter()
        .any(|seen| seen.contains(WAKE)));
    let (dir, thread) = (chat.data.path().to_owned(), chat.thread.clone());
    tokio::task::spawn_blocking(move || {
        send(&dir, &thread, "third message");
        until("still running", || idle(&dir, &thread));
    })
    .await
    .unwrap();
    let seen = requests(&chat.llm).await;
    let third = seen
        .iter()
        .find(|text| text.contains("third message"))
        .unwrap();
    assert!(third.contains(LABEL), "{third}");
    chat.serving.abort();
}

#[tokio::test(flavor = "multi_thread")]
async fn a_hook_that_finishes_quietly_starts_nothing() {
    let chat = chat("echo all fine >&2; exit 0", ALLOW_HOOKS).await;
    let (dir, thread) = (chat.data.path().to_owned(), chat.thread.clone());
    tokio::task::spawn_blocking(move || {
        send(&dir, &thread, "first message");
        until("still running", || idle(&dir, &thread));
    })
    .await
    .unwrap();
    tokio::time::sleep(Duration::from_secs(2)).await;
    assert_eq!(chat.llm.received_requests().await.unwrap().len(), 1);
    assert!(!requests(&chat.llm).await[0].contains(LABEL));
    chat.serving.abort();
}
