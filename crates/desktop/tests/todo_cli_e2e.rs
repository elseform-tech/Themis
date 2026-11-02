//! The main agent's to-do list through the shared server and the `themis` CLI, with a scripted
//! model: `todo_write` keeps it, it survives a restart and a context checkpoint and is read back
//! to the model, and in a goal the check referees it (reopens an item, sees a request the person
//! sent mid-goal in their words) and the goal ends only when the check agrees and nothing is open.

use std::path::Path;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use serde_json::{json, Value};
use themis_desktop::server::{Client, Server};
use themis_desktop::sink::ChannelSink;
use themis_desktop::state::AppState;
use themis_desktop::types::{GoalStatus, ProviderKind};
use wiremock::{matchers::method, Mock, MockServer, Request, ResponseTemplate};

const CHECK_MARKER: &str = "You decide whether an agent has reached a goal";
const NOTE_MARKER: &str = "[To-do — runtime, not from the user]";

fn cli(dir: &Path, command: &str, args: Value) -> Value {
    use std::io::Write;
    use std::process::Stdio;
    let mut child = std::process::Command::new(env!("CARGO_BIN_EXE_themis"))
        .arg("--data-dir")
        .arg(dir)
        .args(["call", command, "-"])
        .env_remove("OPENCODE_KEY")
        .env("HOME", dir)
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
        "{command}: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    serde_json::from_slice(&output.stdout).unwrap_or(Value::Null)
}

fn text_reply(text: &str) -> ResponseTemplate {
    ResponseTemplate::new(200).set_body_json(
        json!({"choices":[{"message":{"role":"assistant","content":text},"finish_reason":"stop"}]}),
    )
}

fn call(name: &str, arguments: Value) -> ResponseTemplate {
    ResponseTemplate::new(200).set_body_json(json!({"choices":[{"message":{"role":"assistant","content":null,"tool_calls":[{"id":"call_1","type":"function","function":{"name":name,"arguments":arguments.to_string()}}]},"finish_reason":"tool_calls"}]}))
}

fn system(body: &Value) -> &str {
    body["messages"][0]["content"].as_str().unwrap_or_default()
}

fn is_summary(body: &Value) -> bool {
    system(body).contains("You summarize agent context")
}

fn is_check(body: &Value) -> bool {
    system(body).contains(CHECK_MARKER)
}

fn last_is_tool(body: &Value) -> bool {
    body["messages"]
        .as_array()
        .and_then(|messages| messages.last())
        .is_some_and(|last| last["role"] == "tool")
}

/// Sends one message through the CLI and waits for its run to end; the event kinds it saw.
async fn send(
    data: &Path,
    events: &mut tokio::net::TcpStream,
    id: &str,
    text: &str,
) -> Vec<String> {
    cli(
        data,
        "send_message",
        json!({"threadId":id,"text":text,"reasoningEffort":null}),
    );
    tokio::time::timeout(Duration::from_secs(300), async {
        let mut kinds = Vec::new();
        loop {
            let item = Client::next_event(events).await.unwrap();
            if item["name"] != "thread-event" || item["payload"]["thread_id"] != id {
                continue;
            }
            let kind = item["payload"]["event"]["kind"]
                .as_str()
                .unwrap_or_default()
                .to_owned();
            kinds.push(kind.clone());
            if ["finished", "failed", "incomplete"].contains(&kind.as_str()) {
                assert_eq!(kind, "finished", "{}", item["payload"]["event"]);
                return kinds;
            }
        }
    })
    .await
    .expect("the run ended")
}

async fn start(
    provider: &MockServer,
    data: &Path,
) -> (
    tokio::net::TcpStream,
    tokio::task::JoinHandle<Result<(), String>>,
) {
    let state = AppState::new_for_test(data.join("settings.json"));
    state
        .set_secret("go".into(), "synthetic-test-key".into())
        .await
        .unwrap();
    state.set_go_base_url_override(Some(provider.uri()));
    let server = Server::bind(state, data).await.unwrap();
    let serving = tokio::spawn(server.run());
    let events = Client::new(data.into()).subscribe().await.unwrap();
    (events, serving)
}

/// Filler that pushes the 24k-token budget toward a checkpoint within a few messages.
fn filler(index: usize) -> String {
    format!(
        "note {index}: {}",
        format!("detail {index} about beds, soil and watering; ").repeat(220)
    )
}

