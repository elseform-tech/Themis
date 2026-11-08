//! Claude plugin hooks imported through the real server and the `themis` CLI, then run by a chat
//! with a scripted model: a guard sees Claude's field names and still blocks, `if` and MCP
//! matchers pick the right calls, `async` hooks don't hold the turn and `StopFailure` fires only
//! on a failed run. Unix only: the hooks are shell scripts.
#![cfg(unix)]

use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use serde_json::{json, Value};
use themis_desktop::{server::Server, state::AppState, types::ProviderKind};
use wiremock::matchers::method;
use wiremock::{Mock, MockServer, Request, ResponseTemplate};

mod guards_and_stops;
mod sessions_and_prompts;

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

fn themis(data: &Path, args: &[&str]) -> Value {
    let output = std::process::Command::new(env!("CARGO_BIN_EXE_themis"))
        .arg("--data-dir")
        .arg(data)
        .args(args)
        .env_remove("OPENCODE_KEY")
        .output()
        .expect("CLI");
    assert!(
        output.status.success(),
        "{args:?}: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    serde_json::from_slice(&output.stdout).unwrap_or(Value::Null)
}

fn until(what: &str, mut done: impl FnMut() -> bool) {
    let deadline = Instant::now() + Duration::from_secs(60);
    while !done() {
        assert!(Instant::now() < deadline, "{what}");
        std::thread::sleep(Duration::from_millis(50));
    }
}

fn reply(text: &str) -> ResponseTemplate {
    ResponseTemplate::new(200).set_body_json(json!({
        "choices": [{"message": {"role": "assistant", "content": text}, "finish_reason": "stop"}]
    }))
}

fn tool_call(id: &str, name: &str, args: &Value) -> ResponseTemplate {
    ResponseTemplate::new(200).set_body_json(json!({
        "choices": [{"message": {"role": "assistant", "content": null, "tool_calls": [
            {"id": id, "type": "function", "function": {"name": name, "arguments": args.to_string()}}
        ]}, "finish_reason": "tool_calls"}]
    }))
}

fn tool_results(body: &Value) -> Vec<String> {
    body["messages"]
        .as_array()
        .map(|messages| {
            messages
                .iter()
                .filter(|message| message["role"] == "tool")
                .map(|message| message["content"].as_str().unwrap_or_default().to_owned())
                .collect()
        })
        .unwrap_or_default()
}

struct App {
    llm: MockServer,
    data: tempfile::TempDir,
    project: tempfile::TempDir,
    _plugins: tempfile::TempDir,
    thread: String,
    state: AppState,
    serving: tokio::task::JoinHandle<()>,
}

/// A chat whose model answers by `respond`, with a Claude plugin holding `hooks` and `files`
/// imported through the CLI and every hook enabled.
async fn app(
    hooks: Value,
    files: &[(&str, &str)],
    respond: impl Fn(&Value) -> ResponseTemplate + Send + Sync + 'static,
) -> App {
    app_with_folder(hooks, files, |_| {}, respond).await
}

/// The same, with `prepare` adding to the plugin's folder before it is imported.
async fn app_with_folder(
    hooks: Value,
    files: &[(&str, &str)],
    prepare: impl FnOnce(&Path),
    respond: impl Fn(&Value) -> ResponseTemplate + Send + Sync + 'static,
) -> App {
    let llm = MockServer::builder().start().await;
    Mock::given(method("POST"))
        .respond_with(move |request: &Request| respond(&request.body_json().unwrap_or(Value::Null)))
        .mount(&llm)
        .await;
    let data = tempfile::tempdir().unwrap();
    let project = tempfile::tempdir().unwrap();
    std::process::Command::new("git")
        .args(["init", "-q"])
        .current_dir(project.path())
        .status()
        .expect("git");
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
    let server = Server::bind(state.clone(), data.path()).await.unwrap();
    let serving = tokio::spawn(async move {
        let _ = server.run().await;
    });
    let plugins = tempfile::tempdir().unwrap();
    let folder = plugin_folder(plugins.path(), &hooks, files);
    prepare(&folder);
    let directory = data.path().to_owned();
    tokio::task::spawn_blocking(move || {
        let plugin = themis(
            &directory,
            &[
                "plugin",
                "import",
                folder.to_str().unwrap(),
                "--allow-partial",
            ],
        );
        for hook in plugin["spec"]["hooks"].as_array().unwrap() {
            themis(
                &directory,
                &["hook", "enable", "guarded", hook["name"].as_str().unwrap()],
            );
        }
        for server in plugin["spec"]["mcp"].as_object().into_iter().flatten() {
            themis(&directory, &["mcp", "enable", "guarded", server.0]);
        }
    })
    .await
    .unwrap();
    App {
        llm,
        data,
        project,
        _plugins: plugins,
        thread,
        state,
        serving,
    }
}

fn plugin_folder(parent: &Path, hooks: &Value, files: &[(&str, &str)]) -> PathBuf {
    let root = parent.join("guarded");
    std::fs::create_dir_all(root.join(".claude-plugin")).unwrap();
    std::fs::create_dir_all(root.join("hooks")).unwrap();
    std::fs::write(
        root.join(".claude-plugin/plugin.json"),
        json!({"name": "guarded", "description": "A test plugin"}).to_string(),
    )
    .unwrap();
    std::fs::write(
        root.join("hooks/hooks.json"),
        json!({ "hooks": hooks }).to_string(),
    )
    .unwrap();
    for (path, content) in files {
        let path = root.join(path);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, content).unwrap();
    }
    root
}

