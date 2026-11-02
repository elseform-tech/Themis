//! Memory across chats through the shared server and the `themis` CLI, with a scripted model:
//! a memory saved in one chat is in the next chat's system message (main agent), update and
//! delete reach the index, a project fact can't be saved globally, a memory built from a web
//! page keeps its source, secrets are stripped, and with memory off there is no index and the
//! tool refuses. Settings › Memory's list and delete work through the CLI with revisions.

use std::path::Path;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use serde_json::{json, Value};
use themis_desktop::server::{Client, Server};
use themis_desktop::state::AppState;
use wiremock::{matchers::method, Mock, MockServer, Request, ResponseTemplate};

const NOTE: &str = "Memory from earlier chats";
const SUBAGENT: &str = "You are a subagent";
const SECRET: &str = "ghp_abcdefghijklmnopqrstuvwxyz0123456789AB";

fn cli(dir: &Path, command: &str, args: Value) -> Result<Value, String> {
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
    if !output.status.success() {
        return Err(String::from_utf8_lossy(&output.stderr).into_owned());
    }
    Ok(serde_json::from_slice(&output.stdout).unwrap_or(Value::Null))
}

fn text_reply(text: &str) -> ResponseTemplate {
    ResponseTemplate::new(200).set_body_json(
        json!({"choices":[{"message":{"role":"assistant","content":text},"finish_reason":"stop"}]}),
    )
}

fn call(name: &str, arguments: Value) -> ResponseTemplate {
    ResponseTemplate::new(200).set_body_json(json!({"choices":[{"message":{"role":"assistant","content":null,"tool_calls":[{"id":"call_1","type":"function","function":{"name":name,"arguments":arguments.to_string()}}]},"finish_reason":"tool_calls"}]}))
}

