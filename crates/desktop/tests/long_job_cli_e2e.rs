//! Shared server + `themis` CLI journeys for long jobs: an oversized tool batch runs what fits
//! and the run continues, unchanged re-reads are warned and never end the run, notes carry
//! progress into later messages, and open task lines send the agent back to work (again only
//! after it worked since the last check). The provider is scripted: this checks wiring, not
//! model quality.

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

/// A model that keeps narrating with open task lines is sent back after each stretch of real
/// work (at most three times); narration right after a check is accepted as the answer.
#[tokio::test(flavor = "multi_thread")]
async fn cli_open_task_checks_repeat_only_after_work() {
    let data = tempfile::tempdir().unwrap();
    let project = tempfile::tempdir().unwrap();
    std::fs::write(project.path().join("a.txt"), "A").unwrap();
    let provider = MockServer::start().await;
    let requests = Arc::new(Mutex::new(Vec::<Value>::new()));
    let sink = requests.clone();
    let notes = json!({"set":[{"key":"goal","text":"ship it"},
                              {"key":"step","text":"not done","status":"pending"}]});
    let script = [
        json!({"tool_calls":[{"id":"n1","type":"function","function":{"name":"notes","arguments":notes.to_string()}}]}),
        json!({"content":"Let me continue."}),
        json!({"tool_calls":[read_call("r1", "a.txt")]}),
        json!({"content":"Now I will finish."}),
        json!({"content":"Stopping; the step is still open."}),
    ];
    Mock::given(method("POST"))
        .respond_with(move |request: &Request| {
            let body: Value = request.body_json().unwrap();
            let mut seen = sink.lock().unwrap();
            seen.push(body);
            reply(script[(seen.len() - 1).min(script.len() - 1)].clone())
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
    let done = send(data.path(), &mut events, &id, "Ship it").await;
    assert_eq!(done["kind"], "finished", "{done}");
    assert_eq!(
        done["result"].as_str().unwrap(),
        "Stopping; the step is still open."
    );
    let seen = requests.lock().unwrap();
    assert_eq!(
        seen.len(),
        5,
        "a check after the notes, one after the read, then the answer"
    );
    let checks: Vec<usize> = seen
        .iter()
        .map(|body| {
            serde_json::to_string(&body["messages"])
                .unwrap()
                .matches("[Runtime check, not from the user]")
                .count()
        })
        .collect();
    assert_eq!(checks, vec![0, 0, 1, 1, 2]);
    let system = seen[0]["messages"][0]["content"].as_str().unwrap();
    assert!(system.contains("a reply without tool calls ends your turn"));
    assert!(!system.contains("private reasoning"));
    serving.abort();
}

/// A rate-limited provider (HTTP 429) is waited out instead of failing the run: the request
/// is retried after the rate-limit backoff and the run finishes.
#[tokio::test(flavor = "multi_thread")]
async fn cli_run_waits_out_a_rate_limit() {
    let data = tempfile::tempdir().unwrap();
    let project = tempfile::tempdir().unwrap();
    let provider = MockServer::start().await;
    let calls = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let seen = calls.clone();
    Mock::given(method("POST"))
        .respond_with(move |_: &Request| {
            if seen.fetch_add(1, std::sync::atomic::Ordering::SeqCst) == 0 {
                ResponseTemplate::new(429).set_body_json(
                    json!({"error":{"type":"server_error","message":"Rate limit exceeded. Please try again later."}}),
                )
            } else {
                reply(json!({"content":"Done after the wait."}))
            }
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
    let started = std::time::Instant::now();
    let done = send(data.path(), &mut events, &id, "Say done").await;
    assert_eq!(done["kind"], "finished", "{done}");
    assert_eq!(done["result"].as_str().unwrap(), "Done after the wait.");
    assert_eq!(calls.load(std::sync::atomic::Ordering::SeqCst), 2);
    assert!(
        started.elapsed() >= std::time::Duration::from_secs(10),
        "waited the rate-limit backoff"
    );
    serving.abort();
}

/// A thinking model's reasoning goes back to it with its own tool call on the next request,
/// and is never shown in the answer or saved with the chat.
#[tokio::test(flavor = "multi_thread")]
async fn cli_replays_reasoning_with_its_tool_call_but_never_saves_it() {
    const REASONING: &str = "private-reasoning-sentinel: read notes.txt before answering";
    let data = tempfile::tempdir().unwrap();
    let project = tempfile::tempdir().unwrap();
    std::fs::write(project.path().join("notes.txt"), "the answer is 42\n").unwrap();
    let provider = MockServer::start().await;
    Mock::given(method("POST"))
        .respond_with(|request: &Request| {
            let body: Value = request.body_json().unwrap();
            let answered = body["messages"]
                .as_array()
                .is_some_and(|messages| messages.iter().any(|m| m["role"] == "tool"));
            if answered {
                reply(json!({"content":"The notes say 42."}))
            } else {
                reply(json!({"content":"","reasoning_content":REASONING,
                    "tool_calls":[read_call("reasoned-read-1","notes.txt")]}))
            }
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
    let done = send(data.path(), &mut events, &id, "What do the notes say?").await;
    assert_eq!(done["kind"], "finished", "{done}");
    assert_eq!(done["result"].as_str().unwrap(), "The notes say 42.");
    let requests = provider.received_requests().await.unwrap();
    let replayed = requests.last().unwrap().body_json::<Value>().unwrap();
    let assistant = replayed["messages"]
        .as_array()
        .unwrap()
        .iter()
        .find(|m| m["role"] == "assistant" && m["tool_calls"].is_array())
        .expect("the tool call goes back");
    assert_eq!(assistant["reasoning_content"], REASONING);
    serving.abort();
    // Nothing the app wrote (chat history, transcripts, evidence, logs) holds the reasoning.
    for root in [data.path(), project.path()] {
        for entry in walk(root) {
            let bytes = std::fs::read(&entry).unwrap_or_default();
            assert!(
                !String::from_utf8_lossy(&bytes).contains("private-reasoning-sentinel"),
                "reasoning saved in {}",
                entry.display()
            );
        }
    }
}

fn walk(root: &Path) -> Vec<std::path::PathBuf> {
    let mut files = Vec::new();
    let mut pending = vec![root.to_path_buf()];
    while let Some(dir) = pending.pop() {
        for entry in std::fs::read_dir(&dir).into_iter().flatten().flatten() {
            let path = entry.path();
            if path.is_dir() {
                pending.push(path);
            } else {
                files.push(path);
            }
        }
    }
    files
}

/// A provider stream that drops part-way is retried, and the run still finishes.
#[tokio::test(flavor = "multi_thread")]
async fn cli_retries_a_stream_that_dropped_part_way() {
    let data = tempfile::tempdir().unwrap();
    let project = tempfile::tempdir().unwrap();
    let provider = MockServer::start().await;
    let calls = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let seen = calls.clone();
    Mock::given(method("POST"))
        .respond_with(move |_: &Request| {
            let body = if seen.fetch_add(1, std::sync::atomic::Ordering::SeqCst) == 0 {
                // Reasoning, then the connection ends with no finish reason.
                format!("data: {}\n\n", json!({"choices":[{"delta":{"reasoning_content":"thinking"}}]}))
            } else {
                format!(
                    "data: {}\n\ndata: [DONE]\n\n",
                    json!({"choices":[{"delta":{"content":"Done after the drop."},"finish_reason":"stop"}]})
                )
            };
            ResponseTemplate::new(200).set_body_raw(body, "text/event-stream")
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
    let done = send(data.path(), &mut events, &id, "Say done").await;
    assert_eq!(done["kind"], "finished", "{done}");
    assert_eq!(done["result"].as_str().unwrap(), "Done after the drop.");
    assert_eq!(calls.load(std::sync::atomic::Ordering::SeqCst), 2);
    serving.abort();
}

/// Reasoning that comes as `reasoning` with `reasoning_details` (Kimi, Hunyuan) goes back in
/// those same fields with the tool call it led to.
#[tokio::test(flavor = "multi_thread")]
async fn cli_replays_reasoning_details_in_the_fields_they_came_in() {
    let data = tempfile::tempdir().unwrap();
    let project = tempfile::tempdir().unwrap();
    std::fs::write(project.path().join("notes.txt"), "the answer is 42\n").unwrap();
    let provider = MockServer::start().await;
    Mock::given(method("POST"))
        .respond_with(|request: &Request| {
            let body: Value = request.body_json().unwrap();
            let answered = body["messages"]
                .as_array()
                .is_some_and(|messages| messages.iter().any(|m| m["role"] == "tool"));
            if answered {
                reply(json!({"content":"The notes say 42."}))
            } else {
                reply(json!({"content":"","reasoning":"Read the notes.",
                    "reasoning_details":[{"type":"reasoning.text","text":"Read the notes.","index":0}],
                    "tool_calls":[read_call("detail-read-1","notes.txt")]}))
            }
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
    let done = send(data.path(), &mut events, &id, "What do the notes say?").await;
    assert_eq!(done["kind"], "finished", "{done}");
    let requests = provider.received_requests().await.unwrap();
    let last = requests.last().unwrap().body_json::<Value>().unwrap();
    let assistant = last["messages"]
        .as_array()
        .unwrap()
        .iter()
        .find(|m| m["role"] == "assistant" && m["tool_calls"].is_array())
        .expect("the tool call goes back");
    assert_eq!(assistant["reasoning"], "Read the notes.");
    assert_eq!(assistant["reasoning_details"][0]["text"], "Read the notes.");
    assert!(assistant.get("reasoning_content").is_none());
    serving.abort();
}
