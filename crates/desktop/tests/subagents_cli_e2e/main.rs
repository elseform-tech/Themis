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

mod plugin_profiles;
mod runs;

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

/// The tools a list or a profile gives. `Skill` depends on the skills this machine's user has.
fn sorted_tools(body: &Value) -> Vec<String> {
    let mut names = tool_names(body);
    names.retain(|name| name != "Skill");
    names.sort();
    names
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
