//! What a request is made of, the same on every platform: the system message's pieces and how
//! large tool output enters the context. Found by a Windows-only budget failure: the same
//! scripted run had a 2,134-token system message and 9,959 tokens of tool activity on Windows
//! against 8,273 and 3,272 on macOS.

use serde_json::{json, Value};
use std::{
    path::Path,
    sync::{Arc, Mutex},
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

/// The request after three large shell outputs, as the provider received it.
async fn request_after_large_output() -> Value {
    let data = tempfile::tempdir().unwrap();
    let project = tempfile::tempdir().unwrap();
    std::fs::create_dir(project.path().join(".themis")).unwrap();
    std::fs::write(
        project.path().join(".themis/config.jsonc"),
        r#"{"context_token_budget":24000,"recent_messages":2,"total_turns":12}"#,
    )
    .unwrap();
    let provider = MockServer::start().await;
    let seen: Arc<Mutex<Vec<Value>>> = Arc::default();
    let recorded = Arc::clone(&seen);
    Mock::given(method("POST"))
        .respond_with(move |request: &Request| {
            let body: Value = request.body_json().unwrap();
            let mut seen = recorded.lock().unwrap();
            seen.push(body);
            if seen.len() == 1 {
                let calls: Vec<Value> = ['a', 'b', 'c']
                    .iter()
                    .map(|fill| {
                        json!({"id": format!("shell-{fill}"), "type": "function", "function": {
                            "name": "shell",
                            "arguments": json!({"command": "python3", "args": ["-c", format!("print('{fill}'*11000)")], "cwd": ""}).to_string()}})
                    })
                    .collect();
                return ResponseTemplate::new(200).set_body_json(json!({"choices":[{"message":{"content":"Gathering output.","tool_calls":calls},"finish_reason":"tool_calls"}]}));
            }
            ResponseTemplate::new(200).set_body_json(
                json!({"choices":[{"message":{"content":"Done."},"finish_reason":"stop"}]}),
            )
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
        json!({"threadId":id,"text":"Gather the output."}),
    );
    tokio::time::timeout(std::time::Duration::from_secs(60), async {
        loop {
            let item = Client::next_event(&mut events).await.unwrap();
            let kind = item["payload"]["event"]["kind"]
                .as_str()
                .unwrap_or_default();
            if item["name"] == "thread-event"
                && item["payload"]["thread_id"] == id
                && ["finished", "incomplete", "failed"].contains(&kind)
            {
                break;
            }
        }
    })
    .await
    .unwrap();
    client.call("shutdown", json!({})).await.unwrap();
    let _ = serving.await;
    let seen = seen.lock().unwrap();
    assert!(seen.len() >= 2, "the run made {} requests", seen.len());
    seen[1].clone()
}

/// {"system": [(first line, chars)], "tools": [(chars, first 80 chars)], "tool_schema_chars"}.
fn anatomy(body: &Value) -> Value {
    let messages = body["messages"].as_array().unwrap();
    let system: Vec<Value> = messages
        .iter()
        .filter(|message| message["role"] == "system")
        .flat_map(|message| {
            message["content"]
                .as_str()
                .unwrap_or_default()
                .split("\n\n")
                .map(|piece| {
                    json!([
                        piece
                            .lines()
                            .next()
                            .unwrap_or_default()
                            .chars()
                            .take(70)
                            .collect::<String>(),
                        piece.chars().count()
                    ])
                })
                .collect::<Vec<_>>()
        })
        .collect();
    let tools: Vec<Value> = messages
        .iter()
        .filter(|message| message["role"] == "tool")
        .map(|message| {
            let content = message["content"].as_str().unwrap_or_default();
            json!([
                content.chars().count(),
                content.chars().take(80).collect::<String>()
            ])
        })
        .collect();
    json!({"system": system, "tools": tools, "tool_schema_chars": body["tools"].to_string().len()})
}

#[tokio::test(flavor = "multi_thread")]
async fn large_output_and_the_system_message_are_the_same_on_every_platform() {
    let body = request_after_large_output().await;
    let report = anatomy(&body);
    eprintln!("CONTEXT_ANATOMY {report}");
    // Personal skills (~/.agents, ~/.codex, ~/.claude) never reach a test's system message.
    assert!(
        !body.to_string().contains("--discovered-"),
        "a personal skill reached the test's system message: {report}"
    );
    let tools = report["tools"].as_array().unwrap();
    assert_eq!(tools.len(), 3, "{report}");
    // 11,000 characters of output at a 24k budget: each result must reach the model reduced.
    for tool in tools {
        assert!(
            tool[0].as_u64().unwrap() < 6000,
            "a large output reached the model whole: {report}"
        );
    }
}
