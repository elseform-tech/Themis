//! Subagents through the real shared server and the `themis` CLI, with a scripted model:
//! the main agent starts two read-only subagents, keeps working, and its turn ends only
//! after both results arrived as notes; stopping a subagent ends it with what it had, and
//! `wait_subagents` takes a result back so it is not repeated; `subagents.enabled: false`
//! hides the tools.

use std::path::Path;
use std::time::{Duration, Instant};

use serde_json::{json, Value};
use themis_desktop::{server::Server, state::AppState, types::ProviderKind};
use wiremock::matchers::method;
use wiremock::{Mock, MockServer, Request, ResponseTemplate};

const SUBAGENT: &str = "You are a subagent";
const NOTE: &str = "A subagent you started has ended.";
const DEADLINE: Duration = Duration::from_secs(60);

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
    let deadline = Instant::now() + DEADLINE;
    while !done() {
        assert!(Instant::now() < deadline, "{what}");
        std::thread::sleep(Duration::from_millis(100));
    }
}

fn reply(text: &str) -> ResponseTemplate {
    ResponseTemplate::new(200).set_body_json(json!({
        "choices": [{"message": {"role": "assistant", "content": text}, "finish_reason": "stop"}]
    }))
}

fn tools(calls: &[(&str, &str, Value)]) -> ResponseTemplate {
    let calls: Vec<Value> = calls
        .iter()
        .map(|(id, name, args)| json!({"id": id, "type": "function", "function": {"name": name, "arguments": args.to_string()}}))
        .collect();
    ResponseTemplate::new(200).set_body_json(json!({
        "choices": [{"message": {"role": "assistant", "content": null, "tool_calls": calls}, "finish_reason": "tool_calls"}]
    }))
}

fn messages(body: &Value) -> &[Value] {
    body["messages"].as_array().map_or(&[], Vec::as_slice)
}

fn text(message: &Value) -> &str {
    message["content"].as_str().unwrap_or_default()
}

fn is_subagent(body: &Value) -> bool {
    messages(body)
        .first()
        .is_some_and(|m| text(m).starts_with(SUBAGENT))
}

fn mentions(body: &Value, needle: &str) -> bool {
    messages(body).iter().any(|m| text(m).contains(needle))
}

fn tool_results(body: &Value) -> usize {
    messages(body)
        .iter()
        .filter(|m| m["role"] == "tool")
        .count()
}

fn tool_names(body: &Value) -> Vec<String> {
    body["tools"]
        .as_array()
        .map(|tools| {
            tools
                .iter()
                .filter_map(|tool| tool["function"]["name"].as_str().map(str::to_owned))
                .collect()
        })
        .unwrap_or_default()
}

