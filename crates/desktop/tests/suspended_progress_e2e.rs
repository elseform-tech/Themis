//! Failed checkpoint progress must survive restart without replaying completed writes.
use serde_json::{json, Value};
use std::{
    path::Path,
    sync::{
        atomic::{AtomicUsize, Ordering},
        Arc, Mutex,
    },
};
use themis_desktop::{
    server::{Client, Server},
    state::AppState,
};
use wiremock::{matchers::method, Mock, MockServer, Request, ResponseTemplate};

fn cli(data: &Path, method: &str, args: Value) -> Value {
    let output = std::process::Command::new(env!("CARGO_BIN_EXE_themis"))
        .args([
            "--data-dir",
            data.to_str().unwrap(),
            "call",
            method,
            &args.to_string(),
        ])
        .env_remove("OPENCODE_KEY")
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    serde_json::from_slice(&output.stdout).unwrap()
}

async fn terminal(
    events: &mut tokio::net::TcpStream,
    id: &str,
    allow_checkpoint: bool,
) -> (Value, Option<String>) {
    tokio::time::timeout(std::time::Duration::from_secs(60), async {
        let mut progress = None;
        loop {
            let item = Client::next_event(events).await.unwrap();
            if item["name"] == "thread-event" && item["payload"]["thread_id"] == id {
                let event = &item["payload"]["event"];
                if event["kind"] == "context_suspended" {
                    assert!(progress.is_none(), "duplicate suspended progress event");
                    progress = Some(event["progress"].as_str().unwrap().to_owned());
                }
                if !allow_checkpoint {
                    assert_ne!(
                        event["kind"], "context_checkpoint",
                        "failed summaries must not install a checkpoint"
                    );
                }
                if ["finished", "failed", "incomplete"].contains(&event["kind"].as_str().unwrap()) {
                    return (event.clone(), progress);
                }
            }
        }
    })
    .await
    .unwrap()
}

fn append_call() -> Value {
    json!({"id":"append","type":"function","function":{"name":"shell","arguments":json!({"command":"python3","args":["-c","from pathlib import Path; p=Path('completed.txt'); p.open('a', newline='').write('once\\n'); print('APPEND_DONE_753')"]}).to_string()}})
}

#[tokio::test(flavor = "multi_thread")]
async fn failed_checkpoint_delivers_completed_action_after_restart() {
    check_restart(false).await;
}

#[tokio::test(flavor = "multi_thread")]
async fn suspended_action_survives_a_later_lossy_checkpoint() {
    check_restart(true).await;
}