#[tokio::test(flavor = "multi_thread")]
async fn cli_keeps_the_agents_list_across_a_restart_and_a_checkpoint_and_reads_it_back() {
    let data = tempfile::tempdir().unwrap();
    let project = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(project.path().join(".themis")).unwrap();
    std::fs::write(
        project.path().join(".themis/config.jsonc"),
        r#"{"context_token_budget":24000,"recent_messages":4,"total_turns":8}"#,
    )
    .unwrap();
    let provider = MockServer::builder().start().await;
    let ordinary = Arc::new(Mutex::new(Vec::<Value>::new()));
    let seen = Arc::clone(&ordinary);
    Mock::given(method("POST"))
        .respond_with(move |request: &Request| {
            let body: Value = request.body_json().unwrap();
            if is_summary(&body) {
                return text_reply("Goal/constraints: plan the garden. Unresolved work: finish the garden plan. Parked work: none. Next action: continue the garden plan if asked.");
            }
            seen.lock().unwrap().push(body.clone());
            let wire = body["messages"].to_string();
            if last_is_tool(&body) {
                return text_reply("Planned.");
            }
            if wire.contains("Plan the garden") && !wire.contains(NOTE_MARKER) {
                return call(
                    "todo_write",
                    json!({"todos":[
                        {"title":"Measure the beds","status":"in_progress","details":"all four beds, in metres"},
                        {"title":"Choose the plants","status":"pending"}]}),
                );
            }
            text_reply("noted")
        })
        .mount(&provider)
        .await;
    let (mut events, serving) = start(&provider, data.path()).await;
    let thread = cli(
        data.path(),
        "create_thread",
        json!({"projectRoot":project.path(),"provider":"go","model":"test-model"}),
    );
    let id = thread["id"].as_str().unwrap().to_owned();
    send(data.path(), &mut events, &id, "Plan the garden").await;
    let list = cli(data.path(), "get_todo", json!({"threadId": id}));
    assert_eq!(list["items"][0]["id"], "t1", "{list}");
    assert_eq!(list["items"][0]["title"], "Measure the beds");
    assert_eq!(list["items"][0]["status"], "in_progress");
    assert_eq!(list["items"][1]["title"], "Choose the plants");
    // The list is saved beside the chat, outside its history.
    assert!(data
        .path()
        .join("todos")
        .join(format!("{id}.json"))
        .exists());

    // A restart keeps it, and the next run is told it.
    serving.abort();
    drop(events);
    tokio::time::sleep(Duration::from_millis(300)).await;
    let (mut events, serving) = start(&provider, data.path()).await;
    let list = cli(data.path(), "get_todo", json!({"threadId": id}));
    assert_eq!(list["items"].as_array().unwrap().len(), 2, "{list}");
    send(data.path(), &mut events, &id, "What's next?").await;
    let latest = ordinary.lock().unwrap().last().unwrap().clone();
    let task = latest["messages"]
        .as_array()
        .unwrap()
        .iter()
        .rev()
        .find(|message| message["role"] == "user")
        .unwrap()["content"]
        .as_str()
        .unwrap()
        .to_owned();
    assert!(task.starts_with("What's next?"), "{task}");
    assert!(task.contains(NOTE_MARKER), "{task}");
    assert!(
        task.contains("- t1 [in progress] Measure the beds\n  details: all four beds, in metres"),
        "{task}"
    );

    // After a checkpoint the model reads the list again, as it stands then.
    let mut checkpointed = false;
    for index in 0..8 {
        let kinds = send(data.path(), &mut events, &id, &filler(index)).await;
        if kinds.iter().any(|kind| kind == "context_checkpoint") {
            checkpointed = true;
            break;
        }
    }
    assert!(checkpointed, "the long chat never compacted");
    let latest = ordinary.lock().unwrap().last().unwrap().clone();
    let messages = latest["messages"].as_array().unwrap();
    let checkpoint = messages
        .iter()
        .position(|message| {
            message["content"]
                .as_str()
                .is_some_and(|text| text.starts_with("Earlier context checkpoint"))
        })
        .expect("the request after the checkpoint carries it");
    let note = messages[checkpoint..]
        .iter()
        .find(|message| {
            message["role"] == "user"
                && message["content"]
                    .as_str()
                    .is_some_and(|text| text.starts_with(NOTE_MARKER))
        })
        .expect("the list is read back after the checkpoint");
    assert!(note["content"]
        .as_str()
        .unwrap()
        .contains("Choose the plants"));
    serving.abort();
}

