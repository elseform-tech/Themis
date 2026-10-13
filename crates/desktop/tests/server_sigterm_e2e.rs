//! SIGTERM must stop the shared server and the background programs it owns,
//! even while clients keep connecting (a signal between accepts or during
//! cleanup must never be dropped). One test per binary: it signals this process.
#![cfg(unix)]

use std::sync::Arc;
use std::time::{Duration, Instant};

use serde_json::{json, Value};
use themis_desktop::server::Server;
use themis_desktop::sink::{ChannelSink, EventSink, TestEvent};
use themis_desktop::state::AppState;
use themis_desktop::types::{ApprovalDecision, ProviderKind, ThreadEvent};
use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, Request, ResponseTemplate};

#[allow(dead_code)]
mod common;

fn alive(pid: u64) -> bool {
    std::process::Command::new("kill")
        .args(["-0", &pid.to_string()])
        .stderr(std::process::Stdio::null())
        .status()
        .is_ok_and(|status| status.success())
}

#[tokio::test(flavor = "multi_thread")]
async fn sigterm_under_connection_churn_stops_the_server_and_its_background_task() {
    assert!(common::git_available(), "git is required");
    let (_repo, project) = common::init_repo();
    let data = tempfile::tempdir().unwrap();
    let state = AppState::new_for_test(data.path().join("settings.json"));
    let provider = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/chat/completions"))
        .respond_with(|request: &Request| {
            let body: Value = serde_json::from_slice(&request.body).unwrap_or(Value::Null);
            let answered = body["messages"]
                .as_array()
                .is_some_and(|m| m.iter().any(|m| m["role"] == "tool"));
            ResponseTemplate::new(200).set_body_json(if answered {
                json!({"choices":[{"message":{"role":"assistant","content":"done"},"finish_reason":"stop"}]})
            } else {
                json!({"choices":[{"message":{"role":"assistant","content":null,"tool_calls":[{
                    "id":"c0","type":"function","function":{"name":"shell","arguments":
                    json!({"command":"sh","args":["-c","exec sleep 3160"],"background":true}).to_string()}}]}}]})
            })
        })
        .mount(&provider)
        .await;
    common::use_mock_llm(&state, &provider).await;
    let thread = state
        .create_thread(
            project.to_string_lossy().into(),
            ProviderKind::Go,
            Some("test-model".to_owned()),
        )
        .await
        .unwrap();
    let server = Server::bind(state.clone(), data.path()).await.unwrap();
    let serving = tokio::spawn(server.run());

    let (sink, mut rx) = ChannelSink::channel();
    state
        .send_message(
            Arc::new(sink) as Arc<dyn EventSink>,
            thread.id.clone(),
            "go".into(),
        )
        .await
        .unwrap();
    loop {
        match tokio::time::timeout(Duration::from_secs(60), rx.recv())
            .await
            .unwrap()
            .unwrap()
        {
            TestEvent::Approval(request) => state
                .approve_action(
                    thread.id.clone(),
                    request.approval_id,
                    ApprovalDecision::Once,
                )
                .await
                .unwrap(),
            TestEvent::Thread(envelope)
                if matches!(envelope.event, ThreadEvent::Finished { .. }) =>
            {
                break
            }
            _ => {}
        }
    }
    let tasks = state
        .get_thread_tasks(&thread.id)
        .await
        .unwrap()
        .background_tasks;
    let pid = u64::from(tasks[0].pid);
    assert!(alive(pid));

    // Churn: clients connect continuously while the signal arrives.
    let data_dir = data.path().to_path_buf();
    // Stops before the server is gone: a call after that would make the CLI
    // auto-start a detached server.
    let churning = Arc::new(std::sync::atomic::AtomicBool::new(true));
    let keep_going = Arc::clone(&churning);
    let churn = std::thread::spawn(move || {
        let deadline = Instant::now() + Duration::from_secs(8);
        while Instant::now() < deadline && keep_going.load(std::sync::atomic::Ordering::SeqCst) {
            let _ = std::process::Command::new(env!("CARGO_BIN_EXE_themis"))
                .arg("--data-dir")
                .arg(&data_dir)
                .env_remove("OPENCODE_KEY")
                .args(["call", "list_automations", "{}"])
                .output();
        }
    });
    tokio::time::sleep(Duration::from_millis(700)).await;
    let me = std::process::id().to_string();
    assert!(std::process::Command::new("kill")
        .args(["-TERM", &me])
        .status()
        .unwrap()
        .success());

    let finished = tokio::time::timeout(Duration::from_secs(15), serving).await;
    churning.store(false, std::sync::atomic::Ordering::SeqCst);
    assert!(finished.is_ok(), "SIGTERM did not stop the server");
    let deadline = Instant::now() + Duration::from_secs(10);
    while alive(pid) {
        assert!(
            Instant::now() < deadline,
            "background task survived the server"
        );
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
    churn.join().unwrap();
    // A call in flight when the server died may have auto-started a fresh one.
    let _ = std::process::Command::new(env!("CARGO_BIN_EXE_themis"))
        .arg("--data-dir")
        .arg(data.path())
        .args(["server", "stop"])
        .env_remove("OPENCODE_KEY")
        .output();
    tokio::time::sleep(Duration::from_secs(1)).await;
    let leftover = std::process::Command::new("pgrep")
        .arg("-f")
        .arg(data.path().to_string_lossy().as_ref())
        .output()
        .is_ok_and(|output| output.status.success());
    assert!(!leftover, "a detached server was left running");
}
