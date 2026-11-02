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