async fn chat(server: &MockServer) -> (AppState, tempfile::TempDir, tempfile::TempDir, String) {
    let data = tempfile::tempdir().unwrap();
    let project = tempfile::tempdir().unwrap();
    let state = AppState::new_for_test(data.path().join("settings.json"));
    state
        .set_secret("go".into(), "synthetic-test-key".into())
        .await
        .unwrap();
    state.set_go_base_url_override(Some(server.uri()));
    let thread = state
        .create_thread(
            project.path().to_string_lossy().into_owned(),
            ProviderKind::Go,
            Some("test-model".into()),
        )
        .await
        .unwrap();
    (state, data, project, thread.id)
}

async fn goal_until(state: &AppState, thread: &str, status: GoalStatus) {
    let reached = tokio::time::timeout(Duration::from_secs(120), async {
        while state.goal_for(thread).map(|goal| goal.status) != Some(status) {
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
    })
    .await;
    assert!(reached.is_ok(), "goal: {:?}", state.goal_for(thread));
}

/// The check reopens an item the agent finished without showing it, the agent finishes it
/// again, and only then, with nothing open, is the goal reached. A request the person steered
/// in mid-goal lands on the list in their words, and the check reads it.
#[tokio::test(flavor = "multi_thread")]
async fn the_check_referees_the_list_and_a_request_sent_mid_goal_becomes_an_item() {
    const REQUEST: &str = "Also export the board as CSV, with the due dates in ISO format please.";
    let server = MockServer::builder().start().await;
    let checks = Arc::new(Mutex::new(Vec::<Value>::new()));
    let checked = Arc::clone(&checks);
    let gate = Arc::new(std::sync::atomic::AtomicBool::new(false));
    let held = Arc::clone(&gate);
    let turns = Arc::new(AtomicUsize::new(0));
    let turn = Arc::clone(&turns);
    Mock::given(method("POST"))
        .respond_with(move |request: &Request| {
            let body: Value = request.body_json().unwrap();
            if is_check(&body) {
                let count = {
                    let mut checks = checked.lock().unwrap();
                    checks.push(body);
                    checks.len()
                };
                return text_reply(&match count {
                    1 => r#"{"verdict":"reached","reason":"Looks done.","reopen":[{"id":"t2","reason":"dragging was never tried with the mouse"}],"progressed":true}"#.to_owned(),
                    _ => r#"{"verdict":"reached","reason":"The board drags and the CSV export works.","progressed":true}"#.to_owned(),
                });
            }
            if last_is_tool(&body) {
                return text_reply("Done for this turn.");
            }
            let wire = body["messages"].to_string();
            match turn.fetch_add(1, Ordering::SeqCst) {
                0 => {
                    let waited = std::time::Instant::now();
                    while !held.load(Ordering::SeqCst) && waited.elapsed() < Duration::from_secs(60) {
                        std::thread::sleep(Duration::from_millis(10));
                    }
                    call("todo_write", json!({"todos":[{"title":"Build the board","status":"completed","outcome":"cards drag"}]}))
                }
                _ => {
                    // The agent finishes what its list (read back at the turn's start) holds.
                    let csv = wire.contains("[pending; the person asked]");
                    let mut todos = vec![json!({"id":"t2","title":"Build the board","status":"completed","outcome":"dragged a card with the mouse"})];
                    if csv {
                        todos.push(json!({"id":"t1","title":"CSV export","status":"completed","outcome":"exported"}));
                    }
                    call("todo_write", json!({ "todos": todos }))
                }
            }
        })
        .mount(&server)
        .await;
    let (state, data, _project, thread) = chat(&server).await;
    let (sink, _rx) = ChannelSink::channel();
    let sink = Arc::new(sink);
    state
        .send_message(
            sink.clone(),
            thread.clone(),
            "/goal Build the board.".into(),
        )
        .await
        .unwrap();
    let steer = state
        .enqueue_message(sink.clone(), thread.clone(), REQUEST.into(), None, vec![])
        .await
        .unwrap();
    state
        .steer_queued_message(sink.clone(), thread.clone(), steer.id)
        .await
        .unwrap();
    let list = state.todo_for(&thread);
    let asked = list
        .items
        .iter()
        .find(|item| item.details == REQUEST)
        .expect("the request is on the list at once, in the person's words");
    assert_eq!(asked.source, themis_core::todo::TodoSource::Person);
    gate.store(true, Ordering::SeqCst);

    // The first check doesn't accept the board: it is reopened, and the goal goes on.
    let reopened = tokio::time::timeout(Duration::from_secs(120), async {
        loop {
            let list = state.todo_for(&thread);
            if let Some(reason) = list.items.iter().find_map(|item| item.reopened.clone()) {
                return reason;
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    })
    .await
    .expect("the check reopened the board");
    assert_eq!(reopened, "dragging was never tried with the mouse");
    goal_until(&state, &thread, GoalStatus::Reached).await;
    let checks = checks.lock().unwrap().clone();
    assert!(checks.len() >= 2, "reached only on a later check");
    let first = checks[0]["messages"][1]["content"].as_str().unwrap();
    assert!(
        first.contains("- t2 [done] Build the board — outcome: cards drag"),
        "{first}"
    );
    assert!(
        first.contains(REQUEST),
        "the check reads their words: {first}"
    );
    let list = state.todo_for(&thread);
    assert!(list.items.iter().all(|item| !item.is_open()), "{list:?}");
    assert!(list.items.iter().all(|item| item.reopened.is_none()));
    // Metadata only in the logs: counts, never the words.
    tokio::time::sleep(Duration::from_millis(200)).await;
    let logs = std::fs::read_dir(data.path().join("logs"))
        .map(|entries| {
            entries
                .flatten()
                .map(|entry| std::fs::read_to_string(entry.path()).unwrap_or_default())
                .collect::<String>()
        })
        .unwrap_or_default();
    assert!(logs.contains("check_refereed"), "{logs}");
    assert!(logs.contains("request_added"), "{logs}");
    assert!(!logs.contains("ISO format"), "no item text in the logs");
}

/// Live: a free model never called `todo_write`, so every request the person sent stayed
/// pending though the work was built, and the goal looped. Here the agent never touches its
/// list: the check confirms what the evidence shows done, closes a request a later one
/// cancelled and a side question the reply answered, and the goal is reached. The person's
/// words stay as they sent them, and every closed item says why.
#[tokio::test(flavor = "multi_thread")]
async fn the_check_closes_items_an_agent_that_never_writes_its_list_left_open() {
    use themis_core::todo::{CheckClosure, TodoSource, TodoStatus};
    const SELECTOR: &str = "Add a currency selector to the budget page.";
    const QUESTION: &str = "Side question, no need to change anything: what's the difference between localStorage and sessionStorage?";
    const CANCEL: &str = "Actually, drop the currency selector.";
    let server = MockServer::builder().start().await;
    let checks = Arc::new(Mutex::new(Vec::<Value>::new()));
    let checked = Arc::clone(&checks);
    let gate = Arc::new(std::sync::atomic::AtomicBool::new(false));
    let held = Arc::clone(&gate);
    let tool_calls = Arc::new(AtomicUsize::new(0));
    let called = Arc::clone(&tool_calls);
    Mock::given(method("POST"))
        .respond_with(move |request: &Request| {
            let body: Value = request.body_json().unwrap();
            if is_check(&body) {
                let count = {
                    let mut checks = checked.lock().unwrap();
                    checks.push(body);
                    checks.len()
                };
                return text_reply(match count {
                    1 => r#"{"verdict":"not_yet","reason":"The budget table isn't shown working yet.","next":"Run the table's test.",
                        "add":[{"title":"Budget table","details":"the table renders the entries"}],
                        "close":[{"id":"t1","why":"cancelled"},{"id":"t2","why":"answered"}],
                        "complete":[{"id":"t3","outcome":"the selector was never built, nothing to undo"}],"progressed":true}"#,
                    _ => r#"{"verdict":"reached","reason":"The table renders and its test passed.",
                        "complete":[{"id":"t4","outcome":"table test: 1 passed"}],"progressed":true,"told_user":true}"#,
                });
            }
            if last_is_tool(&body) {
                called.fetch_add(1, Ordering::SeqCst);
            }
            // The agent answers in words only and never writes its list.
            let waited = std::time::Instant::now();
            while !held.load(Ordering::SeqCst) && waited.elapsed() < Duration::from_secs(60) {
                std::thread::sleep(Duration::from_millis(10));
            }
            text_reply("Built the budget table. localStorage keeps data after the browser closes; sessionStorage is cleared when the tab closes. The table's test: 1 passed.")
        })
        .mount(&server)
        .await;
    let (state, data, _project, thread) = chat(&server).await;
    let (sink, _rx) = ChannelSink::channel();
    let sink = Arc::new(sink);
    state
        .send_message(
            sink.clone(),
            thread.clone(),
            "/goal Build the budget page.".into(),
        )
        .await
        .unwrap();
    for words in [SELECTOR, QUESTION, CANCEL] {
        let steer = state
            .enqueue_message(sink.clone(), thread.clone(), words.into(), None, vec![])
            .await
            .unwrap();
        state
            .steer_queued_message(sink.clone(), thread.clone(), steer.id)
            .await
            .unwrap();
    }
    let list = state.todo_for(&thread);
    let ids: Vec<(&str, &str)> = list
        .items
        .iter()
        .map(|item| (item.id.as_str(), item.details.as_str()))
        .collect();
    assert_eq!(
        ids,
        [("t1", SELECTOR), ("t2", QUESTION), ("t3", CANCEL)],
        "each request is on the list in their words"
    );
    gate.store(true, Ordering::SeqCst);

    goal_until(&state, &thread, GoalStatus::Reached).await;
    let checks = checks.lock().unwrap().clone();
    assert_eq!(
        checks.len(),
        2,
        "the first check held the goal for the table"
    );
    let system = checks[0]["messages"][0]["content"].as_str().unwrap();
    assert!(system.contains("\"close\":[{\"id\""), "{system}");
    assert!(
        system.contains("the agent saying so is not evidence"),
        "{system}"
    );
    // The second check reads who closed each item and why.
    let second = checks[1]["messages"][1]["content"].as_str().unwrap();
    assert!(
        second.contains("- t1 [closed by a check: cancelled by a later request; the person asked]"),
        "{second}"
    );
    assert!(
        second.contains("- t2 [closed by a check: answered; the person asked]"),
        "{second}"
    );
    assert!(
        second.contains("- t4 [pending; added by the check] Budget table"),
        "{second}"
    );

    let list = state.todo_for(&thread);
    assert_eq!(
        tool_calls.load(Ordering::SeqCst),
        0,
        "the agent never called a tool"
    );
    assert!(
        list.items
            .iter()
            .all(|item| item.status == TodoStatus::Completed),
        "{list:?}"
    );
    let closed: Vec<(Option<CheckClosure>, &str)> = list
        .items
        .iter()
        .map(|item| (item.closed, item.outcome.as_str()))
        .collect();
    assert_eq!(
        closed,
        [
            (Some(CheckClosure::Cancelled), ""),
            (Some(CheckClosure::Answered), ""),
            (
                Some(CheckClosure::Confirmed),
                "the selector was never built, nothing to undo"
            ),
            (Some(CheckClosure::Confirmed), "table test: 1 passed"),
        ]
    );
    assert_eq!(list.items[0].details, SELECTOR, "their words stay intact");
    assert_eq!(list.items[1].details, QUESTION);
    assert_eq!(list.items[3].source, TodoSource::Check);
    // Saved with the chat, so the closed states survive a restart.
    let saved: Value = serde_json::from_str(
        &std::fs::read_to_string(data.path().join("todos").join(format!("{thread}.json"))).unwrap(),
    )
    .unwrap();
    assert_eq!(saved["items"][0]["closed"], "cancelled", "{saved}");
    assert_eq!(saved["items"][1]["closed"], "answered", "{saved}");
    assert_eq!(saved["items"][3]["closed"], "confirmed", "{saved}");
    // Metadata only in the logs: counts, never the words.
    tokio::time::sleep(Duration::from_millis(200)).await;
    let logs = std::fs::read_dir(data.path().join("logs"))
        .map(|entries| {
            entries
                .flatten()
                .map(|entry| std::fs::read_to_string(entry.path()).unwrap_or_default())
                .collect::<String>()
        })
        .unwrap_or_default();
    assert!(logs.contains("\"confirmed\":1"), "{logs}");
    assert!(logs.contains("\"closed\":2"), "{logs}");
    assert!(!logs.contains("sessionStorage"), "no item text in the logs");
}

/// Waits on the event stream for a `todo_changed` whose list holds `words` with `status`.
async fn todo_event(events: &mut tokio::net::TcpStream, id: &str, words: &str, status: &str) {
    tokio::time::timeout(Duration::from_secs(120), async {
        loop {
            let item = Client::next_event(events).await.unwrap();
            let event = &item["payload"]["event"];
            if item["name"] != "thread-event"
                || item["payload"]["thread_id"] != id
                || event["kind"] != "todo_changed"
            {
                continue;
            }
            if event["todo"]["items"]
                .as_array()
                .into_iter()
                .flatten()
                .any(|item| item["details"] == words && item["status"] == status)
            {
                return;
            }
        }
    })
    .await
    .unwrap_or_else(|_| panic!("no todo_changed with the request {status}"));
}

/// The person's request in `list`, by their words.
fn request_item(list: &Value, words: &str) -> Value {
    list["items"]
        .as_array()
        .into_iter()
        .flatten()
        .find(|item| item["details"] == words)
        .cloned()
        .unwrap_or_else(|| panic!("the request is on the list: {list}"))
}

/// A queued request of the person's is in progress while the turn it started runs, even with a
/// model that never calls `todo_write`, and back to pending (never done) once that turn ends.
#[tokio::test(flavor = "multi_thread")]
async fn cli_a_delivered_request_is_in_progress_for_its_turn_and_pending_after() {
    const REQUEST: &str = "Also add a dark mode toggle to the settings page.";
    let data = tempfile::tempdir().unwrap();
    let project = tempfile::tempdir().unwrap();
    let provider = MockServer::builder().start().await;
    let first_gate = Arc::new(std::sync::atomic::AtomicBool::new(false));
    let request_gate = Arc::new(std::sync::atomic::AtomicBool::new(false));
    let (first, held) = (Arc::clone(&first_gate), Arc::clone(&request_gate));
    let wait = |gate: &std::sync::atomic::AtomicBool| {
        let waited = std::time::Instant::now();
        while !gate.load(Ordering::SeqCst) && waited.elapsed() < Duration::from_secs(60) {
            std::thread::sleep(Duration::from_millis(10));
        }
    };
    let requests = Arc::new(Mutex::new(Vec::<Value>::new()));
    let seen = Arc::clone(&requests);
    Mock::given(method("POST"))
        .respond_with(move |request: &Request| {
            let body: Value = request.body_json().unwrap();
            if is_check(&body) {
                return text_reply(
                    r#"{"verdict":"not_yet","reason":"Keep going.","progressed":true}"#,
                );
            }
            seen.lock().unwrap().push(body.clone());
            let last_user = body["messages"]
                .as_array()
                .and_then(|messages| messages.iter().rev().find(|m| m["role"] == "user"))
                .and_then(|message| message["content"].as_str())
                .unwrap_or_default()
                .to_owned();
            if last_user.starts_with("/goal") {
                wait(&first);
            } else if last_user.starts_with(REQUEST) {
                wait(&held);
            }
            // The model never touches its list.
            text_reply("Working on it.")
        })
        .mount(&provider)
        .await;
    let (mut events, serving) = start(&provider, data.path()).await;
    let thread = cli(
        data.path(),
        "create_thread",
        json!({"projectRoot":project.path(),"provider":"go","model":"test-model"}),
    );
    let id = thread["id"].as_str().unwrap().to_owned();
    cli(
        data.path(),
        "send_message",
        json!({"threadId": id, "text": "/goal Build the settings page.", "reasoningEffort": null}),
    );
    // The goal's first turn is running: the request waits in the queue.
    cli(
        data.path(),
        "enqueue_message",
        json!({"threadId": id, "text": REQUEST}),
    );
    first_gate.store(true, Ordering::SeqCst);

    // The queue delivers it: its turn starts and the request is on the list, in progress.
    todo_event(&mut events, &id, REQUEST, "in_progress").await;
    let list = cli(data.path(), "get_todo", json!({"threadId": id}));
    let item = request_item(&list, REQUEST);
    assert_eq!(item["source"], "person");
    assert_eq!(item["status"], "in_progress", "{list}");

    // Its turn ends without the model ever calling todo_write: back to pending, not done.
    request_gate.store(true, Ordering::SeqCst);
    todo_event(&mut events, &id, REQUEST, "pending").await;
    let list = cli(data.path(), "get_todo", json!({"threadId": id}));
    let item = request_item(&list, REQUEST);
    assert_eq!(item["status"], "pending", "{list}");
    assert!(item.get("marked_for_turn").is_none(), "{item}");
    // The turn's model read the request as the one in progress.
    let task = requests
        .lock()
        .unwrap()
        .iter()
        .filter_map(|body| {
            body["messages"].as_array()?.last()?["content"]
                .as_str()
                .map(str::to_owned)
        })
        .find(|text| text.starts_with(REQUEST))
        .expect("the request's turn reached the model");
    assert!(task.contains("[in progress; the person asked]"), "{task}");
    cli(data.path(), "pause_goal", json!({"threadId": id}));
    serving.abort();
}

/// Live: the person asked for a goal in their own words and, before the agent called `set_goal`,
/// sent "Also add a category filter…", which reached the agent first. Requests the person sent
/// after the message that set the goal, in the same run or the queued runs that followed it,
/// join the list as "you asked" once the goal exists; the goal message itself does not.
#[tokio::test(flavor = "multi_thread")]
async fn cli_requests_sent_before_set_goal_in_the_same_chain_join_the_list() {
    const ASK: &str = "Set a goal: build the board.";
    const STEERED: &str = "Also add a category filter to the board.";
    const QUEUED: &str = "And show the due dates in ISO format.";
    for queued in [false, true] {
        let request = if queued { QUEUED } else { STEERED };
        let data = tempfile::tempdir().unwrap();
        let project = tempfile::tempdir().unwrap();
        let provider = MockServer::builder().start().await;
        let gate = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let held = Arc::clone(&gate);
        Mock::given(method("POST"))
            .respond_with(move |request: &Request| {
                let body: Value = request.body_json().unwrap();
                if is_check(&body) {
                    return text_reply(
                        r#"{"verdict":"not_yet","reason":"Keep going.","progressed":true}"#,
                    );
                }
                if last_is_tool(&body) {
                    return text_reply("On it.");
                }
                let last_user = body["messages"]
                    .as_array()
                    .and_then(|messages| messages.iter().rev().find(|m| m["role"] == "user"))
                    .and_then(|message| message["content"].as_str())
                    .unwrap_or_default()
                    .to_owned();
                let set_goal = call("set_goal", json!({"goal": "Build the board"}));
                if last_user.starts_with(ASK) {
                    let waited = std::time::Instant::now();
                    while !held.load(Ordering::SeqCst) && waited.elapsed() < Duration::from_secs(60)
                    {
                        std::thread::sleep(Duration::from_millis(10));
                    }
                    // Steered: the agent sets the goal in this run. Queued: in the next one.
                    return if queued {
                        text_reply("Planning the board.")
                    } else {
                        set_goal
                    };
                }
                if last_user.starts_with(QUEUED) {
                    return set_goal;
                }
                text_reply("Working.")
            })
            .mount(&provider)
            .await;
        let (mut events, serving) = start(&provider, data.path()).await;
        let thread = cli(
            data.path(),
            "create_thread",
            json!({"projectRoot":project.path(),"provider":"go","model":"test-model"}),
        );
        let id = thread["id"].as_str().unwrap().to_owned();
        cli(
            data.path(),
            "send_message",
            json!({"threadId": id, "text": ASK, "reasoningEffort": null}),
        );
        let waiting = cli(
            data.path(),
            "enqueue_message",
            json!({"threadId": id, "text": request}),
        );
        if !queued {
            cli(
                data.path(),
                "steer_queued_message",
                json!({"threadId": id, "messageId": waiting["id"]}),
            );
        }
        // No goal yet, so the request is not on a list yet.
        assert_eq!(
            cli(data.path(), "get_goal", json!({"threadId": id})),
            Value::Null
        );
        gate.store(true, Ordering::SeqCst);

        // The run the request reached sets the goal: the request is on the list, in progress.
        todo_event(&mut events, &id, request, "in_progress").await;
        let goal = cli(data.path(), "get_goal", json!({"threadId": id}));
        assert_eq!(goal["text"], "Build the board", "{goal}");
        let list = cli(data.path(), "get_todo", json!({"threadId": id}));
        assert_eq!(request_item(&list, request)["source"], "person", "{list}");
        // The goal-setting message is the goal, not a request; nothing is added twice.
        let items = list["items"].as_array().unwrap();
        assert!(items.iter().all(|item| item["details"] != ASK), "{list}");
        assert_eq!(
            items
                .iter()
                .filter(|item| item["details"] == request)
                .count(),
            1,
            "{list}"
        );
        cli(data.path(), "pause_goal", json!({"threadId": id}));
        serving.abort();
    }
}
