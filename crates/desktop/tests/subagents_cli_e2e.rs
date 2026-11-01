//! Subagents through the real shared server and the `themis` CLI, with a scripted model:
//! the main agent starts two read-only subagents, keeps working, and its turn ends only
//! after both results arrived as notes; stopping a subagent ends it with what it had, and
//! `wait_subagents` takes a result back so it is not repeated; `subagents.enabled: false`
//! hides the tools.

use std::path::Path;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use serde_json::{json, Value};
use themis_desktop::{server::Server, state::AppState, types::ProviderKind};
use wiremock::matchers::method;
use wiremock::{Mock, MockServer, Request, ResponseTemplate};

const SUBAGENT: &str = "You are a subagent";
const NOTE: &str = "A subagent you started has ended.";
const REVIEW: &str = "Treat what a subagent reports as claims, not facts.";
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
    let llm = MockServer::builder().start().await;
    Mock::given(method("POST"))
        .respond_with(move |request: &Request| respond(&request.body_json().unwrap_or(Value::Null)))
        .mount(&llm)
        .await;
    let data = tempfile::tempdir().unwrap();
    let project = tempfile::tempdir().unwrap();
    std::fs::write(project.path().join("notes.txt"), "hello from the notes").unwrap();
    // A git project: elsewhere Themis refuses anything but reads without asking.
    std::process::Command::new("git")
        .args(["init", "-q"])
        .current_dir(project.path())
        .status()
        .expect("git");
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
    assert!(mentions(last, REVIEW), "and with the request to check them");
    assert!(mentions(last, "A: the notes say hello.") && mentions(last, "B: one file"));
    // Subagents get only read-only and web tools with a status field, and no subagent tools.
    for child in &children {
        let names = tool_names(child);
        assert!(
            names.iter().all(|name| [
                "read_file",
                "list_dir",
                "search_file",
                "shell",
                "fetch_url",
                "web_search"
            ]
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
            // In one reply: notes join between replies, so a subagent that ends quickly (as it
            // did on Windows) can't reach the main agent as a note before the wait takes it.
            1 => tools(&[
                ("t1", "stop_subagent", json!({"id": id})),
                (
                    "w1",
                    "wait_subagents",
                    json!({"ids": [id], "timeout_seconds": 30}),
                ),
            ]),
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
    // A result returned by wait_subagents carries the same request to check it.
    assert!(mentions(last, REVIEW), "{last}");
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

/// Two read-only subagents and one that owns `notes/`: it writes there and nowhere else, in
/// YOLO too, and the others stay read-only. Overlaps and escapes are refused before any start.
#[tokio::test(flavor = "multi_thread")]
async fn write_paths_hold_a_subagent_to_its_own_folder() {
    let app = app(None, |body| {
        if is_subagent(body) {
            if !mentions(body, "Write the summary") {
                return reply("Read-only part done.");
            }
            return match tool_results(body) {
                // Slow enough that the main agent's next spawn finds it running.
                0 => tools(&[(
                    "w1",
                    "write_file",
                    json!({"file_path": "notes/summary.md", "content": "inside", "status": "Writing"}),
                )])
                .set_delay(std::time::Duration::from_millis(1500)),
                1 => tools(&[(
                    "w2",
                    "write_file",
                    json!({"file_path": "outside.md", "content": "outside", "status": "Trying"}),
                )]),
                2 => tools(&[(
                    "w3",
                    "shell",
                    json!({"command": "sh", "args": ["-c", "echo shell > notes/shell.md; echo shell > shell-outside.md"], "status": "Shell"}),
                )]),
                _ => reply("Writer done."),
            };
        }
        match tool_results(body) {
            0 => tools(&[(
                "bad1",
                "spawn_subagents",
                json!({"subagents": [{"brief": "Escape", "write_paths": ["../up/"]}]}),
            )]),
            1 => tools(&[(
                "bad2",
                "spawn_subagents",
                json!({"subagents": [{"brief": "Settings", "write_paths": [".themis/"]}]}),
            )]),
            2 => tools(&[(
                "bad3",
                "spawn_subagents",
                json!({"subagents": [
                    {"brief": "One", "write_paths": ["notes/"]},
                    {"brief": "Two", "write_paths": ["notes/deep/file.md"]}
                ]}),
            )]),
            3 => tools(&[(
                "s1",
                "spawn_subagents",
                json!({"subagents": [
                    {"name": "Reader A", "brief": "Read part A"},
                    {"name": "Reader B", "brief": "Read part B"},
                    {"name": "Writer", "brief": "Write the summary", "write_paths": ["notes/"]}
                ]}),
            )]),
            4 => tools(&[(
                "s2",
                "spawn_subagents",
                json!({"subagents": [{"brief": "Late", "write_paths": ["notes/late.md"]}]}),
            )]),
            5 => tools(&[("w", "wait_subagents", json!({}))]),
            _ => reply("All done."),
        }
    })
    .await;
    run(app.data.path(), &app.thread, "Do the parts");
    let root = app.project.path();
    assert_eq!(
        std::fs::read_to_string(root.join("notes/summary.md")).unwrap(),
        "inside"
    );
    assert!(!root.join("outside.md").exists(), "the file tool stayed in");
    assert!(!root.join("up").exists());

    let all = bodies(&app.llm).await;
    let main_results = |needle: &str| {
        all.iter().any(|body| {
            !is_subagent(body)
                && messages(body)
                    .iter()
                    .any(|m| m["role"] == "tool" && text(m).contains(needle))
        })
    };
    assert!(main_results("must stay inside the project"));
    assert!(main_results("never writable by a subagent"));
    assert!(main_results("overlaps"), "within one call");
    assert!(main_results("which Writer owns"), "with a running one");

    let writer_tools = all
        .iter()
        .find(|body| is_subagent(body) && mentions(body, "Write the summary"))
        .map(tool_names)
        .unwrap();
    assert!(writer_tools.iter().any(|name| name == "write_file"));
    assert!(writer_tools.iter().any(|name| name == "apply_patch"));
    assert!(!writer_tools.iter().any(|name| name == "delete_file"));
    assert!(all.iter().any(|body| is_subagent(body)
        && mentions(
            body,
            "may change files only under these project paths: notes/"
        )));
    let reader = all
        .iter()
        .find(|body| is_subagent(body) && mentions(body, "Read part A"))
        .unwrap();
    assert!(!tool_names(reader).iter().any(|name| name == "write_file"));
    assert!(mentions(reader, "You are read-only"));

    let writer_outcome = |needle: &str| {
        all.iter().any(|body| {
            is_subagent(body)
                && messages(body)
                    .iter()
                    .any(|m| m["role"] == "tool" && text(m).contains(needle))
        })
    };
    assert!(
        writer_outcome("outside the folders"),
        "the refusal reaches it"
    );
    // The shell wall needs an enforcing sandbox: macOS always has one.
    if cfg!(target_os = "macos") {
        assert!(
            !root.join("shell-outside.md").exists(),
            "the shell stayed in"
        );
        assert!(
            root.join("notes/shell.md").exists(),
            "its own folder is writable"
        );
    }

    let events = history_kinds(app.data.path(), &app.thread);
    let started = events
        .iter()
        .find(|event| event["kind"] == "subagent_started" && event["name"] == "Writer")
        .expect("the writer started");
    assert_eq!(started["write_paths"], json!(["notes/"]));
    app.serving.abort();
}

/// The main agent sends a finished subagent back to correct itself: same subagent, its earlier
/// exchange as history, and a second report. A later turn can do the same.
#[tokio::test(flavor = "multi_thread")]
async fn the_main_agent_can_send_a_finished_subagent_back() {
    let id = Arc::new(Mutex::new(String::new()));
    let known = Arc::clone(&id);
    let app = app(None, move |body| {
        if is_subagent(body) {
            return if mentions(body, "That is wrong") {
                reply("Corrected answer: 42.")
            } else {
                reply("First answer: 41.")
            };
        }
        if tool_names(body).is_empty() {
            return reply("Hmm, let me look at that.");
        }
        if !mentions(body, "Ask it again") {
            return match tool_results(body) {
                0 => tools(&[(
                    "s1",
                    "spawn_subagents",
                    json!({"say": "Starting Adder to do the sum.", "subagents": [{"name": "Adder", "brief": "Add the numbers"}]}),
                )]),
                1 => tools(&[("w1", "wait_subagents", json!({}))]),
                2 => tools(&[("m1", "message_subagent", {
                    let found = spawned_id(body).unwrap();
                    *known.lock().unwrap() = found.clone();
                    json!({"id": found, "message": "That is wrong, check the sum"})
                })]),
                3 => tools(&[("w2", "wait_subagents", json!({}))]),
                _ => reply("Now it says 42."),
            };
        }
        // A later turn: its tool results start again, and the id comes from the first turn.
        match tool_results(body) {
            0 => tools(&[(
                "m2",
                "message_subagent",
                json!({"id": known.lock().unwrap().clone(), "message": "One more: double it"}),
            )]),
            1 => tools(&[("w3", "wait_subagents", json!({}))]),
            _ => reply("It doubled it."),
        }
    })
    .await;
    run(app.data.path(), &app.thread, "Do the sum");
    let all = bodies(&app.llm).await;
    let second = all
        .iter()
        .find(|body| is_subagent(body) && mentions(body, "That is wrong"))
        .expect("it was sent back");
    assert!(
        mentions(second, "Add the numbers"),
        "its brief is still in its history"
    );
    assert!(mentions(second, "First answer: 41."), "so is its answer");
    assert!(all
        .iter()
        .any(|body| !is_subagent(body) && mentions(body, "Corrected answer: 42.")));
    // After each report the main agent had a step with no tools, to say what it made of it.
    assert_eq!(
        all.iter()
            .filter(|body| !is_subagent(body) && tool_names(body).is_empty())
            .count(),
        2
    );
    assert!(all
        .iter()
        .all(|body| !is_subagent(body) || !mentions(body, "A subagent just reported back")));
    let events = history_kinds(app.data.path(), &app.thread);
    let kinds: Vec<&str> = events
        .iter()
        .filter_map(|event| event["kind"].as_str())
        .filter(|kind| *kind != "subagent_status")
        .collect();
    assert_eq!(
        kinds,
        [
            "subagent_started",
            "subagent_ended",
            "subagent_message",
            "subagent_ended"
        ]
    );
    let message = events
        .iter()
        .find(|event| event["kind"] == "subagent_message")
        .unwrap();
    assert_eq!(message["resumed"], json!(true));
    // The line on a subagent tool shows in the chat before the tool starts.
    let history = call(
        app.data.path(),
        "get_thread_history",
        json!({"threadId": app.thread}),
    );
    let kinds: Vec<Value> = history
        .as_array()
        .or_else(|| history["items"].as_array())
        .cloned()
        .unwrap_or_default()
        .into_iter()
        .map(|item| item["envelope"]["event"].clone())
        .collect();
    let said = kinds
        .iter()
        .position(|event| {
            event["kind"] == "assistant_text"
                && event["text"]
                    .as_str()
                    .is_some_and(|t| t.contains("Starting Adder"))
        })
        .expect("the line was shown");
    let started = kinds
        .iter()
        .position(|event| event["kind"] == "tool_started" && event["tool"] == "spawn_subagents")
        .unwrap();
    assert!(said < started);

    run(app.data.path(), &app.thread, "Ask it again");
    let all = bodies(&app.llm).await;
    let third = all
        .iter()
        .find(|body| is_subagent(body) && mentions(body, "One more: double it"))
        .expect("a later turn reached it too");
    assert!(mentions(third, "Corrected answer: 42."));
    app.serving.abort();
}

/// Ordinary tool calls never lose their tools for a step.
#[tokio::test(flavor = "multi_thread")]
async fn plain_tool_calls_are_not_nudged() {
    let app = app(None, |body| match tool_results(body) {
        0 => tools(&[("r1", "read_file", json!({"file_path": "notes.txt"}))]),
        1 => tools(&[("r2", "list_dir", json!({"path": "."}))]),
        _ => reply("Done."),
    })
    .await;
    run(app.data.path(), &app.thread, "Look around");
    assert!(!bodies(&app.llm)
        .await
        .iter()
        .any(|body| tool_names(body).is_empty()));
    app.serving.abort();
}

/// `subagents.allow_writes` off keeps a subagent read-only and says so; a project can do this.
#[tokio::test(flavor = "multi_thread")]
async fn a_project_can_turn_subagent_writes_off() {
    let app = app(Some(r#"{"subagents": {"allow_writes": false}}"#), |body| {
        if is_subagent(body) {
            return reply("Done.");
        }
        match tool_results(body) {
            0 => tools(&[(
                "s1",
                "spawn_subagents",
                json!({"subagents": [{"brief": "Write it", "write_paths": ["notes/"]}]}),
            )]),
            1 => tools(&[("w", "wait_subagents", json!({}))]),
            _ => reply("All done."),
        }
    })
    .await;
    run(app.data.path(), &app.thread, "Do it");
    let all = bodies(&app.llm).await;
    let child = all.iter().find(|body| is_subagent(body)).unwrap();
    assert!(!tool_names(child).iter().any(|name| name == "write_file"));
    assert!(mentions(child, "You are read-only"));
    assert!(all.iter().any(|body| {
        !is_subagent(body)
            && messages(body)
                .iter()
                .any(|m| m["role"] == "tool" && text(m).contains("write_paths were ignored"))
    }));
    assert!(!app.project.path().join("notes").exists());
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

/// `call` that returns the CLI's error text instead of failing the test.
fn try_call(data: &Path, command: &str, args: Value) -> Result<Value, String> {
    let output = std::process::Command::new(env!("CARGO_BIN_EXE_themis"))
        .arg("--data-dir")
        .arg(data)
        .args(["call", command, &args.to_string()])
        .env_remove("OPENCODE_KEY")
        .output()
        .expect("CLI");
    if output.status.success() {
        Ok(serde_json::from_slice(&output.stdout).unwrap_or(Value::Null))
    } else {
        Err(String::from_utf8_lossy(&output.stderr).into_owned())
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn settings_change_only_their_fields_and_refuse_a_stale_page() {
    let app = app(None, |_| reply("Done.")).await;
    let data = app.data.path().to_owned();
    tokio::task::spawn_blocking(move || {
        let view = call(&data, "get_subagent_settings", json!({}));
        assert_eq!(view["settings"]["max_parallel"], 3);
        let revision = view["revision"].clone();
        let saved = call(
            &data,
            "update_subagent_settings",
            json!({"changes": {"max_parallel": 2, "model": "glm-5"}, "revision": revision}),
        );
        assert_eq!(saved["settings"]["max_parallel"], 2);
        assert_eq!(saved["settings"]["model"], "glm-5");
        assert_eq!(saved["settings"]["enabled"], true);
        // The page that loaded before is stale now.
        let stale = try_call(
            &data,
            "update_subagent_settings",
            json!({"changes": {"enabled": false}, "revision": revision}),
        );
        assert!(stale.unwrap_err().contains("changed in runtime.jsonc"));
        // "Chat model" clears the field.
        let cleared = call(
            &data,
            "update_subagent_settings",
            json!({"changes": {"model": null}, "revision": saved["revision"]}),
        );
        assert!(cleared["settings"]["model"].is_null());
        // Only the fields the page shows, within their limits.
        assert!(try_call(
            &data,
            "update_subagent_settings",
            json!({"changes": {"max_turns": 5}})
        )
        .unwrap_err()
        .contains("max_turns"));
        assert!(try_call(
            &data,
            "update_subagent_settings",
            json!({"changes": {"max_parallel": 9}})
        )
        .is_err());
    })
    .await
    .unwrap();
    app.serving.abort();
}

/// Sends `task` in Custom mode and waits until the run is over.
fn run_custom(data: &Path, thread: &str, task: &str) {
    call(
        data,
        "set_thread_approval_mode",
        json!({"threadId": thread, "mode": "custom"}),
    );
    call(
        data,
        "send_message",
        json!({"threadId": thread, "text": task}),
    );
    until("the run never finished", || !running(data, thread));
}

#[tokio::test(flavor = "multi_thread")]
async fn starting_subagents_needs_no_prompt_but_a_deny_rule_still_holds() {
    let respond = |body: &Value| {
        if is_subagent(body) {
            return reply("Nothing to report.");
        }
        match tool_results(body) {
            0 => tools(&[(
                "s1",
                "spawn_subagents",
                json!({"subagents": [{"name": "Quick", "brief": "Say hello."}]}),
            )]),
            _ if mentions(body, NOTE) => reply("Quick reported back."),
            _ if mentions(body, "approval denied") => reply("Subagents are not allowed here."),
            _ => reply("Waiting."),
        }
    };
    // Custom mode with no rules: no approval prompt holds the spawn up.
    let app1 = app(None, respond).await;
    let (data, thread) = (app1.data.path().to_owned(), app1.thread.clone());
    tokio::task::spawn_blocking(move || run_custom(&data, &thread, "say hello"))
        .await
        .unwrap();
    let all = bodies(&app1.llm).await;
    assert!(all
        .iter()
        .any(|body| !is_subagent(body) && mentions(body, NOTE)));
    app1.serving.abort();
    // A project rule that denies the tool refuses it.
    let app2 = app(
        Some(r#"{"approval": {"rules": [{"tool": "spawn_subagents", "action": "deny"}]}}"#),
        respond,
    )
    .await;
    let (data, thread) = (app2.data.path().to_owned(), app2.thread.clone());
    tokio::task::spawn_blocking(move || run_custom(&data, &thread, "say hello"))
        .await
        .unwrap();
    let all = bodies(&app2.llm).await;
    assert!(all.iter().all(|body| !is_subagent(body)), "no subagent ran");
    assert!(all.iter().any(|body| mentions(body, "approval denied")));
    app2.serving.abort();
}

fn allow_models(data: &Path, models: &[&str]) {
    let view = call(data, "get_subagent_settings", json!({}));
    call(
        data,
        "update_subagent_settings",
        json!({"changes": {"models": models}, "revision": view["revision"]}),
    );
}

fn spawn_description(body: &Value) -> String {
    body["tools"]
        .as_array()
        .and_then(|tools| {
            tools
                .iter()
                .find(|tool| tool["function"]["name"] == "spawn_subagents")
        })
        .and_then(|tool| tool["function"]["description"].as_str())
        .unwrap_or_default()
        .to_owned()
}

#[tokio::test(flavor = "multi_thread")]
async fn the_person_names_a_model_per_subagent_from_the_allowed_ones() {
    let app = app(None, |body| {
        if is_subagent(body) {
            return reply(&format!(
                "Ran on {}.",
                body["model"].as_str().unwrap_or("?")
            ));
        }
        match tool_results(body) {
            // A model that is not allowed is refused before anything starts.
            0 => tools(&[(
                "s0",
                "spawn_subagents",
                json!({"subagents": [{"name": "Big", "brief": "b", "model": "claude-opus"}]}),
            )]),
            1 => tools(&[(
                "s1",
                "spawn_subagents",
                json!({"subagents": [
                    {"name": "Review", "brief": "Review src/.", "model": "GLM 5"},
                    {"name": "Docs", "brief": "Check docs/."}
                ]}),
            )]),
            _ if mentions(body, "Ran on glm-5.") && mentions(body, "Ran on test-model.") => {
                reply("Both done.")
            }
            _ => reply("Waiting."),
        }
    })
    .await;
    let (data, thread) = (app.data.path().to_owned(), app.thread.clone());
    tokio::task::spawn_blocking({
        let (data, thread) = (data.clone(), thread.clone());
        move || {
            allow_models(&data, &["glm-5"]);
            run(&data, &thread, "use glm-5 for the review");
        }
    })
    .await
    .unwrap();
    let all = bodies(&app.llm).await;
    let main: Vec<&Value> = all.iter().filter(|body| !is_subagent(body)).collect();
    assert!(
        spawn_description(main[0]).contains("one of glm-5"),
        "{}",
        spawn_description(main[0])
    );
    assert!(main
        .iter()
        .any(|body| mentions(body, "subagent model 'claude-opus' is not available")));
    let models: Vec<&str> = all
        .iter()
        .filter(|body| is_subagent(body))
        .filter_map(|body| body["model"].as_str())
        .collect();
    assert!(
        models.contains(&"glm-5") && models.contains(&"test-model"),
        "{models:?}"
    );
    tokio::task::spawn_blocking(move || {
        let listed = call(&data, "list_subagents", json!({"threadId": thread}));
        let review = listed
            .as_array()
            .unwrap()
            .iter()
            .find(|agent| agent["name"] == "Review")
            .unwrap()
            .clone();
        assert_eq!(review["model"], "glm-5");
        assert!(review["ran_on"].is_null());
    })
    .await
    .unwrap();
    app.serving.abort();
}

#[tokio::test(flavor = "multi_thread")]
async fn a_subagent_whose_model_fails_at_once_falls_back_to_the_chat_model() {
    let app = app(None, |body| {
        if is_subagent(body) {
            if body["model"] == "glm-5" {
                return ResponseTemplate::new(400)
                    .set_body_json(json!({"error": {"message": "model glm-5 is over its quota"}}));
            }
            return reply("Done on the fallback.");
        }
        match tool_results(body) {
            0 => tools(&[("s1", "spawn_subagents", json!({"subagents": [{"name": "Review", "brief": "Review src/.", "model": "glm-5"}]}))]),
            _ if mentions(body, NOTE) => reply("Reported."),
            _ => reply("Waiting."),
        }
    })
    .await;
    let (data, thread) = (app.data.path().to_owned(), app.thread.clone());
    tokio::task::spawn_blocking(move || {
        allow_models(&data, &["glm-5"]);
        run(&data, &thread, "use glm-5 for the review");
        let listed = call(&data, "list_subagents", json!({"threadId": thread}));
        assert_eq!(listed[0]["status"], "finished", "{listed}");
        assert_eq!(listed[0]["model"], "glm-5");
        assert_eq!(listed[0]["ran_on"], "test-model");
        assert!(listed[0]["result"].as_str().unwrap().contains("fallback"));
        let events = history_kinds(&data, &thread);
        let ended = events
            .iter()
            .find(|e| e["kind"] == "subagent_ended")
            .unwrap();
        assert_eq!(ended["model"], "test-model");
    })
    .await
    .unwrap();
    app.serving.abort();
}

#[tokio::test(flavor = "multi_thread")]
async fn a_subagent_waiting_for_approval_does_not_hold_the_reply() {
    let app = app(None, |body| {
        if is_subagent(body) {
            return tools(&[("c1", "shell", json!({"command": "nl", "args": ["-n", "notes.txt"]}))]);
        }
        match tool_results(body) {
            0 => tools(&[(
                "s1",
                "spawn_subagents",
                json!({"subagents": [{"name": "Lister", "brief": "Number the lines of notes.txt."}]}),
            )]),
            _ if mentions(body, NOTE) => reply("Done."),
            _ => reply("Waiting."),
        }
    })
    .await;
    let (data, thread) = (app.data.path().to_owned(), app.thread.clone());
    tokio::task::spawn_blocking(move || {
        call(
            &data,
            "set_thread_approval_mode",
            json!({"threadId": thread, "mode": "custom"}),
        );
        call(
            &data,
            "send_message",
            json!({"threadId": thread, "text": "list the files"}),
        );
        let mut pending = Value::Null;
        until("the subagent never asked", || {
            pending = call(&data, "get_pending_approvals", json!({}));
            pending.as_array().is_some_and(|list| !list.is_empty())
        });
        assert_eq!(pending[0]["subagent"]["name"], "Lister", "{pending}");
        // Its approval belongs to the subagent; the reply itself is not waiting on you.
        let tasks = call(&data, "get_thread_tasks", json!({"threadId": thread}));
        assert_eq!(tasks["run"]["awaiting_approval"], false, "{tasks}");
        assert_eq!(tasks["run"]["running"], true);
        let id = call(&data, "list_subagents", json!({"threadId": thread}))[0]["id"]
            .as_str()
            .unwrap()
            .to_owned();
        call(
            &data,
            "stop_subagent",
            json!({"threadId": thread, "subagentId": id}),
        );
        until("the run never finished", || !running(&data, &thread));
    })
    .await
    .unwrap();
    app.serving.abort();
}

// Subagent profiles from plugins (`agents/*.md`).

const PROFILE_BODY: &str = "PROFILE-BODY: review like a careful maintainer.";

/// A plugin folder under `parent`, with its manifest and these files.
fn plugin_folder(parent: &Path, name: &str, files: &[(&str, &str)]) -> std::path::PathBuf {
    let root = parent.join(name);
    std::fs::create_dir_all(root.join(".claude-plugin")).unwrap();
    std::fs::write(
        root.join(".claude-plugin/plugin.json"),
        json!({"name": name, "description": "A test plugin"}).to_string(),
    )
    .unwrap();
    for (path, content) in files {
        let path = root.join(path);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, content).unwrap();
    }
    root
}

fn agent_file(name: &str, extra_frontmatter: &str) -> String {
    format!(
        "---\nname: {name}\ndescription: Reviews code for bugs.\n{extra_frontmatter}---\n{PROFILE_BODY}\n"
    )
}

/// Runs the `themis` CLI; the exit status, the parsed output and what it printed as an error.
fn themis(data: &Path, args: &[&str]) -> (bool, Value, String) {
    let output = std::process::Command::new(env!("CARGO_BIN_EXE_themis"))
        .arg("--data-dir")
        .arg(data)
        .args(args)
        .env_remove("OPENCODE_KEY")
        .output()
        .expect("CLI");
    (
        output.status.success(),
        serde_json::from_slice(&output.stdout).unwrap_or(Value::Null),
        String::from_utf8_lossy(&output.stderr).into_owned(),
    )
}

fn import_plugin(data: &Path, folder: &Path) -> Value {
    let (ok, plugin, error) = themis(data, &["plugin", "import", folder.to_str().unwrap()]);
    assert!(ok, "import failed: {error}");
    plugin
}

fn profile_names(plugin: &Value) -> Vec<String> {
    plugin["spec"]["subagents"]
        .as_array()
        .map(|list| {
            list.iter()
                .filter_map(|profile| profile["name"].as_str().map(str::to_owned))
                .collect()
        })
        .unwrap_or_default()
}

/// A main agent that spawns `subagents` once and then finishes; `child` answers subagents.
fn spawning(
    subagents: Value,
    child: impl Fn(&Value) -> ResponseTemplate + Send + Sync + 'static,
) -> impl Fn(&Value) -> ResponseTemplate + Send + Sync + 'static {
    move |body| {
        if is_subagent(body) {
            return child(body);
        }
        match tool_results(body) {
            0 => tools(&[(
                "s1",
                "spawn_subagents",
                json!({"say": "Starting the review.", "subagents": subagents}),
            )]),
            _ => reply("Done."),
        }
    }
}

async fn run_in_yolo(app: &App, task: &'static str) {
    let (data, thread) = (app.data.path().to_owned(), app.thread.clone());
    tokio::task::spawn_blocking(move || run(&data, &thread, task))
        .await
        .unwrap();
}

async fn on_cli<T: Send + 'static>(work: impl FnOnce() -> T + Send + 'static) -> T {
    tokio::task::spawn_blocking(work).await.unwrap()
}

fn spawn_tool(body: &Value) -> Value {
    body["tools"]
        .as_array()
        .and_then(|tools| {
            tools
                .iter()
                .find(|tool| tool["function"]["name"] == "spawn_subagents")
        })
        .map_or(Value::Null, |tool| tool["function"].clone())
}

/// What the main agent's spawn call returned.
fn spawn_result(all: &[Value]) -> String {
    all.iter()
        .filter(|body| !is_subagent(body))
        .flat_map(|body| messages(body).iter())
        .find(|message| message["role"] == "tool")
        .map(|message| text(message).to_owned())
        .unwrap_or_default()
}

fn sorted_tools(body: &Value) -> Vec<String> {
    let mut names = tool_names(body);
    names.sort();
    names
}

#[tokio::test(flavor = "multi_thread")]
async fn a_plugins_subagent_imports_spawns_and_runs_with_its_instructions() {
    let app = app(
        None,
        spawning(
            json!([{"name": "Review", "brief": "THE-BRIEF: look at notes.txt", "profile": "reviewer"}]),
            |_| reply("Reviewed: all fine."),
        ),
    )
    .await;
    let parent = tempfile::tempdir().unwrap();
    let folder = plugin_folder(
        parent.path(),
        "review-kit",
        &[(
            "agents/reviewer.md",
            &agent_file("reviewer", "tools: Read, Grep, Glob\nmodel: inherit\n"),
        )],
    );
    let data = app.data.path().to_owned();
    let plugin = on_cli({
        let data = data.clone();
        move || import_plugin(&data, &folder)
    })
    .await;
    assert_eq!(profile_names(&plugin), ["reviewer"], "{plugin}");
    // The profile lives in the plugin's own entry of the one registry.
    let registry = std::fs::read_to_string(data.join("plugins/registry.json")).unwrap();
    let registry: Value = serde_json::from_str(&registry).unwrap();
    assert_eq!(
        registry["current"]["review-kit"]["spec"]["subagents"][0]["name"], "reviewer",
        "{registry}"
    );
    run_in_yolo(&app, "review the project").await;
    let all = bodies(&app.llm).await;
    let main = all.iter().find(|body| !is_subagent(body)).unwrap();
    // The model can find the profile: its name and a short description are on the tool.
    let spawn = spawn_tool(main);
    let description = spawn["description"].as_str().unwrap();
    assert!(description.contains("reviewer"), "{description}");
    assert!(description.contains("Reviews code for bugs."));
    assert!(
        spawn["parameters"]["properties"]["subagents"]["items"]["properties"]["profile"]
            .is_object(),
        "{spawn}"
    );
    // The child got the body before the brief, and only what its list names.
    let child = all.iter().find(|body| is_subagent(body)).expect("a child");
    let whole: String = messages(child).iter().map(text).collect();
    let body_at = whole.find(PROFILE_BODY).expect("the profile's body");
    let brief_at = whole.find("THE-BRIEF").expect("the brief");
    assert!(body_at < brief_at, "{whole}");
    // In the system message, with the access it really has stated after it.
    let system = text(&messages(child)[0]);
    assert!(system.starts_with(SUBAGENT), "{system}");
    let role_at = system.find(PROFILE_BODY).expect("in the system message");
    let access_at = system
        .find(themis_core::prompts::SUBAGENT_READ_ONLY_NOTE)
        .expect("the access note");
    assert!(role_at < access_at, "{system}");
    assert_eq!(
        sorted_tools(child),
        ["list_dir", "read_file", "search_file"]
    );
    // Its result reached the main agent, and the listing names the profile.
    assert!(all.iter().any(|body| mentions(body, "Reviewed: all fine.")));
    let listed = call(
        app.data.path(),
        "list_subagents",
        json!({"threadId": app.thread}),
    );
    assert_eq!(listed[0]["profile"], "reviewer", "{listed}");
    app.serving.abort();
}

#[tokio::test(flavor = "multi_thread")]
async fn a_project_plugins_subagent_is_kept_in_the_project_and_runs() {
    let app = app(
        None,
        spawning(
            json!([{"name": "Review", "brief": "Look.", "profile": "local-reviewer"}]),
            |_| reply("Local review done."),
        ),
    )
    .await;
    let parent = tempfile::tempdir().unwrap();
    let folder = plugin_folder(
        parent.path(),
        "project-kit",
        &[(
            "agents/local-reviewer.md",
            &agent_file("local-reviewer", ""),
        )],
    );
    let (data, project) = (app.data.path().to_owned(), app.project.path().to_owned());
    let plugin = on_cli(move || {
        let (ok, plugin, error) = themis(
            &data,
            &[
                "plugin",
                "import",
                folder.to_str().unwrap(),
                "--scope",
                "local",
                "--project",
                project.to_str().unwrap(),
            ],
        );
        assert!(ok, "{error}");
        plugin
    })
    .await;
    assert_eq!(profile_names(&plugin), ["local-reviewer"], "{plugin}");
    assert!(
        std::fs::read_to_string(app.project.path().join(".themis/plugins/registry.json"))
            .unwrap()
            .contains("local-reviewer")
    );
    run_in_yolo(&app, "review the project").await;
    let all = bodies(&app.llm).await;
    let child = all.iter().find(|body| is_subagent(body)).expect("a child");
    assert!(mentions(child, PROFILE_BODY));
    assert!(all.iter().any(|body| mentions(body, "Local review done.")));
    app.serving.abort();
}

/// An MCP server with one tool, `lookup`.
async fn lookup_server() -> MockServer {
    let mcp = MockServer::builder().start().await;
    Mock::given(method("POST"))
        .respond_with(|request: &Request| {
            let body: Value = serde_json::from_slice(&request.body).unwrap();
            let result = match body["method"].as_str().unwrap_or_default() {
                "initialize" => json!({"protocolVersion": "2025-03-26", "capabilities": {},
                    "serverInfo": {"name": "docs", "version": "1"}}),
                "tools/list" => json!({"tools": [{"name": "lookup", "description": "x",
                    "inputSchema": {"type": "object", "properties": {}}}]}),
                _ => json!({}),
            };
            ResponseTemplate::new(200)
                .set_body_json(json!({"jsonrpc": "2.0", "id": body["id"], "result": result}))
        })
        .mount(&mcp)
        .await;
    mcp
}

#[tokio::test(flavor = "multi_thread")]
async fn a_tool_list_only_narrows() {
    let mcp = lookup_server().await;
    let app = app(
        None,
        spawning(
            json!([
                {"name": "Named", "brief": "names mcp", "profile": "names-mcp"},
                {"name": "Omits", "brief": "omits mcp", "profile": "omits-mcp"},
                {"name": "Scoped", "brief": "scoped shell", "profile": "scoped-shell"},
                {"name": "Denied", "brief": "denied shell", "profile": "no-shell"},
                {"name": "Plain", "brief": "no list", "profile": "no-list"}
            ]),
            |_| reply("Done looking."),
        ),
    )
    .await;
    let parent = tempfile::tempdir().unwrap();
    let names_mcp = agent_file(
        "names-mcp",
        "tools:\n  - Read\n  - Bash\n  - WebFetch\n  - mcp__docs__lookup\n",
    );
    let omits_mcp = agent_file("omits-mcp", "tools: [\"Read\"]\n");
    let scoped = agent_file("scoped-shell", "tools: Read, Bash(git diff:*)\n");
    let no_shell = agent_file("no-shell", "disallowedTools: Bash(rm:*)\n");
    let no_list = agent_file("no-list", "");
    let mcp_file = json!({"mcpServers": {"docs": {"type": "http", "url": mcp.uri()}}}).to_string();
    let folder = plugin_folder(
        parent.path(),
        "narrow-kit",
        &[
            ("agents/names-mcp.md", &names_mcp),
            ("agents/omits-mcp.md", &omits_mcp),
            ("agents/scoped-shell.md", &scoped),
            ("agents/no-shell.md", &no_shell),
            ("agents/no-list.md", &no_list),
            (".mcp.json", &mcp_file),
        ],
    );
    let data = app.data.path().to_owned();
    let report = on_cli(move || {
        let (ok, report, error) = themis(&data, &["plugin", "inspect", folder.to_str().unwrap()]);
        assert!(ok, "{error}");
        let (ok, _, error) = themis(
            &data,
            &[
                "plugin",
                "import",
                folder.to_str().unwrap(),
                "--allow-partial",
            ],
        );
        assert!(ok, "{error}");
        let (ok, _, error) = themis(&data, &["mcp", "enable", "narrow-kit", "docs"]);
        assert!(ok, "{error}");
        report
    })
    .await;
    // Every name here means something, so nothing is dropped.
    let reasons = report["spec"]["unsupported"].to_string();
    assert!(!reasons.contains("mcp__docs__lookup"), "{reasons}");
    assert!(!reasons.contains("Bash(git diff:*)"), "{reasons}");

    run_in_yolo(&app, "review the project").await;
    let all = bodies(&app.llm).await;
    let main = all.iter().find(|body| !is_subagent(body)).unwrap();
    assert!(
        tool_names(main).iter().any(|name| name.contains("lookup")),
        "the main agent has the MCP tool: {:?}",
        tool_names(main)
    );
    let tools_of = |brief: &str| {
        let child = all
            .iter()
            .find(|body| is_subagent(body) && mentions(body, brief))
            .unwrap_or_else(|| panic!("no child for {brief}"));
        // What MCP tools they get has its own tests (`subagent_mcp_cli_e2e`).
        let mut names = sorted_tools(child);
        names.retain(|name| !name.starts_with("mcp_"));
        names
    };
    let reads = ["list_dir", "read_file", "search_file"];
    // What the list names, and no more.
    assert_eq!(
        tools_of("names mcp"),
        ["fetch_url", "list_dir", "read_file", "search_file", "shell"]
    );
    assert_eq!(tools_of("omits mcp"), reads);
    // A scoped Bash(...) gives the shell, held to its commands (`subagent_tools_cli_e2e`).
    assert_eq!(
        tools_of("scoped shell"),
        ["list_dir", "read_file", "search_file", "shell"]
    );
    // A list that denies part of the shell loses the shell.
    assert_eq!(
        tools_of("denied shell"),
        ["fetch_url", "list_dir", "read_file", "search_file"]
    );
    // No list: a subagent's usual read-only tools, and the web (this test has no search address).
    assert_eq!(
        tools_of("no list"),
        ["fetch_url", "list_dir", "read_file", "search_file", "shell"]
    );
    app.serving.abort();
}

#[tokio::test(flavor = "multi_thread")]
async fn a_profile_listing_write_still_needs_write_paths() {
    let child = |body: &Value| match tool_results(body) {
        0 => tools(&[(
            "w1",
            "write_file",
            json!({"file_path": if mentions(body, "with paths") { "out/made.txt" } else { "stray.txt" },
                "content": "written"}),
        )]),
        _ => reply("Finished writing."),
    };
    let app = app(
        None,
        spawning(
            json!([
                {"name": "Without", "brief": "no paths given", "profile": "writer"},
                {"name": "With", "brief": "with paths given", "profile": "writer", "write_paths": ["out/"]}
            ]),
            child,
        ),
    )
    .await;
    let parent = tempfile::tempdir().unwrap();
    let folder = plugin_folder(
        parent.path(),
        "write-kit",
        &[(
            "agents/writer.md",
            &agent_file("writer", "tools: Read, Write, Edit\n"),
        )],
    );
    let data = app.data.path().to_owned();
    on_cli(move || import_plugin(&data, &folder)).await;
    run_in_yolo(&app, "write something").await;
    let all = bodies(&app.llm).await;
    let without = all
        .iter()
        .find(|body| is_subagent(body) && mentions(body, "no paths given"))
        .expect("the read-only child");
    assert_eq!(
        sorted_tools(without),
        ["list_dir", "read_file", "search_file"]
    );
    let with = all
        .iter()
        .find(|body| is_subagent(body) && mentions(body, "with paths given"))
        .expect("the writing child");
    assert_eq!(
        sorted_tools(with),
        [
            "apply_patch",
            "create_dir",
            "list_dir",
            "read_file",
            "search_file",
            "write_file"
        ]
    );
    let root = app.project.path();
    assert!(!root.join("stray.txt").exists(), "no write without paths");
    assert_eq!(
        std::fs::read_to_string(root.join("out/made.txt")).unwrap(),
        "written"
    );
    app.serving.abort();
}

#[tokio::test(flavor = "multi_thread")]
async fn write_paths_for_a_profile_without_write_tools_are_refused() {
    let app = app(
        None,
        spawning(
            json!([{"name": "Reader", "brief": "x", "profile": "reader", "write_paths": ["out/"]}]),
            |_| reply("never"),
        ),
    )
    .await;
    let parent = tempfile::tempdir().unwrap();
    let folder = plugin_folder(
        parent.path(),
        "read-kit",
        &[("agents/reader.md", &agent_file("reader", "tools: Read\n"))],
    );
    let data = app.data.path().to_owned();
    on_cli(move || import_plugin(&data, &folder)).await;
    run_in_yolo(&app, "go").await;
    let all = bodies(&app.llm).await;
    assert!(all.iter().all(|body| !is_subagent(body)), "nothing started");
    let result = spawn_result(&all);
    assert!(result.contains("reader"), "{result}");
    assert!(result.contains("write_paths"), "{result}");
    app.serving.abort();
}

#[tokio::test(flavor = "multi_thread")]
async fn an_unknown_profile_fails_clearly_and_starts_nothing() {
    let app = app(
        None,
        spawning(
            json!([
                {"name": "Good", "brief": "fine", "profile": "reviewer"},
                {"name": "Bad", "brief": "broken", "profile": "no-such-profile"}
            ]),
            |_| reply("never"),
        ),
    )
    .await;
    let parent = tempfile::tempdir().unwrap();
    let folder = plugin_folder(
        parent.path(),
        "review-kit",
        &[("agents/reviewer.md", &agent_file("reviewer", ""))],
    );
    let data = app.data.path().to_owned();
    on_cli(move || import_plugin(&data, &folder)).await;
    run_in_yolo(&app, "go").await;
    let all = bodies(&app.llm).await;
    assert!(all.iter().all(|body| !is_subagent(body)), "nothing started");
    let result = spawn_result(&all);
    assert!(result.contains("no-such-profile"), "{result}");
    assert!(result.contains("reviewer"), "names the choices: {result}");
    app.serving.abort();
}

#[tokio::test(flavor = "multi_thread")]
async fn a_disabled_plugins_profiles_are_not_available() {
    let app = app(
        None,
        spawning(
            json!([{"name": "Review", "brief": "x", "profile": "reviewer"}]),
            |_| reply("never"),
        ),
    )
    .await;
    let parent = tempfile::tempdir().unwrap();
    let folder = plugin_folder(
        parent.path(),
        "review-kit",
        &[("agents/reviewer.md", &agent_file("reviewer", ""))],
    );
    let data = app.data.path().to_owned();
    on_cli(move || {
        import_plugin(&data, &folder);
        let (ok, _, error) = themis(&data, &["plugin", "disable", "review-kit"]);
        assert!(ok, "{error}");
    })
    .await;
    run_in_yolo(&app, "go").await;
    let all = bodies(&app.llm).await;
    assert!(all.iter().all(|body| !is_subagent(body)), "nothing started");
    let spawn = spawn_tool(all.first().unwrap());
    assert!(
        !spawn["description"].as_str().unwrap().contains("reviewer"),
        "not offered: {spawn}"
    );
    assert!(
        spawn["parameters"]["properties"]["subagents"]["items"]["properties"]["profile"].is_null(),
        "{spawn}"
    );
    let result = spawn_result(&all);
    assert!(result.contains("reviewer"), "{result}");
    assert!(result.to_lowercase().contains("profile"), "{result}");
    app.serving.abort();
}

#[tokio::test(flavor = "multi_thread")]
async fn a_profiles_model_maps_to_the_models_list_or_the_default_applies() {
    let app = app(
        None,
        spawning(
            json!([
                {"name": "Listed", "brief": "a", "profile": "listed"},
                {"name": "Unlisted", "brief": "b", "profile": "unlisted"},
                {"name": "Inherit", "brief": "c", "profile": "inherits"},
                {"name": "Asked", "brief": "d", "profile": "listed", "model": "test-model"}
            ]),
            |_| reply("ok"),
        ),
    )
    .await;
    let parent = tempfile::tempdir().unwrap();
    let folder = plugin_folder(
        parent.path(),
        "model-kit",
        &[
            ("agents/listed.md", &agent_file("listed", "model: haiku\n")),
            (
                "agents/unlisted.md",
                &agent_file("unlisted", "model: opus\n"),
            ),
            (
                "agents/inherits.md",
                &agent_file("inherits", "model: inherit\n"),
            ),
        ],
    );
    let data = app.data.path().to_owned();
    on_cli(move || {
        allow_models(&data, &["claude-haiku-4"]);
        import_plugin(&data, &folder);
    })
    .await;
    run_in_yolo(&app, "go").await;
    let listed = call(
        app.data.path(),
        "list_subagents",
        json!({"threadId": app.thread}),
    );
    let model_of = |name: &str| {
        listed
            .as_array()
            .unwrap()
            .iter()
            .find(|agent| agent["name"] == name)
            .unwrap_or_else(|| panic!("{name}: {listed}"))["model"]
            .as_str()
            .unwrap()
            .to_owned()
    };
    assert_eq!(model_of("Listed"), "claude-haiku-4");
    assert_eq!(model_of("Unlisted"), "test-model");
    assert_eq!(model_of("Inherit"), "test-model");
    // What the main agent asks for wins over what the profile says.
    assert_eq!(model_of("Asked"), "test-model");
    app.serving.abort();
}

#[tokio::test(flavor = "multi_thread")]
async fn malformed_subagent_definitions_fail_the_import() {
    let data = tempfile::tempdir().unwrap();
    let state = AppState::new_for_test(data.path().join("settings.json"));
    let serving = tokio::spawn(Server::bind(state, data.path()).await.unwrap().run());
    let parent = tempfile::tempdir().unwrap();
    let cases = [
        ("unclosed", "---\nname: x\ndescription: y\nbody"),
        ("no-frontmatter", "Just some text."),
        ("no-name", "---\ndescription: y\n---\nbody"),
        ("no-description", "---\nname: x\n---\nbody"),
        (
            "unsafe-name",
            "---\nname: ../../evil\ndescription: y\n---\nbody",
        ),
        ("empty-body", "---\nname: x\ndescription: y\n---\n"),
        (
            "unclosed-list",
            "---\nname: x\ndescription: y\ntools: [Read, Bash\n---\nbody",
        ),
        (
            "two-lists",
            "---\nname: x\ndescription: y\ntools: Read\ntools: Bash\n---\nbody",
        ),
    ];
    for (case, content) in cases {
        let folder = plugin_folder(
            parent.path(),
            case,
            &[
                ("agents/one.md", content),
                (
                    "skills/s/SKILL.md",
                    "---\nname: s\ndescription: d\n---\nUse.",
                ),
            ],
        );
        let path = data.path().to_owned();
        let (ok, plugin, error) = on_cli(move || {
            themis(
                &path,
                &[
                    "plugin",
                    "import",
                    folder.to_str().unwrap(),
                    "--allow-partial",
                ],
            )
        })
        .await;
        assert!(!ok, "{case} should fail: {plugin}");
        assert!(
            error.contains("agents/one.md"),
            "{case} names the file: {error}"
        );
    }
    let path = data.path().to_owned();
    let (_, listed, _) = on_cli(move || themis(&path, &["plugin", "list"])).await;
    assert_eq!(listed, json!([]), "nothing was saved");
    // Two files with one name are a bad definition too.
    let folder = plugin_folder(
        parent.path(),
        "duplicates",
        &[
            ("agents/one.md", &agent_file("same", "")),
            ("agents/two.md", &agent_file("same", "")),
        ],
    );
    let path = data.path().to_owned();
    let (ok, plugin, _) =
        on_cli(move || themis(&path, &["plugin", "import", folder.to_str().unwrap()])).await;
    assert!(!ok, "{plugin}");
    serving.abort();
}

#[tokio::test(flavor = "multi_thread")]
async fn subagent_paths_that_leave_the_plugin_are_not_imported() {
    let data = tempfile::tempdir().unwrap();
    let state = AppState::new_for_test(data.path().join("settings.json"));
    let serving = tokio::spawn(Server::bind(state, data.path()).await.unwrap().run());
    let parent = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(parent.path().join("outside")).unwrap();
    std::fs::write(
        parent.path().join("outside/evil.md"),
        agent_file("evil", ""),
    )
    .unwrap();
    let folder = plugin_folder(
        parent.path(),
        "escape-kit",
        &[("agents/good.md", &agent_file("good", ""))],
    );
    std::fs::write(
        folder.join(".claude-plugin/plugin.json"),
        json!({"name": "escape-kit", "agents": ["../outside/evil.md", "./agents/../../outside/"]})
            .to_string(),
    )
    .unwrap();
    #[cfg(unix)]
    std::os::unix::fs::symlink(
        parent.path().join("outside/evil.md"),
        folder.join("agents/link.md"),
    )
    .unwrap();
    let path = data.path().to_owned();
    let (ok, plugin, error) = on_cli(move || {
        themis(
            &path,
            &[
                "plugin",
                "import",
                folder.to_str().unwrap(),
                "--allow-partial",
            ],
        )
    })
    .await;
    assert!(ok, "{error}");
    assert_eq!(profile_names(&plugin), ["good"], "{plugin}");
    let unsupported = plugin["spec"]["unsupported"].to_string();
    assert!(unsupported.contains("outside the plugin"), "{unsupported}");
    #[cfg(unix)]
    assert!(unsupported.contains("Link not followed"), "{unsupported}");
    serving.abort();
}