/// Sends `task` in YOLO and waits until the run is over, allowing once any destructive step
/// it asks about first.
async fn run(app: &App, task: &'static str) {
    let (data, thread) = (app.data.path().to_owned(), app.thread.clone());
    tokio::task::spawn_blocking(move || {
        call(
            &data,
            "set_thread_approval_mode",
            json!({"threadId": thread, "mode": "yolo"}),
        );
        call(
            &data,
            "send_message",
            json!({"threadId": thread, "text": task}),
        );
        until("the run never finished", || {
            // A destructive step asks the person first even in YOLO: answer as one would.
            if let Some(waiting) = call(&data, "get_pending_approvals", json!({})).as_array() {
                for approval in waiting.iter().filter(|a| a["thread_id"] == thread.as_str()) {
                    call(
                        &data,
                        "approve_action",
                        json!({"threadId": thread, "approvalId": approval["approval_id"],
                            "decision": "once"}),
                    );
                }
            }
            call(&data, "get_thread", json!({"threadId": thread}))["running"] != true
        });
    })
    .await
    .unwrap();
}

async fn until_async<F: std::future::Future<Output = bool>>(what: &str, done: impl Fn() -> F) {
    let deadline = Instant::now() + Duration::from_secs(30);
    while !done().await {
        assert!(Instant::now() < deadline, "{what}");
        tokio::time::sleep(Duration::from_millis(30)).await;
    }
}

async fn bodies(llm: &MockServer) -> Vec<Value> {
    llm.received_requests()
        .await
        .unwrap()
        .iter()
        .map(|request| request.body_json().unwrap_or(Value::Null))
        .collect()
}

/// What the tool answered in the last request the model got.
async fn last_tool_result(app: &App) -> String {
    bodies(&app.llm)
        .await
        .iter()
        .flat_map(tool_results)
        .next_back()
        .unwrap_or_default()
}

/// Calls `tool` once with `args`, then ends the turn.
fn calls_once(
    tool: &'static str,
    args: Value,
) -> impl Fn(&Value) -> ResponseTemplate + Send + Sync + 'static {
    move |body| {
        if tool_results(body).is_empty() {
            tool_call("c1", tool, &args)
        } else {
            reply("Done.")
        }
    }
}

fn exists(app: &App, name: &str) -> bool {
    app.project.path().join(name).exists()
}

/// A script, written the way catalog plugins write theirs: it reads Claude's field names from
/// its input and refuses with exit code 2 and a reason on stderr.
const GUARD: &str = r#"#!/bin/sh
input=$(cat)
printf '%s' "$input" > seen.json
case "$input" in
  *'"tool_name":"Bash"'*'git push'*) echo 'pushes are frozen' >&2; exit 2 ;;
  *'git push'*'"tool_name":"Bash"'*) echo 'pushes are frozen' >&2; exit 2 ;;
