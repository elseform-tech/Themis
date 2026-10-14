//! Shared server + CLI: `recall` is offered in a real run, needs no approval under
//! the default Custom mode, and returns citable coordinates for an attachment original
//! that never fit in context. The provider is scripted: this checks the wiring, not
//! live model quality.

use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::{path::Path, sync::Arc};
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

async fn finish(events: &mut tokio::net::TcpStream, id: &str) -> (Value, usize) {
    let mut approvals = 0;
    let terminal = tokio::time::timeout(std::time::Duration::from_secs(90), async {
        loop {
            let item = Client::next_event(events).await.unwrap();
            if item["name"] == "approval-request" {
                approvals += 1;
            }
            if item["name"] == "thread-event" && item["payload"]["thread_id"] == id {
                let event = &item["payload"]["event"];
                if event["kind"] == "finished" || event["kind"] == "failed" {
                    break event.clone();
                }
            }
        }
    })
    .await
    .unwrap();
    (terminal, approvals)
}

#[tokio::test(flavor = "multi_thread")]
async fn cli_recall_returns_attachment_original_coordinates_without_approval() {
    let data = tempfile::tempdir().unwrap();
    let project = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(project.path().join(".themis")).unwrap();
    std::fs::write(
        project.path().join(".themis/config.jsonc"),
        r#"{"context_token_budget":24000,"recent_messages":4,"total_turns":8}"#,
    )
    .unwrap();
    let outside = tempfile::tempdir().unwrap();
    let source = outside.path().join("notes.dat");
    let body = format!(
        "{}\nThe dock code for the north pier is MARLIN-5521.\n{}",
        "tide table entry; ".repeat(3000),
        "harbour log entry; ".repeat(3000)
    );
    std::fs::write(&source, &body).unwrap();

    let provider = MockServer::start().await;
    let requests_seen = Arc::new(std::sync::Mutex::new(Vec::<Value>::new()));
    let sink = requests_seen.clone();
    Mock::given(method("POST"))
        .respond_with(move |request: &Request| {
            let body: Value = request.body_json().unwrap();
            if body["messages"][0]["content"]
                .as_str()
                .is_some_and(|t| t.contains("You summarize agent context"))
            {
                return ResponseTemplate::new(200).set_body_json(json!({"choices":[{"message":{"content":"Goal/constraints: answer questions. Confirmed findings: a notes file was attached. Completed changes/tests: none. Unresolved work: none. Next action: answer."},"finish_reason":"stop"}]}));
            }
            sink.lock().unwrap().push(body.clone());
            let messages = body["messages"].as_array().unwrap();
            let last = messages.last().unwrap();
            let message = if last["role"] == "tool" {
                let result = last["content"].as_str().unwrap_or_default();
                json!({"content": format!("CODE {}", if result.contains("MARLIN-5521") { "MARLIN-5521" } else { "unknown" })})
            } else if last["content"].as_str().is_some_and(|t| t.contains("dock code?")) {
                json!({"tool_calls":[{"id":"r1","type":"function","function":{"name":"recall","arguments":json!({"query":"dock code north pier"}).to_string()}}]})
            } else {
                json!({"content":"noted"})
            };
            ResponseTemplate::new(200).set_body_json(json!({"choices":[{"message":message,"finish_reason":"stop"}]}))
        })
        .mount(&provider)
        .await;

    let state = AppState::new_for_test(data.path().join("settings.json"));
    state.set_secret("go".into(), "test-key".into()).await.unwrap();
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
    let id = thread["id"].as_str().unwrap();
    let attached = cli(
        data.path(),
        "attach_files",
        json!({"threadId":id,"paths":[source]}),
    );
    let stored = attached[0]["path"].as_str().unwrap().to_owned();
    cli(
        data.path(),
        "send_message",
        json!({"threadId":id,"text":"Here are the harbour notes.","attachments":[stored]}),
    );
    let (first, _) = finish(&mut events, id).await;
    assert_eq!(first["kind"], "finished", "{first}");

    cli(
        data.path(),
        "send_message",
        json!({"threadId":id,"text":"What is the dock code?"}),
    );
    let (second, approvals) = finish(&mut events, id).await;
    assert_eq!(second["kind"], "finished", "{second}");
    assert_eq!(second["result"], "CODE MARLIN-5521");
    assert_eq!(approvals, 0, "recall is a Read tool and needs no approval");

    let seen = requests_seen.lock().unwrap().clone();
    let last = seen.last().unwrap();
    assert!(last["tools"]
        .as_array()
        .unwrap()
        .iter()
        .any(|t| t["function"]["name"] == "recall"));
    let system = last["messages"][0]["content"].as_str().unwrap();
    assert!(system.contains("Use the `recall` tool to search this chat's saved originals"));
    let tool_result: Value = last["messages"]
        .as_array()
        .unwrap()
        .iter()
        .rev()
        .find(|m| m["role"] == "tool")
        .map(|m| serde_json::from_str(m["content"].as_str().unwrap()).unwrap())
        .unwrap();
    let hit = tool_result["hits"]
        .as_array()
        .unwrap()
        .iter()
        .find(|h| h["excerpt"].as_str().unwrap().contains("MARLIN-5521"))
        .expect("attachment original hit");
    assert_eq!(hit["kind"], "attachment");
    assert_eq!(hit["source"], "notes.dat");
    assert_eq!(hit["evidence_kind"], "original");
    let digest: String = Sha256::digest(body.as_bytes())
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect();
    assert_eq!(hit["sha256"], digest);
    assert!(hit["path"]
        .as_str()
        .unwrap()
        .starts_with(".themis/attachments/"));
    let range = hit["char_range"].as_array().unwrap();
    let chars: Vec<char> = body.chars().collect();
    let slice: String = chars
        [range[0].as_u64().unwrap() as usize..range[1].as_u64().unwrap() as usize]
        .iter()
        .collect();
    assert_eq!(slice, hit["excerpt"].as_str().unwrap());

    client.call("shutdown", json!({})).await.unwrap();
    serving.await.unwrap().unwrap();
    // No detached `themis ... serve` may be left on this data directory.
    let leftover = std::process::Command::new("pgrep")
        .arg("-f")
        .arg(data.path().to_string_lossy().as_ref())
        .output()
        .map(|output| output.status.success())
        .unwrap_or(false);
    assert!(!leftover, "a server process was left running");
}
