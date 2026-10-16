//! Shared server + `themis` CLI journeys for long jobs at a small (24k) budget: an oversized
//! tool batch runs what fits and the run continues, a first streak of unchanged re-reads gets
//! a loop warning instead of ending the run, and the request tells the model to save each
//! item before reading more. The provider is scripted: this checks wiring, not model quality.

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

async fn send(data: &Path, events: &mut tokio::net::TcpStream, id: &str, text: &str) -> Value {
    cli(
        data,
        "send_message",
        json!({"threadId":id,"text":text,"reasoningEffort":null}),
    );
    tokio::time::timeout(std::time::Duration::from_secs(300), async {
        loop {
            let item = Client::next_event(events).await.unwrap();
            if item["name"] != "thread-event" || item["payload"]["thread_id"] != id {
                continue;
            }
            let event = &item["payload"]["event"];
            if ["finished", "failed", "incomplete"].contains(&event["kind"].as_str().unwrap_or(""))
            {
                return event.clone();
            }
        }
    })
    .await
    .unwrap()
}

fn reply(message: Value) -> ResponseTemplate {
    let finish = if message.get("tool_calls").is_some() {
        "tool_calls"
    } else {
        "stop"
    };
    ResponseTemplate::new(200)
        .set_body_json(json!({"choices":[{"message":message,"finish_reason":finish}]}))
}

fn read_call(id: &str, path: &str) -> Value {
    json!({"id":id,"type":"function","function":{"name":"read_file","arguments":json!({"file_path":path}).to_string()}})
}