async fn check_restart(checkpoint_after_restart: bool) {
    let data = tempfile::tempdir().unwrap();
    let project = tempfile::tempdir().unwrap();
    std::fs::create_dir(project.path().join(".themis")).unwrap();
    std::fs::write(
        project.path().join(".themis/config.jsonc"),
        r#"{"context_token_budget":24000,"recent_messages":2,"total_turns":40}"#,
    )
    .unwrap();
    std::fs::write(
        project.path().join("source.txt"),
        "source observation\n".repeat(9000),
    )
    .unwrap();
    let provider = MockServer::builder().start().await;
    let ordinary = Arc::new(AtomicUsize::new(0));
    let summaries = Arc::new(AtomicUsize::new(0));
    let calls = ordinary.clone();
    let attempts = summaries.clone();
    Mock::given(method("POST")).respond_with(move |request: &Request| {
        let body: Value = request.body_json().unwrap();
        if body["messages"][0]["content"].as_str().unwrap().contains("You summarize agent context") {
            attempts.fetch_add(1, Ordering::SeqCst);
            return ResponseTemplate::new(503).set_body_json(json!({"error":{"message":"temporary test outage"}}));
        }
        let index = calls.fetch_add(1, Ordering::SeqCst);
        let call = if index == 0 { append_call() } else {
            json!({"id":format!("read-{index}"),"type":"function","function":{"name":"read_file","arguments":json!({"file_path":"source.txt","offset":(index-1)*4000}).to_string()}})
        };
        ResponseTemplate::new(200).set_body_json(json!({"choices":[{"message":{"content":"Inspect the next source range","tool_calls":[call]},"finish_reason":"tool_calls"}]}))
    }).mount(&provider).await;
    let state = AppState::new_for_test(data.path().join("settings.json"));
    state
        .set_secret("go".into(), "test-key".into())
        .await
        .unwrap();
    state.set_go_base_url_override(Some(provider.uri()));
    let server = Server::bind(state.clone(), data.path()).await.unwrap();
    let serving = tokio::spawn(server.run());
    let client = Client::new(data.path().into());
    let thread = cli(
        data.path(),
        "create_thread",
        json!({"projectRoot":project.path(),"provider":"go","model":"test-model"}),
    );
    let id = thread["id"].as_str().unwrap();
    cli(
        data.path(),
        "set_thread_approval_mode",
        json!({"threadId":id,"mode":"yolo"}),
    );
    let mut events = client.subscribe().await.unwrap();
    cli(
        data.path(),
        "send_message",
        json!({"threadId":id,"text":"Append once, inspect source.txt, then report."}),
    );
    let (failed, progress) = terminal(&mut events, id, false).await;
    assert_eq!(failed["kind"], "failed", "{failed}");
    let progress = progress.expect("progress must be published before the terminal failure");
    let action: Value = progress
        .lines()
        .filter_map(|line| serde_json::from_str::<Value>(line).ok())
        .find(|item| item["tool"] == "shell")
        .expect("completed shell receipt");
    let source = &action["source"];
    let snapshot = project.path().join(source["path"].as_str().unwrap());
    assert!(snapshot
        .canonicalize()
        .unwrap()
        .starts_with(project.path().canonicalize().unwrap()));
    let original: Value = serde_json::from_str(
        std::fs::read_to_string(&snapshot)
            .unwrap()
            .lines()
            .nth(source["record"].as_u64().unwrap() as usize - 1)
            .unwrap(),
    )
    .unwrap();
    assert!(
        original
            .pointer(source["json_pointer"].as_str().unwrap())
            .unwrap()
            .as_str()
            .unwrap()
            .contains("APPEND_DONE_753"),
        "receipt must address the actual immutable tool observation"
    );
    assert!(failed["error"]
        .as_str()
        .unwrap()
        .contains("checkpoint failed"));
    assert_eq!(summaries.load(Ordering::SeqCst), 3);
    assert_eq!(
        std::fs::read_to_string(project.path().join("completed.txt")).unwrap(),
        "once\n"
    );
    client.call("shutdown", json!({})).await.unwrap();
    serving.await.unwrap().unwrap();
    drop(state);
    provider.reset().await;
    let received = Arc::new(Mutex::new(Vec::<Value>::new()));
    let capture = received.clone();
    let restart_summary = Arc::new(AtomicUsize::new(0));
    let checkpoint_seen = restart_summary.clone();
    let reads = AtomicUsize::new(0);
    Mock::given(method("POST")).respond_with(move |request: &Request| {
        let body: Value = request.body_json().unwrap();
        capture.lock().unwrap().push(body.clone());
        if body["messages"][0]["content"].as_str().unwrap().contains("You summarize agent context") {
            checkpoint_seen.fetch_add(1, Ordering::SeqCst);
            return ResponseTemplate::new(200).set_body_json(json!({"choices":[{"message":{"content":"Goal/constraints: continue. Confirmed findings: read source. Completed changes/tests: source inspection. Unresolved work: report. Next action: finish."},"finish_reason":"stop"}]}));
        }
        let current = body["messages"].as_array().unwrap().iter().rev().find(|m| m["role"] == "user").unwrap()["content"].as_str().unwrap();
        let progress_present = body["messages"].to_string().contains("APPEND_DONE_753");
        let tool_returned = body["messages"].as_array().unwrap().iter().any(|m|m["role"]=="tool");
        let message = if current == "Continue." && !progress_present && !tool_returned {
            json!({"content":"Append before finishing","tool_calls":[append_call()]})
        } else if current == "Continue." && checkpoint_after_restart && checkpoint_seen.load(Ordering::SeqCst) == 0 {
            let index = reads.fetch_add(1, Ordering::SeqCst);
            json!({"content":"Inspect another range before reporting","tool_calls":[{"id":format!("pressure-{index}"),"type":"function","function":{"name":"read_file","arguments":json!({"file_path":"source.txt","offset":80000+index*4000}).to_string()}}]})
        } else { json!({"content":if current == "Continue." {"Resumed without repeating the append"} else {"Unrelated task finished"}}) };
        ResponseTemplate::new(200).set_body_json(json!({"choices":[{"finish_reason":if message.get("tool_calls").is_some(){"tool_calls"}else{"stop"},"message":message}]}))
    }).mount(&provider).await;
    let state = AppState::new_for_test(data.path().join("settings.json"));
    state
        .set_secret("go".into(), "test-key".into())
        .await
        .unwrap();
    state.set_go_base_url_override(Some(provider.uri()));
    let server = Server::bind(state, data.path()).await.unwrap();
    let serving = tokio::spawn(server.run());
    let mut events = client.subscribe().await.unwrap();
    cli(
        data.path(),
        "send_message",
        json!({"threadId":id,"text":"Continue."}),
    );
    let (resumed, progress) = terminal(&mut events, id, checkpoint_after_restart).await;
    assert_eq!(resumed["kind"], "finished", "{resumed}");
    assert!(
        progress.is_none(),
        "successful run must not emit suspended progress"
    );
    let requests = received.lock().unwrap().clone();
    assert!(
        requests[0]["messages"]
            .to_string()
            .contains("APPEND_DONE_753"),
        "the first continuation request omitted completed action progress"
    );
    assert_eq!(
        std::fs::read_to_string(project.path().join("completed.txt")).unwrap(),
        "once\n",
        "completed append was replayed"
    );
    if checkpoint_after_restart {
        assert!(
            restart_summary.load(Ordering::SeqCst) > 0,
            "real semantic checkpoint required"
        );
        let after = requests
            .iter()
            .rev()
            .find(|packet| {
                !packet["messages"][0]["content"]
                    .as_str()
                    .unwrap()
                    .contains("You summarize agent context")
            })
            .unwrap();
        assert!(
            after["messages"].to_string().contains("APPEND_DONE_753"),
            "the next checkpoint lost suspended completion progress"
        );
    } else {
        assert_eq!(
            requests.len(),
            1,
            "continuation should not execute another tool"
        );
    }
    assert_eq!(requests[0]["messages"][0]["role"], "system");
    cli(
        data.path(),
        "send_message",
        json!({"threadId":id,"text":"Explain a new unrelated topic; do not resume the old task."}),
    );
    let (unrelated, progress) = terminal(&mut events, id, false).await;
    assert_eq!(unrelated["kind"], "finished");
    assert!(progress.is_none());
    let requests = received.lock().unwrap().clone();
    let packet = requests.last().unwrap();
    let last_user = packet["messages"]
        .as_array()
        .unwrap()
        .iter()
        .rev()
        .find(|m| m["role"] == "user")
        .unwrap();
    assert_eq!(
        last_user["content"],
        "Explain a new unrelated topic; do not resume the old task."
    );
    assert!(!packet["messages"][0]["content"]
        .as_str()
        .unwrap()
        .contains("APPEND_DONE_753"));
    assert_eq!(
        std::fs::read_to_string(project.path().join("completed.txt")).unwrap(),
        "once\n"
    );
    client.call("shutdown", json!({})).await.unwrap();
    serving.await.unwrap().unwrap();
}
