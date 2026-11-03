//! The to-do list experiment switch, through the shared server and the `themis` CLI with a
//! scripted model (no real model is called): with `goal.seed_todo`, a new goal's list is seeded
//! before its first turn, which reads it, and the goal is reached once the items are done.
//! (Automatic memory is covered by `memory_cli_e2e.rs`.)

use std::path::Path;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use serde_json::{json, Value};
use themis_desktop::server::{Client, Server};
use themis_desktop::state::AppState;
use wiremock::{matchers::method, Mock, MockServer, Request, ResponseTemplate};

const SEED: &str = "You turn a goal into a short to-do list";
const CHECK: &str = "You decide whether an agent has reached a goal";
const SUBAGENT: &str = "You are a subagent";
const SUMMARY: &str = "You summarize agent context";

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
    ResponseTemplate::new(200).set_body_json(json!({
        "choices":[{"message":{"role":"assistant","content":text},"finish_reason":"stop"}],
        "usage":{"prompt_tokens":321,"completion_tokens":12,"total_tokens":333}
    }))
}

fn call(name: &str, arguments: Value) -> ResponseTemplate {
    ResponseTemplate::new(200).set_body_json(json!({"choices":[{"message":{"role":"assistant","content":null,"tool_calls":[{"id":"call_1","type":"function","function":{"name":name,"arguments":arguments.to_string()}}]},"finish_reason":"tool_calls"}]}))
}

fn system(body: &Value) -> &str {
    body["messages"][0]["content"].as_str().unwrap_or_default()
}

fn last(body: &Value) -> Value {
    body["messages"]
        .as_array()
        .and_then(|messages| messages.last().cloned())
        .unwrap_or(Value::Null)
}

fn last_user(body: &Value) -> String {
    body["messages"]
        .as_array()
        .and_then(|messages| messages.iter().rev().find(|m| m["role"] == "user"))
        .and_then(|m| m["content"].as_str())
        .unwrap_or_default()
        .to_owned()
}

/// What the main agent does with each message.
fn main_answer(body: &Value) -> ResponseTemplate {
    if last(body)["role"] == "tool" {
        return text_reply("Done.");
    }
    let user = last_user(body);
    let wire = body["messages"].to_string();
    match () {
        _ if user.starts_with("spawn-one") => call(
            "spawn_subagents",
            json!({"say":"Starting a helper.","subagents":[{"name":"Helper","brief":"Look around."}]}),
        ),
        // A rewrite that puts back an item the check completed, as a stale agent would.
        _ if user.starts_with("redo-list") => call(
            "todo_write",
            json!({"todos":[
                {"id":"t1","title":"Tasks can be added and ticked off","status":"completed","outcome":"added and ticked three"},
                {"id":"t2","title":"CSV export downloads every task","status":"completed","outcome":"downloaded 3 rows"},
                {"id":"t3","title":"Verify end to end","status":"pending"}]}),
        ),
        _ if user.starts_with("plan-it") => call(
            "todo_write",
            json!({"todos":[{"title":"Write the parser","status":"in_progress"},{"title":"Write its tests","status":"pending"}]}),
        ),
        _ if user.starts_with("write-file") => call(
            "write_file",
            json!({"file_path": format!("{}.txt", user.lines().next().unwrap_or_default().replace(' ', "-")), "content":"x", "append": false}),
        ),
        // A goal turn finishes the features on its list (read at the turn's start), and leaves
        // the last item for the check.
        _ if wire.contains("[pending; from the goal] Tasks can be added") => call(
            "todo_write",
            json!({"todos":[
                {"id":"t1","title":"Tasks can be added and ticked off","status":"completed","outcome":"added and ticked three"},
                {"id":"t2","title":"CSV export downloads every task","status":"completed","outcome":"downloaded 3 rows"},
                {"id":"t3","title":"Verify end to end","status":"pending"}]}),
        ),
        _ if user.starts_with("filler") => text_reply("Noted."),
        _ => text_reply("Fixed the build."),
    }
}

struct Harness {
    data: tempfile::TempDir,
    project: tempfile::TempDir,
    events: tokio::net::TcpStream,
    _serving: tokio::task::JoinHandle<Result<(), String>>,
    _provider: MockServer,
    requests: Arc<Mutex<Vec<Value>>>,
}