esac
exit 0
"#;

/// A `Stop` hook counting rounds: it refuses twice, then lets the turn end.
const LOOPING: &str = r#"#!/bin/sh
input=$(cat)
count=$(cat count 2>/dev/null || echo 0)
printf '%s' "$input" > "stop-$count.json"
echo $((count + 1)) > count
if [ "$count" -lt 2 ]; then
  echo "{\"decision\":\"block\",\"reason\":\"Now do round $((count + 1))\"}"
fi
"#;

/// A loop plugin's whole path, as ralph-loop does it: it reads the agent's last reply from the
/// transcript with `grep` and `jq`, and ends when it finds its promise.
const PROMISE_LOOP: &str = r#"#!/bin/bash
set -euo pipefail
HOOK_INPUT=$(cat)
TRANSCRIPT_PATH=$(echo "$HOOK_INPUT" | jq -r '.transcript_path')
[[ -f "$TRANSCRIPT_PATH" ]] || { echo "no transcript" > failed; exit 0; }
grep -q '"role":"assistant"' "$TRANSCRIPT_PATH" || { echo "no assistant" > failed; exit 0; }
LAST_LINES=$(grep '"role":"assistant"' "$TRANSCRIPT_PATH" | tail -n 100)
LAST_OUTPUT=$(echo "$LAST_LINES" | jq -rs 'map(.message.content[]? | select(.type == "text") | .text) | last // ""')
echo "$LAST_OUTPUT" >> replies.txt
if echo "$LAST_OUTPUT" | grep -q '<promise>DONE</promise>'; then exit 0; fi
jq -n --arg prompt "Work on the task again" '{"decision": "block", "reason": $prompt}'
"#;

const SKILL_FILE: &str = "---\nname: review\ndescription: Review well\n---\nTHE SKILL BODY";

/// The path of the plugin's skill in the catalog in the system message, if it lists it. The
/// catalog's built-in skills come first.
fn catalog_path(body: &Value) -> Option<String> {
    let text = body["messages"][0]["content"].as_str()?;
    let end = text.find("review/SKILL.md")? + "review/SKILL.md".len();
    let at = text[..end].rfind(".themis/skill-catalogs/")?;
    Some(text[at..end].to_owned())
}

/// Reads the skill the catalog lists, twice, then ends.
fn reads_the_skill(body: &Value) -> ResponseTemplate {
    let done = tool_results(body).len();
    match catalog_path(body) {
        Some(path) if done < 2 => tool_call(
            &format!("r{done}"),
            "read_file",
            &json!({"file_path": path}),
        ),
        _ => reply("Done."),
    }
}

/// What a hook appended to `name` in `root`, one JSON payload a line.
fn lines_in(root: &Path, name: &str) -> Vec<Value> {
    std::fs::read_to_string(root.join(name))
        .unwrap_or_default()
        .lines()
        .map(|line| serde_json::from_str(line).unwrap())
        .collect()
}

fn lines(app: &App, name: &str) -> Vec<Value> {
    lines_in(app.project.path(), name)
}

fn logging(file: &str) -> String {
    format!("(cat; echo) >> {file}")
}

fn logs(event: &str, file: &str) -> Value {
    json!({event: [{"hooks": [{"type": "command", "command": logging(file)}]}]})
}

/// The names the plugin's hooks were imported under, for those whose command has `part`.
fn hook_named(app: &App, part: &str) -> String {
    let list = themis(app.data.path(), &["plugin", "show", "guarded"]);
    list["spec"]["hooks"]
        .as_array()
        .unwrap()
        .iter()
        .find(|hook| hook["command"].as_str().unwrap().contains(part))
        .map(|hook| hook["name"].as_str().unwrap().to_owned())
        .unwrap()
}