fn system(body: &Value) -> String {
    body["messages"][0]["content"]
        .as_str()
        .unwrap_or_default()
        .to_owned()
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

/// The tool call each request message makes.
fn scripted(request: &str) -> Option<Value> {
    let feedback = json!({"action":"save","scope":"global","name":"free-model-for-tests","type":"feedback",
        "description":"Use the free model for test runs","body":"Use the free model for test runs.",
        "why":"said so in chat","how_to_apply":"pick it for any test run"});
    Some(match request {
        _ if request.starts_with("remember-feedback") => feedback,
        _ if request.starts_with("remember-page") => {
            json!({"action":"save","name":"burndown-layout","type":"reference",
            "description":"Burndown chart layout from a blog post",
            "body":format!("X axis days, Y axis open tasks. The post's token was {SECRET} and password=hunter2."),
            "source":"web page https://example.com/burndown"})
        }
        _ if request.starts_with("global-project") => {
            json!({"action":"save","scope":"global","name":"repo-layout","type":"project",
            "description":"Where the code lives","body":"crates/ holds the code."})
        }
        _ if request.starts_with("update-it") => {
            json!({"action":"update","scope":"global","name":"free-model-for-tests","type":"feedback",
            "description":"Use the free model for every test and eval run","body":"Use the free model for tests and evals."})
        }
        _ if request.starts_with("read-it") => {
            json!({"action":"read","scope":"global","name":"free-model-for-tests"})
        }
        _ if request.starts_with("forget-it") => {
            json!({"action":"delete","scope":"global","name":"free-model-for-tests"})
        }
        _ if request.starts_with("try-off") => {
            json!({"action":"save","name":"while-off","type":"user",
            "description":"Should not be saved","body":"nothing"})
        }
        _ => return None,
    })
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
    async fn start() -> Self {
        let data = tempfile::tempdir().unwrap();
        let project = tempfile::tempdir().unwrap();
        let provider = MockServer::builder().start().await;
        let requests = Arc::new(Mutex::new(Vec::<Value>::new()));
        let seen = Arc::clone(&requests);
        Mock::given(method("POST"))
            .respond_with(move |request: &Request| {
                let body: Value = request.body_json().unwrap();
                seen.lock().unwrap().push(body.clone());
                if system(&body).starts_with(SUBAGENT) {
                    return text_reply("Sub done.");
                }
                if last_user(&body).starts_with("spawn-one") && last(&body)["role"] == "user" {
                    return call(
                        "spawn_subagents",
                        json!({"say":"Starting a helper.","subagents":[{"name":"Helper","brief":"Look around."}]}),
                    );
                }
                if last(&body)["role"] == "tool" {
                    return text_reply("Done.");
                }
                match scripted(&last_user(&body)) {
                    Some(arguments) => call("memory", arguments),
                    None => text_reply("Hello."),
                }
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
        cli(self.data.path(), command, args).unwrap_or_else(|error| panic!("{command}: {error}"))
    }

    fn new_chat(&self) -> String {
        let thread = self.cli(
            "create_thread",
            json!({"projectRoot":self.project.path(),"provider":"go","model":"test-model"}),
        );
        thread["id"].as_str().unwrap().to_owned()
    }

    /// Sends one message and waits for its run to end; the memory events it saw.
    async fn send(&mut self, id: &str, text: &str) -> Vec<Value> {
        let before = self.requests.lock().unwrap().len();
        self.cli(
            "send_message",
            json!({"threadId":id,"text":text,"reasoningEffort":null}),
        );
        let memory_events = tokio::time::timeout(Duration::from_secs(120), async {
            let mut found = Vec::new();
            loop {
                let item = Client::next_event(&mut self.events).await.unwrap();
                if item["name"] != "thread-event" || item["payload"]["thread_id"] != id {
                    continue;
                }
                let event = &item["payload"]["event"];
                let kind = event["kind"].as_str().unwrap_or_default();
                if kind == "memory_changed" {
                    found.push(event.clone());
                }
                if ["finished", "failed", "incomplete"].contains(&kind) {
                    assert_eq!(kind, "finished", "{event}");
                    return found;
                }
            }
        })
        .await
        .expect("the run ended");
        assert!(self.requests.lock().unwrap().len() > before);
        memory_events
    }

    /// The first request of the latest run.
    fn first_request_of(&self, text: &str) -> Value {
        self.requests
            .lock()
            .unwrap()
            .iter()
            .rev()
            .find(|body| last_user(body).starts_with(text) && last(body)["role"] == "user")
            .cloned()
            .expect("the run's first request")
    }

    /// The tool result the model got back in the latest run.
    fn tool_result(&self) -> String {
        let requests = self.requests.lock().unwrap();
        let body = requests.last().unwrap();
        assert_eq!(last(body)["role"], "tool", "{}", last(body));
        last(body)["content"]
            .as_str()
            .unwrap_or_default()
            .to_owned()
    }

    fn memory_dir(&self) -> std::path::PathBuf {
        self.data.path().join("memory")
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn memories_saved_in_one_chat_reach_the_next_and_follow_updates_deletes_and_the_switch() {
    let mut h = Harness::start().await;
    let first = h.new_chat();

    // A first chat starts with no memory note, but with the memory tool and its description.
    h.send(&first, "hello").await;
    let request = h.first_request_of("hello");
    assert!(!system(&request).contains(NOTE));
    let tool = request["tools"]
        .as_array()
        .unwrap()
        .iter()
        .find(|tool| tool["function"]["name"] == "memory")
        .expect("the memory tool is offered")
        .clone();
    assert_eq!(
        tool["function"]["description"],
        themis_core::prompts::MEMORY_TOOL,
        "{tool}"
    );

    // Saved globally, with Why and How to apply; the chat shows it.
    let events = h.send(&first, "remember-feedback").await;
    assert_eq!(
        events.len(),
        1,
        "{:?}",
        h.requests.lock().unwrap().last().map(last)
    );
    assert_eq!(events[0]["action"], "saved");
    assert_eq!(events[0]["scope"], "global");
    assert_eq!(events[0]["name"], "free-model-for-tests");
    let file =
        std::fs::read_to_string(h.memory_dir().join("global/free-model-for-tests.md")).unwrap();
    assert!(file.starts_with("---\nname: free-model-for-tests\ndescription: Use the free model for test runs\ntype: feedback\nsaved: "), "{file}");
    assert!(
        file.contains("\nWhy: said so in chat\nHow to apply: pick it for any test run\n"),
        "{file}"
    );

    // Built from a web page: project-scoped, marked with its source, secrets stripped.
    let events = h.send(&first, "remember-page").await;
    assert_eq!(events[0]["scope"], "project");
    let project_dir = std::fs::read_dir(h.memory_dir().join("projects"))
        .unwrap()
        .next()
        .unwrap()
        .unwrap()
        .path();
    let page = std::fs::read_to_string(project_dir.join("burndown-layout.md")).unwrap();
    assert!(
        page.contains("\nsource: web page https://example.com/burndown\n"),
        "{page}"
    );
    assert!(
        !page.contains(SECRET) && !page.contains("hunter2"),
        "{page}"
    );
    assert!(page.contains("X axis days"), "{page}");
    // Nothing went into the project.
    assert!(!h.project.path().join(".themis/memory").exists());

    // A project fact may not be saved everywhere.
    let events = h.send(&first, "global-project").await;
    assert!(events.is_empty());
    assert!(
        h.tool_result().contains("belongs to this project"),
        "{}",
        h.tool_result()
    );

    // The next chat's system message carries both indexes.
    let second = h.new_chat();
    h.send(&second, "hello again").await;
    let note = system(&h.first_request_of("hello again"));
    // The whole note, as the prompt file says it, around the two indexes.
    let preamble = themis_core::prompts::SYSTEM_MEMORY
        .split("{{global}}")
        .next()
        .unwrap();
    assert!(note.contains(preamble), "{note}");
    assert!(
        note.contains("Everywhere:\n- free-model-for-tests (feedback, "),
        "{note}"
    );
    assert!(
        note.contains("): Use the free model for test runs"),
        "{note}"
    );
    assert!(
        note.contains("This project:\n- burndown-layout (reference, "),
        "{note}"
    );
    assert!(
        note.contains(", from web page https://example.com/burndown): Burndown chart layout"),
        "{note}"
    );
    // The full memory stays out of the prompt until read.
    assert!(!note.contains("pick it for any test run"), "{note}");
    h.send(&second, "read-it").await;
    assert!(
        h.tool_result().contains("pick it for any test run"),
        "{}",
        h.tool_result()
    );

    // A subagent starts with the same index and only reads memories.
    h.send(&second, "spawn-one").await;
    let child = tokio::time::timeout(Duration::from_secs(60), async {
        loop {
            let found = h
                .requests
                .lock()
                .unwrap()
                .iter()
                .find(|body| system(body).starts_with(SUBAGENT))
                .cloned();
            if let Some(found) = found {
                return found;
            }
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
    })
    .await
    .expect("the subagent asked its model");
    let child_system = system(&child);
    assert!(child_system.contains(preamble), "{child_system}");
    assert!(
        child_system.contains("- free-model-for-tests (feedback, "),
        "{child_system}"
    );
    let child_tool = child["tools"]
        .as_array()
        .unwrap()
        .iter()
        .find(|tool| tool["function"]["name"] == "memory")
        .expect("the subagent can read memories")
        .clone();
    assert_eq!(
        child_tool["function"]["description"],
        themis_core::prompts::MEMORY_TOOL_READ
    );

    // A chat in another project sees the global memory but not this project's.
    let other = tempfile::tempdir().unwrap();
    let elsewhere = h.cli(
        "create_thread",
        json!({"projectRoot":other.path(),"provider":"go","model":"test-model"}),
    )["id"]
        .as_str()
        .unwrap()
        .to_owned();
    h.send(&elsewhere, "hello elsewhere").await;
    let note = system(&h.first_request_of("hello elsewhere"));
    assert!(note.contains("free-model-for-tests"), "{note}");
    assert!(!note.contains("burndown-layout"), "{note}");

    // Update and delete reach the index.
    let events = h.send(&second, "update-it").await;
    assert_eq!(events[0]["action"], "updated");
    h.send(&second, "after update").await;
    let note = system(&h.first_request_of("after update"));
    assert!(
        note.contains("Use the free model for every test and eval run"),
        "{note}"
    );
    let events = h.send(&second, "forget-it").await;
    assert_eq!(events[0]["action"], "forgot");
    h.send(&second, "after delete").await;
    let note = system(&h.first_request_of("after delete"));
    assert!(!note.contains("free-model-for-tests"), "{note}");
    assert!(note.contains("Everywhere:\n(none)"), "{note}");

    // The memory events are saved with the chat.
    let history = h.cli("get_thread_history", json!({"threadId": second}));
    let saved: Vec<&str> = history
        .as_array()
        .unwrap()
        .iter()
        .filter(|item| item["envelope"]["event"]["kind"] == "memory_changed")
        .map(|item| item["envelope"]["event"]["action"].as_str().unwrap())
        .collect();
    assert_eq!(saved, ["updated", "forgot"], "{history}");

    // Settings › Memory: list and delete, refused on a stale revision.
    let root = h.project.path().to_string_lossy().into_owned();
    let listed = h.cli(
        "list_memories",
        json!({"scope":"project","projectRoot":root}),
    );
    assert_eq!(listed["memories"][0]["name"], "burndown-layout", "{listed}");
    assert_eq!(
        listed["memories"][0]["source"],
        "web page https://example.com/burndown"
    );
    assert_eq!(listed["index_lines"], 1);
    assert_eq!(listed["max_lines"], 200);
    assert_eq!(listed["enabled"], true);
    assert!(cli(
        h.data.path(),
        "delete_memory",
        json!({"scope":"project","projectRoot":root,"name":"burndown-layout","revision":"stale"}),
    )
    .is_err());

    // Memory off: no index, the tool refuses, memories are kept.
    let settings = h.cli("get_memory_settings", json!({}));
    assert_eq!(settings["settings"]["enabled"], true);
    let off = h.cli(
        "update_memory_settings",
        json!({"enabled": false, "revision": settings["revision"]}),
    );
    assert_eq!(off["settings"]["enabled"], false);
    let config = std::fs::read_to_string(h.data.path().join("runtime.jsonc")).unwrap();
    assert!(config.contains("\"memory\""), "{config}");
    h.send(&second, "try-off").await;
    let request = h.first_request_of("try-off");
    assert!(!system(&request).contains(NOTE));
    // The tool isn't offered, so nothing can be saved or read.
    assert!(
        !request["tools"]
            .as_array()
            .unwrap()
            .iter()
            .any(|tool| tool["function"]["name"] == "memory"),
        "memory tool offered while off"
    );
    assert!(!project_dir.join("while-off.md").exists());
    assert!(project_dir.join("burndown-layout.md").exists());

    let deleted = h.cli(
        "delete_memory",
        json!({"scope":"project","projectRoot":root,"name":"burndown-layout","revision":listed["revision"]}),
    );
    assert!(deleted["revision"].is_string());
    let listed = h.cli(
        "list_memories",
        json!({"scope":"project","projectRoot":root}),
    );
    assert_eq!(listed["memories"].as_array().unwrap().len(), 0);
    assert_eq!(listed["enabled"], false);

    // Diagnostics name scope, type and counts, never a memory's text.
    let logs = std::fs::read_dir(h.data.path().join("logs"))
        .map(|entries| {
            entries
                .flatten()
                .map(|entry| std::fs::read_to_string(entry.path()).unwrap_or_default())
                .collect::<String>()
        })
        .unwrap_or_default();
    assert!(logs.contains("\"memory\""), "no memory diagnostics");
    for private in ["free model", "Burndown", "X axis", "hunter2", SECRET] {
        assert!(!logs.contains(private), "diagnostics hold {private:?}");
    }
}
