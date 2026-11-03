//! The main agent's to-do list through the shared server and the `themis` CLI, with a scripted
//! model: `todo_write` keeps it, it survives a restart and a context checkpoint and is read back
//! to the model, and in a goal the check referees it (reopens an item, sees a request the person
//! sent mid-goal in their words) and the goal ends only when the check agrees and nothing is open.

use std::path::Path;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use serde_json::{json, Value};
use themis_desktop::server::{Client, Server};
use themis_desktop::state::AppState;
use wiremock::{matchers::method, Mock, MockServer, Request, ResponseTemplate};

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