/// Sends `task` in Custom, with hooks allowed by a rule so that the only question is about the
/// tool, and returns once the run is waiting on one or has ended, saying which.
async fn run_custom_until_asked(app: &App, task: &'static str) -> Option<String> {
    std::fs::write(
        app.data.path().join("runtime.jsonc"),
        r#"{"approval":{"rules":[{"tool":"hook_*","action":"allow"}]}}"#,
    )
    .unwrap();
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
            json!({"threadId": thread, "text": task}),
        );
        let deadline = Instant::now() + Duration::from_secs(30);
        loop {
            assert!(Instant::now() < deadline, "the run neither asked nor ended");
            let pending = call(&data, "get_pending_approvals", json!({}));
            if let Some(approval) = pending.as_array().and_then(|list| list.first()) {
                let id = approval["approval_id"]
                    .as_str()
                    .or_else(|| approval["approvalId"].as_str())
                    .unwrap();
                return Some(id.to_owned());
            }
            if call(&data, "get_thread", json!({"threadId": thread}))["running"] != true {
                return None;
            }
            std::thread::sleep(Duration::from_millis(50));
        }
    })
    .await
    .unwrap()
}

/// Whether a request is a prompt hook's: its own system message, no tools.
fn is_prompt_hook(body: &Value) -> bool {
    body["messages"][0]["content"]
        .as_str()
        .unwrap_or_default()
        .contains("a hook's check")
}

fn prompt_hook(event: &str, matcher: &str, prompt: &str) -> Value {
    json!({event: [{"matcher": matcher, "hooks": [{"type": "prompt", "prompt": prompt}]}]})
}

const WRITES: &str = r#"{"file_path":"new.txt","content":"THE_FILE_TEXT"}"#;

fn writes() -> Value {
    serde_json::from_str(WRITES).unwrap()
}

/// A chat with a plugin's real hooks, from a copy of it: a tool call a turn, for two turns.
async fn real_plugin(folder: PathBuf) -> App {
    let hooks: Value =
        serde_json::from_str(&std::fs::read_to_string(folder.join("hooks/hooks.json")).unwrap())
            .unwrap();
    crate::app_with_folder(
        hooks["hooks"].clone(),
        &[],
        move |root| {
            for entry in std::fs::read_dir(&folder).unwrap() {
                let entry = entry.unwrap();
                if entry.file_name() != ".claude-plugin" {
                    let status = std::process::Command::new("cp")
                        .arg("-R")
                        .arg(entry.path())
                        .arg(root)
                        .status()
                        .unwrap();
                    assert!(status.success());
                }
            }
        },
        |body| {
            // One call for each message of the person's, so each turn really uses a tool.
            match tool_results(body).len() % 2 {
                0 => tool_call("c", "shell", &json!({"command": "ls"})),
                _ => reply("Done."),
            }
        },
    )
    .await
}

/// An MCP server with `run_query` and `other`, answering each call.
async fn mcp_server() -> MockServer {
    let mcp = MockServer::builder().start().await;
    Mock::given(method("POST"))
        .respond_with(|request: &Request| {
            let body: Value = serde_json::from_slice(&request.body).unwrap();
            let schema = json!({"type": "object", "properties": {}});
            let result = match body["method"].as_str().unwrap_or_default() {
                "initialize" => json!({"protocolVersion": "2025-03-26", "capabilities": {},
                    "serverInfo": {"name": "stats", "version": "1"}}),
                "tools/list" => json!({"tools": [
                    {"name": "run_query", "description": "x", "inputSchema": schema},
                    {"name": "other", "description": "x", "inputSchema": schema}
                ]}),
                "tools/call" => json!({"content": [{"type": "text", "text": "served"}]}),
                _ => json!({}),
            };
            ResponseTemplate::new(200)
                .set_body_json(json!({"jsonrpc": "2.0", "id": body["id"], "result": result}))
        })
        .mount(&mcp)
        .await;
    mcp
}

/// The wire name of the main agent's MCP tool whose name ends in `tail`.
fn mcp_wire_name(body: &Value, tail: &str) -> Option<String> {
    body["tools"].as_array()?.iter().find_map(|tool| {
        let name = tool["function"]["name"].as_str()?;
        (name.starts_with("mcp_") && name.ends_with(tail)).then(|| name.to_owned())
    })
}

async fn called(mcp: &MockServer, tool: &str) -> bool {
    mcp.received_requests()
        .await
        .unwrap()
        .iter()
        .any(|request| {
            let body: Value = request.body_json().unwrap_or(Value::Null);
            body["method"] == "tools/call" && body["params"]["name"] == tool
        })
}
