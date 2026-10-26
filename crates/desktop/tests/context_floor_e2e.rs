//! A budget too small for the work (only possible from runtime.jsonc, for evals: Settings offer
//! 200,000 up): a run whose checkpoints come every request or two stops with advice instead of
//! summarizing forever. Found by the verify eval: at 16k one run made 236 checkpoints in 572 tool
//! calls and finished nothing.

use serde_json::{json, Value};
use std::{
    path::Path,
    sync::{
        atomic::{AtomicUsize, Ordering},
        Arc,
    },
};
use themis_desktop::{
    server::{Client, Server},
    state::AppState,
};
use wiremock::{matchers::method, Mock, MockServer, Request, ResponseTemplate};

fn cli(dir: &Path, command: &str, args: Value) -> Value {
    use std::io::Write;
    use std::process::Stdio;
    let mut child = std::process::Command::new(env!("CARGO_BIN_EXE_themis"))
        .arg("--data-dir")
        .arg(dir)
        .args(["call", command, "-"])
        .env_remove("OPENCODE_KEY")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    child
        .stdin
        .take()
        .unwrap()
        .write_all(args.to_string().as_bytes())
        .unwrap();
    let output = child.wait_with_output().unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    serde_json::from_slice(&output.stdout).unwrap()
}

#[tokio::test(flavor = "multi_thread")]
async fn a_budget_too_small_for_the_work_stops_with_advice() {
    let data = tempfile::tempdir().unwrap();
    let project = tempfile::tempdir().unwrap();
    std::fs::create_dir(project.path().join(".themis")).unwrap();
    std::fs::write(
        project.path().join(".themis/config.jsonc"),
        r#"{"context_token_budget":16000,"recent_messages":2,"total_turns":60}"#,
    )
    .unwrap();
    for index in 0..40 {
        std::fs::write(
            project.path().join(format!("part-{index}.txt")),
            format!("part {index}: notes about the ledger. ").repeat(120),
        )
        .unwrap();
    }
    let provider = MockServer::start().await;
    let reads = Arc::new(AtomicUsize::new(0));
    let counter = Arc::clone(&reads);
    Mock::given(method("POST"))
        .respond_with(move |request: &Request| {
            let body: Value = request.body_json().unwrap();
            if body["messages"][0]["content"]
                .as_str()
                .is_some_and(|text| text.contains("You summarize agent context"))
            {
                return ResponseTemplate::new(200).set_body_json(json!({"choices":[{"message":{"content":"Goal/constraints: read every part. Unresolved work: the rest. Next action: read the next part."},"finish_reason":"stop"}]}));
            }
            // Every request reads another large part, so the context never has room.
            let index = counter.fetch_add(1, Ordering::SeqCst);
            let calls: Vec<Value> = (0..1)
                .map(|offset| json!({"id": format!("read-{index}-{offset}"), "type": "function", "function": {
                    "name": "read_file",
                    "arguments": json!({"file_path": format!("part-{}.txt", (index + offset) % 40)}).to_string()}}))
                .collect();
            ResponseTemplate::new(200).set_body_json(json!({"choices":[{"message":{"content":"Reading the next parts.","tool_calls":calls},"finish_reason":"tool_calls"}]}))
        })
        .mount(&provider)
        .await;
    let state = AppState::new_for_test(data.path().join("settings.json"));
    state
        .set_secret("go".into(), "test-key".into())
        .await
        .unwrap();
    state.set_go_base_url_override(Some(provider.uri()));
    let server = Server::bind(state, data.path()).await.unwrap();
    let serving = tokio::spawn(server.run());
    let client = Client::new(data.path().into());
    let mut events = client.subscribe().await.unwrap();
    let thread = cli(
        data.path(),
        "create_thread",
        json!({"projectRoot":project.path(),"provider":"go","model":"test-model"}),
    );
    let id = thread["id"].as_str().unwrap().to_owned();
    cli(
        data.path(),
        "set_thread_approval_mode",
        json!({"threadId":id,"mode":"yolo"}),
    );
    cli(
        data.path(),
        "send_message",
        json!({"threadId":id,"text":"Read every part file and summarize them."}),
    );
    let mut checkpoints = 0;
    let mut budgets = Vec::new();
    let terminal = tokio::time::timeout(std::time::Duration::from_secs(120), async {
        loop {
            let item = Client::next_event(&mut events).await.unwrap();
            if item["name"] != "thread-event" || item["payload"]["thread_id"] != id {
                continue;
            }
            let event = item["payload"]["event"].clone();
            match event["kind"].as_str().unwrap_or_default() {
                "context_checkpoint" => checkpoints += 1,
                "context_budget" => budgets.push(event["budget"].clone()),
                "finished" | "incomplete" | "failed" => break event,
                _ => {}
            }
        }
    })
    .await
    .unwrap();
    client.call("shutdown", json!({})).await.unwrap();
    let _ = serving.await;
    assert_eq!(terminal["kind"], "failed", "{terminal}");
    let error = terminal["error"].as_str().unwrap();
    assert!(
        error.starts_with("Stopped: the context budget is too small to make progress"),
        "{error}"
    );
    assert!(
        error.contains("raise the context budget to at least"),
        "{error}"
    );
    assert_eq!(checkpoints, 6, "it stops at the sixth close checkpoint");
    // The advice names the room left and a budget that would leave enough, in grouped digits.
    let numbers: Vec<u64> = error
        .split(|c: char| !c.is_ascii_digit() && c != ',')
        .filter_map(|word| word.replace(',', "").parse().ok())
        .collect();
    let (room, minimum) = (numbers[numbers.len() - 2], numbers[numbers.len() - 1]);
    assert!(room < 6000 && minimum > 16000, "{error}");
    assert!(!budgets.is_empty());
}