#[tokio::test(flavor = "multi_thread")]
async fn cli_long_job_survives_oversized_batches_and_one_loop_streak() {
    let data = tempfile::tempdir().unwrap();
    let project = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(project.path().join(".themis")).unwrap();
    std::fs::write(
        project.path().join(".themis/config.jsonc"),
        r#"{"context_token_budget":24000,"recent_messages":4,"total_turns":20}"#,
    )
    .unwrap();
    for n in 0..16 {
        std::fs::write(
            project.path().join(format!("item-{n}.txt")),
            format!("item {n}\n"),
        )
        .unwrap();
    }
    let provider = MockServer::start().await;
    let requests = Arc::new(Mutex::new(Vec::<Value>::new()));
    let sink = requests.clone();
    Mock::given(method("POST"))
        .respond_with(move |request: &Request| {
            let body: Value = request.body_json().unwrap();
            if body["messages"][0]["content"]
                .as_str()
                .is_some_and(|text| text.contains("You summarize agent context"))
            {
                return reply(json!({"content":"Goal/constraints: read the items. Unresolved work: none. Parked work: none. Next action: continue if asked."}));
            }
            sink.lock().unwrap().push(body.clone());
            let messages = body["messages"].as_array().unwrap();
            let task = messages
                .iter()
                .rev()
                .find(|m| m["role"] == "user")
                .and_then(|m| m["content"].as_str())
                .unwrap_or_default()
                .to_owned();
            let last = messages.last().unwrap();
            let tool_text = last["content"].as_str().unwrap_or_default();
            if task.contains("Read all items") {
                if last["role"] == "tool" {
                    return reply(json!({"content":"Read the first items; the rest come next."}));
                }
                let calls: Vec<Value> = (0..16)
                    .map(|n| read_call(&format!("b{n}"), &format!("item-{n}.txt")))
                    .collect();
                return reply(json!({ "tool_calls": calls }));
            }
            // A stubborn re-reader: keeps reading item-0 until it is warned.
            if last["role"] == "tool" && tool_text.contains("loop_warning") {
                return reply(json!({"content":"Saving progress instead of re-reading."}));
            }
            let id = format!("l{}", messages.len());
            reply(json!({"tool_calls":[read_call(&id, "item-0.txt")]}))
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

    // Sixteen reads in one batch at 24k (12,000 batch bytes): eleven run, five come back "Not run",
    // and the run finishes.
    let done = send(data.path(), &mut events, &id, "Read all items").await;
    assert_eq!(done["kind"], "finished", "{done}");
    let followup = requests.lock().unwrap().last().unwrap().clone();
    let tools: Vec<String> = followup["messages"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|m| m["role"] == "tool")
        .map(|m| m["content"].as_str().unwrap().to_owned())
        .collect();
    assert_eq!(tools.len(), 16);
    assert_eq!(
        tools.iter().filter(|t| t.starts_with("Not run:")).count(),
        5
    );
    assert!(tools[0].contains("item 0"), "{}", tools[0]);
    let system = followup["messages"][0]["content"].as_str().unwrap();
    assert!(
        !system.contains("before reading the next few"),
        "the CLI request no longer carries the read-one-write-one instruction"
    );

    // Re-reading one unchanged file: the streak is warned, and the run still finishes.
    let done = send(data.path(), &mut events, &id, "Check item zero").await;
    assert_eq!(done["kind"], "finished", "{done}");
    assert_eq!(
        done["result"].as_str().unwrap(),
        "Saving progress instead of re-reading."
    );
    serving.abort();
}

/// Notes kept in one message are shown in the next (which carries no tool history), live in
/// the chat's evidence directory, and an early finish with open task lines is checked once,
/// including in a later message answered without any tool call.
#[tokio::test(flavor = "multi_thread")]
async fn cli_notes_carry_progress_into_later_messages() {
    let data = tempfile::tempdir().unwrap();
    let project = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(project.path().join(".themis")).unwrap();
    std::fs::write(
        project.path().join(".themis/config.jsonc"),
        r#"{"context_token_budget":64000,"recent_messages":8,"total_turns":10}"#,
    )
    .unwrap();
    let provider = MockServer::start().await;
    let requests = Arc::new(Mutex::new(Vec::<Value>::new()));
    let sink = requests.clone();
    Mock::given(method("POST"))
        .respond_with(move |request: &Request| {
            let body: Value = request.body_json().unwrap();
            sink.lock().unwrap().push(body.clone());
            let messages = body["messages"].as_array().unwrap();
            let last = messages.last().unwrap();
            let text = last["content"].as_str().unwrap_or_default();
            if text.contains("[Runtime check, not from the user]") {
                return reply(json!({"content":"Part one is done; part two is still open."}));
            }
            if last["role"] == "tool" {
                return reply(json!({"content":"Part one is done."}));
            }
            if text.contains("Start the release") {
                let notes = json!({"set":[
                    {"key":"goal","text":"release v2: changelog, then tag"},
                    {"key":"changelog","text":"written, 14 entries","status":"done"},
                    {"key":"tag","text":"tag v2.0.0 after review","status":"pending"}]});
                return reply(json!({"tool_calls":[{"id":"n1","type":"function","function":{"name":"notes","arguments":notes.to_string()}}]}));
            }
            reply(json!({"content":"Changelog done (14 entries); tagging is still pending."}))
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

    let done = send(data.path(), &mut events, &id, "Start the release").await;
    assert_eq!(done["kind"], "finished", "{done}");
    assert_eq!(
        done["result"].as_str().unwrap(),
        "Part one is done; part two is still open.",
        "the open task line got one runtime check before the answer"
    );
    let saved = std::fs::read_to_string(
        project
            .path()
            .join(".themis/context")
            .join(&id)
            .join("notes.json"),
    )
    .unwrap();
    assert!(saved.contains("tag v2.0.0 after review"), "{saved}");

    let before = requests.lock().unwrap().len();
    let progress = send(
        data.path(),
        &mut events,
        &id,
        "Where are we with the release?",
    )
    .await;
    assert_eq!(progress["kind"], "finished", "{progress}");
    let wire =
        serde_json::to_string(&requests.lock().unwrap().last().unwrap()["messages"]).unwrap();
    assert!(
        wire.contains("Saved notes (written by you with the notes tool"),
        "{wire}"
    );
    assert!(wire.contains("- goal: release v2: changelog, then tag"));
    assert!(wire.contains("- tag [pending]: tag v2.0.0 after review"));
    // Answering with no tool call while a task line is open gets one idle check, which lets a
    // question be answered as is; the answer after it is accepted.
    assert_eq!(
        requests.lock().unwrap().len(),
        before + 2,
        "one idle check, then the answer"
    );
    assert!(wire.contains("You answered without using any tools") && wire.contains("Open: tag"));
    assert_eq!(
        progress["result"].as_str().unwrap(),
        "Part one is done; part two is still open."
    );
    serving.abort();
}