/// The first subagent id in the run's spawn result.
fn spawned_id(body: &Value) -> Option<String> {
    messages(body)
        .iter()
        .filter(|m| m["role"] == "tool")
        .map(text)
        .find(|text| text.contains("started"))
        .and_then(|text| {
            let start = text.find(r#""id":""#)? + 6;
            text.get(start..start + 12).map(str::to_owned)
        })
}

async fn bodies(llm: &MockServer) -> Vec<Value> {
    llm.received_requests()
        .await
        .unwrap()
        .iter()
        .map(|request| request.body_json().unwrap_or(Value::Null))
        .collect()
}

struct App {
    llm: MockServer,
    data: tempfile::TempDir,
    project: tempfile::TempDir,
    thread: String,
    serving: tokio::task::JoinHandle<()>,
}

async fn app(
    project_config: Option<&str>,
    respond: impl Fn(&Value) -> ResponseTemplate + Send + Sync + 'static,
) -> App {
    let llm = MockServer::start().await;
    Mock::given(method("POST"))
        .respond_with(move |request: &Request| respond(&request.body_json().unwrap_or(Value::Null)))
        .mount(&llm)
        .await;
    let data = tempfile::tempdir().unwrap();
    let project = tempfile::tempdir().unwrap();
    std::fs::write(project.path().join("notes.txt"), "hello from the notes").unwrap();
    if let Some(config) = project_config {
        std::fs::create_dir_all(project.path().join(".themis")).unwrap();
        std::fs::write(project.path().join(".themis/config.jsonc"), config).unwrap();
    }
    let state = AppState::new_for_test(data.path().join("settings.json"));
    state
        .set_secret("go".into(), "synthetic-test-key".into())
        .await
        .unwrap();
    state.set_go_base_url_override(Some(llm.uri()));
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
    App {
        llm,
        data,
        project,
        thread,
        serving,
    }
}

fn running(data: &Path, thread: &str) -> bool {
    call(data, "get_thread", json!({"threadId": thread}))["running"] == true
}

/// Sends `task` in YOLO and waits until the run is over.
fn run(data: &Path, thread: &str, task: &str) {
    call(
        data,
        "set_thread_approval_mode",
        json!({"threadId": thread, "mode": "yolo"}),
    );
    call(
        data,
        "send_message",
        json!({"threadId": thread, "text": task}),
    );
    until("the run never finished", || !running(data, thread));
}

fn history_kinds(data: &Path, thread: &str) -> Vec<Value> {
    let history = call(data, "get_thread_history", json!({"threadId": thread}));
    history
        .as_array()
        .or_else(|| history["items"].as_array())
        .cloned()
        .unwrap_or_default()
        .into_iter()
        .filter_map(|item| {
            let event = item["envelope"]["event"].clone();
            event["kind"]
                .as_str()
                .is_some_and(|kind| kind.starts_with("subagent"))
                .then_some(event)
        })
        .collect()
}

#[tokio::test(flavor = "multi_thread")]
async fn the_turn_ends_after_both_subagents_reported_back() {
    let app = app(None, |body| {
        let last = messages(body)
            .iter()
            .rfind(|m| m["role"] == "user")
            .map(text)
            .unwrap_or_default()
            .to_owned();
        if is_subagent(body) {
            let results = tool_results(body);
            return if last.contains("Read notes.txt") {
                match results {
                    0 => tools(&[(
                        "a1",
                        "read_file",
                        json!({"file_path": "notes.txt", "status": "Reading the notes"}),
                    )]),
                    _ => reply("A: the notes say hello."),
                }
            } else {
                match results {
                    0 => tools(&[(
                        "b1",
                        "list_dir",
                        json!({"path": ".", "status": "Listing the folder"}),
                    )]),
                    _ => reply("B: one file, notes.txt."),
                }
            };
        }
        let both = mentions(body, "A: the notes say hello.") && mentions(body, "B: one file");
        match tool_results(body) {
            0 => tools(&[(
                "s1",
                "spawn_subagents",
                json!({"subagents": [
                    {"name": "Notes", "brief": "Read notes.txt and say what it says."},
                    {"name": "Files", "brief": "List the project folder."}
                ]}),
            )]),
            _ if both => reply("Merged: hello, and one file."),
            _ => reply("Waiting for the subagents."),
        }
    })
    .await;
    let (data, thread) = (app.data.path().to_owned(), app.thread.clone());
    tokio::task::spawn_blocking({
        let (data, thread) = (data.clone(), thread.clone());
        move || run(&data, &thread, "review the project")
    })
    .await
    .unwrap();

    let all = bodies(&app.llm).await;
    let main: Vec<&Value> = all.iter().filter(|body| !is_subagent(body)).collect();
    let children: Vec<&Value> = all.iter().filter(|body| is_subagent(body)).collect();
    // The main agent saw both results, labelled as a subagent's, before it finished.
    let last = main.last().unwrap();
    assert!(mentions(last, NOTE), "results arrive labelled");
    assert!(mentions(last, "A: the notes say hello.") && mentions(last, "B: one file"));
    // Subagents get only read-only tools with a status field, and no subagent tools.
    for child in &children {
        let names = tool_names(child);
        assert!(
            names
                .iter()
                .all(|name| ["read_file", "list_dir", "search_file", "shell"]
                    .contains(&name.as_str())),
            "{names:?}"
        );
        let read = child["tools"]
            .as_array()
            .unwrap()
            .iter()
            .find(|tool| tool["function"]["name"] == "read_file")
            .unwrap();
        assert_eq!(
            read["function"]["parameters"]["properties"]["status"]["maxLength"],
            80
        );
    }
    assert!(tool_names(main[0]).contains(&"spawn_subagents".to_owned()));
    // A read_file call with a status line still read the file.
    assert!(children
        .iter()
        .any(|child| mentions(child, "hello from the notes")));

    tokio::task::spawn_blocking(move || {
        let reply = call(&data, "get_thread", json!({"threadId": thread}));
        let events = history_kinds(&data, &thread);
        let started = events
            .iter()
            .filter(|e| e["kind"] == "subagent_started")
            .count();
        let ended: Vec<_> = events
            .iter()
            .filter(|e| e["kind"] == "subagent_ended")
            .collect();
        assert_eq!((started, ended.len()), (2, 2), "{events:?} {reply}");
        assert!(ended.iter().all(|e| e["status"] == "finished"), "{ended:?}");
        assert!(ended.iter().all(|e| e["tool_calls"] == 1));
        let listed = call(&data, "list_subagents", json!({"threadId": thread}));
        let listed = listed.as_array().unwrap();
        assert_eq!(listed.len(), 2);
        assert!(listed
            .iter()
            .any(|agent| agent["text"] == "Reading the notes"));
        // Each subagent's own events are saved beside the chat.
        let saved = std::fs::read_dir(data.join("subagents").join(&thread))
            .map(|dir| dir.count())
            .unwrap_or(0);
        assert_eq!(saved, 2);
        // The read-only detail reads a subagent's own steps.
        let id = listed[0]["id"].as_str().unwrap();
        let steps = call(
            &data,
            "get_subagent_transcript",
            json!({"threadId": thread, "subagentId": id}),
        );
        let kinds: Vec<&str> = steps
            .as_array()
            .unwrap()
            .iter()
            .filter_map(|event| event["kind"].as_str())
            .collect();
        assert!(
            kinds.contains(&"tool_started") && kinds.contains(&"finished"),
            "{kinds:?}"
        );
        // Deleting the chat removes them.
        call(&data, "discard_thread", json!({"threadId": thread}));
        assert!(!data.join("subagents").join(&thread).exists());
    })
    .await
    .unwrap();
    app.serving.abort();
}

#[tokio::test(flavor = "multi_thread")]
async fn a_stopped_subagent_ends_and_a_waited_result_is_not_repeated() {
    let app = app(None, move |body| {
        if is_subagent(body) {
            // Still busy when the main agent stops it.
            return tools(&[("c1", "list_dir", json!({"path": "."}))])
                .set_delay(Duration::from_secs(3));
        }
        let id = spawned_id(body).unwrap_or_default();
        match tool_results(body) {
            0 => tools(&[(
                "s1",
                "spawn_subagents",
                json!({"subagents": [{"name": "Slow", "brief": "Look at everything."}]}),
            )]),
            1 => tools(&[("t1", "stop_subagent", json!({"id": id}))]),
            2 => tools(&[(
                "w1",
                "wait_subagents",
                json!({"ids": [id], "timeout_seconds": 30}),
            )]),
            _ => reply("It was stopped."),
        }
    })
    .await;
    let (data, thread) = (app.data.path().to_owned(), app.thread.clone());
    tokio::task::spawn_blocking({
        let (data, thread) = (data.clone(), thread.clone());
        move || run(&data, &thread, "look around")
    })
    .await
    .unwrap();
    let all = bodies(&app.llm).await;
    let main: Vec<&Value> = all.iter().filter(|body| !is_subagent(body)).collect();
    let last = main.last().unwrap();
    // wait_subagents returned the result, so it never came again as a note.
    assert!(mentions(last, r#""status":"stopped""#), "{last}");
    assert!(!main.iter().any(|body| mentions(body, NOTE)));
    tokio::task::spawn_blocking(move || {
        let events = history_kinds(&data, &thread);
        let ended: Vec<_> = events
            .iter()
            .filter(|e| e["kind"] == "subagent_ended")
            .collect();
        assert_eq!(ended.len(), 1, "{events:?}");
        assert_eq!(ended[0]["status"], "stopped");
    })
    .await
    .unwrap();
    app.serving.abort();
}

#[tokio::test(flavor = "multi_thread")]
async fn a_project_can_turn_subagents_off() {
    let app = app(Some(r#"{"subagents": {"enabled": false}}"#), |_| {
        reply("Done.")
    })
    .await;
    let (data, thread) = (app.data.path().to_owned(), app.thread.clone());
    tokio::task::spawn_blocking(move || run(&data, &thread, "hello"))
        .await
        .unwrap();
    let all = bodies(&app.llm).await;
    assert!(!all.is_empty());
    assert!(all.iter().all(|body| !tool_names(body)
        .iter()
        .any(|name| name.contains("subagent"))));
    drop(app.project);
    app.serving.abort();
}

#[tokio::test(flavor = "multi_thread")]
async fn the_person_can_stop_one_subagent_and_the_agent_still_hears_from_it() {
    let app = app(None, move |body| {
        if is_subagent(body) {
            return tools(&[("c1", "list_dir", json!({"path": "."}))])
                .set_delay(Duration::from_secs(4));
        }
        match tool_results(body) {
            0 => tools(&[(
                "s1",
                "spawn_subagents",
                json!({"subagents": [{"name": "Slow", "brief": "Look at everything."}]}),
            )]),
            _ if mentions(body, NOTE) => reply("Slow was stopped."),
            _ => reply("Waiting for Slow."),
        }
    })
    .await;
    let (data, thread) = (app.data.path().to_owned(), app.thread.clone());
    tokio::task::spawn_blocking({
        let (data, thread) = (data.clone(), thread.clone());
        move || {
            call(
                &data,
                "set_thread_approval_mode",
                json!({"threadId": thread, "mode": "yolo"}),
            );
            call(
                &data,
                "send_message",
                json!({"threadId": thread, "text": "look around"}),
            );
            let mut id = String::new();
            until("the subagent never started", || {
                let listed = call(&data, "list_subagents", json!({"threadId": thread}));
                id = listed[0]["id"].as_str().unwrap_or_default().to_owned();
                !id.is_empty()
            });
            call(
                &data,
                "stop_subagent",
                json!({"threadId": thread, "subagentId": id}),
            );
            until("the run never finished", || !running(&data, &thread));
            let listed = call(&data, "list_subagents", json!({"threadId": thread}));
            assert_eq!(listed[0]["status"], "stopped");
        }
    })
    .await
    .unwrap();
    let all = bodies(&app.llm).await;
    let main: Vec<&Value> = all.iter().filter(|body| !is_subagent(body)).collect();
    let last = main.last().unwrap();
    assert!(
        mentions(last, NOTE) && mentions(last, "status stopped"),
        "{last}"
    );
    app.serving.abort();
}