impl Harness {
    async fn start(user_config: Option<&str>, project_config: Option<&str>) -> Self {
        let data = tempfile::tempdir().unwrap();
        let project = tempfile::tempdir().unwrap();
        // A repository, so the end of a turn can tell which files it changed.
        let init = std::process::Command::new("git")
            .args(["init", "-q"])
            .current_dir(project.path())
            .status()
            .unwrap();
        assert!(init.success());
        if let Some(config) = user_config {
            std::fs::write(data.path().join("runtime.jsonc"), config).unwrap();
        }
        if let Some(config) = project_config {
            std::fs::create_dir_all(project.path().join(".themis")).unwrap();
            std::fs::write(project.path().join(".themis/config.jsonc"), config).unwrap();
        }
        let provider = MockServer::builder().start().await;
        let requests = Arc::new(Mutex::new(Vec::<Value>::new()));
        let seen = Arc::clone(&requests);
        Mock::given(method("POST"))
            .respond_with(move |request: &Request| {
                let body: Value = request.body_json().unwrap();
                seen.lock().unwrap().push(body.clone());
                let head = system(&body);
                if head.starts_with(SEED) {
                    return text_reply(r#"{"items":[{"title":"Tasks can be added and ticked off"},{"title":"CSV export downloads every task","details":"every row"}]}"#);
                }
                if head.contains(CHECK) {
                    // The check completes the seeded verification item on the turn's evidence.
                    return text_reply(
                        r#"{"verdict":"reached","reason":"Everything works.","progressed":true,"complete":[{"id":"t3","outcome":"clicked through every feature"}]}"#,
                    );
                }
                if head.contains(SUMMARY) {
                    return text_reply("Goal/constraints: keep notes. Unresolved work: none. Parked work: none. Next action: carry on.");
                }
                if head.starts_with(SUBAGENT) {
                    return text_reply("Sub done.");
                }
                main_answer(&body)
            })
            .mount(&provider)
            .await;
        let state = AppState::new_for_test(data.path().join("settings.json"));
        state
            .set_secret("go".into(), "synthetic-test-key".into())
            .await
            .unwrap();
        state.set_go_base_url_override(Some(provider.uri()));
        let server = Server::bind(state, data.path()).await.unwrap();
        let serving = tokio::spawn(server.run());
        let events = Client::new(data.path().into()).subscribe().await.unwrap();
        Self {
            data,
            project,
            events,
            _serving: serving,
            _provider: provider,
            requests,
        }
    }

    fn cli(&self, command: &str, args: Value) -> Value {
        cli(self.data.path(), command, args)
    }

    fn new_chat(&self) -> String {
        let thread = self.cli(
            "create_thread",
            json!({"projectRoot":self.project.path(),"provider":"go","model":"test-model"}),
        );
        let id = thread["id"].as_str().unwrap().to_owned();
        self.cli(
            "set_thread_approval_mode",
            json!({"threadId": id, "mode": "yolo"}),
        );
        id
    }

    /// Sends one message and waits for its run to end, then `settle` longer for what follows
    /// it; the memory lines it saw.
    async fn send(&mut self, id: &str, text: &str, settle: Duration) -> Vec<Value> {
        self.cli(
            "send_message",
            json!({"threadId":id,"text":text,"reasoningEffort":null}),
        );
        let mut found = Vec::new();
        let mut finished = false;
        let deadline = tokio::time::Instant::now() + Duration::from_secs(120);
        loop {
            let wait = if finished {
                settle
            } else {
                deadline - tokio::time::Instant::now()
            };
            let Ok(item) = tokio::time::timeout(wait, Client::next_event(&mut self.events)).await
            else {
                assert!(finished, "the run did not end");
                return found;
            };
            let item = item.unwrap();
            if item["name"] != "thread-event" || item["payload"]["thread_id"] != id {
                continue;
            }
            let event = &item["payload"]["event"];
            let kind = event["kind"].as_str().unwrap_or_default();
            if kind == "memory_changed" {
                found.push(event.clone());
            }
            if ["finished", "failed", "incomplete"].contains(&kind) && !finished {
                assert_eq!(kind, "finished", "{event}");
                finished = true;
            }
        }
    }

    /// Requests of one kind, by their system message's start.
    fn requests_of(&self, head: &str) -> Vec<Value> {
        self.requests
            .lock()
            .unwrap()
            .iter()
            .filter(|body| system(body).starts_with(head))
            .cloned()
            .collect()
    }

    /// The first request of the run that `text` started.
    fn first_request_of(&self, text: &str) -> Value {
        self.requests
            .lock()
            .unwrap()
            .iter()
            .rev()
            .find(|body| {
                !system(body).starts_with(SUBAGENT)
                    && last_user(body).starts_with(text)
                    && last(body)["role"] == "user"
            })
            .cloned()
            .unwrap_or_else(|| panic!("no request for {text}"))
    }

    /// Every diagnostics line of `area`/`event`, and all the log text.
    fn logs(&self, area: &str, event: &str) -> (Vec<Value>, String) {
        let mut text = String::new();
        if let Ok(entries) = std::fs::read_dir(self.data.path().join("logs")) {
            for entry in entries.flatten() {
                text.push_str(&std::fs::read_to_string(entry.path()).unwrap_or_default());
            }
        }
        let lines = text
            .lines()
            .filter_map(|line| serde_json::from_str::<Value>(line).ok())
            .filter(|line| line["service"] == area && line["event"] == event)
            .collect();
        (lines, text)
    }

    /// Waits until there are `count` diagnostics lines of `area`/`event`.
    async fn logged(&self, area: &str, event: &str, count: usize) -> Vec<Value> {
        let deadline = std::time::Instant::now() + Duration::from_secs(30);
        loop {
            let (lines, _) = self.logs(area, event);
            if lines.len() >= count || std::time::Instant::now() > deadline {
                assert!(lines.len() >= count, "{area}/{event}: {lines:?}");
                return lines;
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
    }
}

fn fields(line: &Value) -> &Value {
    &line["details"]
}

#[tokio::test(flavor = "multi_thread")]
async fn a_seeded_goal_reads_its_list_and_is_reached_once_its_items_are_done() {
    let mut h = Harness::start(Some(r#"{"goal":{"seed_todo":true}}"#), None).await;
    let id = h.new_chat();
    h.send(
        &id,
        "/goal Build a todo app with CSV export",
        Duration::from_millis(500),
    )
    .await;
    // One seeding request: the empty project counts as new, so set-up may come first.
    let seeds = h.requests_of(SEED);
    assert_eq!(seeds.len(), 1);
    let user = seeds[0]["messages"][1]["content"].as_str().unwrap();
    assert!(user.contains("a new, empty project"), "{user}");
    assert!(user.contains("Build a todo app with CSV export"), "{user}");
    // The goal's first turn opened with the seeded list.
    let first = h.first_request_of("/goal Build a todo app");
    let opening = last_user(&first);
    assert!(
        opening.contains("[pending; from the goal] Tasks can be added and ticked off"),
        "{opening}"
    );
    assert!(opening.contains("Verify end to end"), "{opening}");
    // The agent finishes the items, the check accepts them, and the goal is reached.
    let deadline = std::time::Instant::now() + Duration::from_secs(60);
    loop {
        let goal = h.cli("get_goal", json!({"threadId": id}));
        if goal["status"] == "reached" {
            break;
        }
        assert!(std::time::Instant::now() < deadline, "{goal}");
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    let list = h.cli("get_todo", json!({"threadId": id}));
    let items = list["items"].as_array().unwrap();
    assert_eq!(items.len(), 3, "{list}");
    assert!(items.iter().all(|item| item["source"] == "goal"), "{list}");
    assert!(
        items.iter().all(|item| item["status"] == "completed"),
        "{list}"
    );
    // The agent ticked the features; the check completed "Verify end to end" itself.
    assert_eq!(items[2]["title"], "Verify end to end");
    assert!(!items[2]["closed"].is_null(), "{list}");
    assert!(items[0]["closed"].is_null(), "{list}");
    // An agent's later rewrite can't put it back: the check's status stands and the agent is
    // told why.
    h.send(&id, "redo-list", Duration::from_millis(300)).await;
    let list = h.cli("get_todo", json!({"threadId": id}));
    assert_eq!(list["items"][2]["status"], "completed", "{list}");
    assert!(!list["items"][2]["closed"].is_null(), "{list}");
    let result = h
        .requests
        .lock()
        .unwrap()
        .iter()
        .rev()
        .find(|body| last(body)["role"] == "tool")
        .map(|body| last(body)["content"].to_string())
        .unwrap_or_default();
    assert!(
        result.contains("t3 was confirmed by the check; only the check can reopen it."),
        "{result}"
    );
    let seeded = h.logged("todo", "seed", 1).await;
    assert_eq!(fields(&seeded[0])["outcome"], "seeded");
    assert_eq!(fields(&seeded[0])["items"], 3);

    // Seeding is off by default: another chat's goal starts with an empty list.
    let mut off = Harness::start(None, None).await;
    let other = off.new_chat();
    off.send(&other, "/goal Build it", Duration::from_millis(300))
        .await;
    assert!(off.requests_of(SEED).is_empty());
}
