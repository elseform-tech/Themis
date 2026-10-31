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
    serving: tokio::task::JoinHandle<()>,
}

/// A chat whose model answers by `respond`, with a Claude plugin holding `hooks` and `files`
/// imported through the CLI and every hook enabled.
async fn app(
    hooks: Value,
    files: &[(&str, &str)],
    respond: impl Fn(&Value) -> ResponseTemplate + Send + Sync + 'static,
) -> App {
    let llm = MockServer::start().await;
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
    let server = Server::bind(state, data.path()).await.unwrap();
    let serving = tokio::spawn(async move {
        let _ = server.run().await;
    });
    let plugins = tempfile::tempdir().unwrap();
    let folder = plugin_folder(plugins.path(), &hooks, files);
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

/// Sends `task` in YOLO and waits until the run is over.
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
            call(&data, "get_thread", json!({"threadId": thread}))["running"] != true
        });
    })
    .await
    .unwrap();
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

#[tokio::test(flavor = "multi_thread")]
async fn a_claude_guard_sees_claudes_fields_and_still_blocks_with_its_options() {
    // statusMessage, async: false and shell are options that must not lose the guard.
    let app = app(
        json!({"PreToolUse": [{"matcher": "Bash", "hooks": [{
            "type": "command",
            "command": "sh",
            "args": ["${CLAUDE_PLUGIN_ROOT}/guard.sh"],
            "statusMessage": "Checking…",
            "async": false,
            "shell": "bash"
        }]}]}),
        &[("guard.sh", GUARD)],
        calls_once(
            "shell",
            json!({"command": "git", "args": ["push", "origin", "main"]}),
        ),
    )
    .await;
    run(&app, "push it").await;
    let result = last_tool_result(&app).await;
    assert!(result.contains("pushes are frozen"), "{result}");
    assert!(result.contains("blocked"), "{result}");
    let seen: Value = serde_json::from_slice(
        &std::fs::read(app.project.path().join("seen.json")).expect("the guard ran"),
    )
    .unwrap();
    assert_eq!(seen["tool_name"], "Bash", "{seen}");
    assert_eq!(seen["tool_input"]["command"], "git push origin main");
    assert_eq!(seen["hook_event_name"], "PreToolUse");
    assert_eq!(seen["session_id"], app.thread.as_str());
    // Themis's own fields stay.
    assert_eq!(seen["tool"], "shell");
    app.serving.abort();
}

#[tokio::test(flavor = "multi_thread")]
async fn a_prompt_hook_that_exits_2_blocks_the_turn_but_a_session_hook_does_not() {
    let app = app(
        json!({
            "UserPromptSubmit": [{"hooks": [
                {"type": "command", "command": "echo 'not this prompt' >&2; exit 2"}
            ]}]
        }),
        &[],
        |_| reply("Never asked."),
    )
    .await;
    run(&app, "hello").await;
    assert!(
        bodies(&app.llm).await.is_empty(),
        "a blocked turn never reaches the model"
    );
    app.serving.abort();

    let app = crate::app(
        json!({
            "SessionStart": [{"hooks": [
                {"type": "command", "command": "echo 'no key' >&2; exit 2"}
            ]}]
        }),
        &[],
        |_| reply("Answered."),
    )
    .await;
    run(&app, "hello").await;
    assert!(
        !bodies(&app.llm).await.is_empty(),
        "a SessionStart hook can't stop every turn"
    );
    app.serving.abort();
}

#[tokio::test(flavor = "multi_thread")]
async fn if_runs_the_guard_only_on_the_commands_it_names() {
    let hooks = json!({"PreToolUse": [{"matcher": "Bash", "hooks": [{
        "type": "command",
        "command": "touch guard-ran; echo 'incident open' >&2; exit 2",
        "if": "Bash(git commit *)"
    }]}]});
    // Another command: the guard stays out of it, and it runs.
    let app = crate::app(
        hooks.clone(),
        &[],
        calls_once(
            "shell",
            json!({"command": "touch", "args": ["other-ran"], "background": false}),
        ),
    )
    .await;
    run(&app, "other").await;
    assert!(exists(&app, "other-ran"));
    assert!(
        !exists(&app, "guard-ran"),
        "the guard ran on another command"
    );
    app.serving.abort();
    // The named command, directly and inside a wrapper: blocked.
    for args in [
        json!({"command": "git", "args": ["commit", "-m", "x"]}),
        json!({"command": "git", "args": ["-C", ".", "commit", "-m", "x"]}),
        json!({"command": "sh", "args": ["-c", "touch before && git commit -m x"]}),
    ] {
        let app = crate::app(hooks.clone(), &[], calls_once("shell", args.clone())).await;
        run(&app, "commit").await;
        let result = last_tool_result(&app).await;
        assert!(exists(&app, "guard-ran"), "{args}");
        assert!(result.contains("incident open"), "{args}: {result}");
        assert!(!exists(&app, "before"), "the command ran anyway: {args}");
        app.serving.abort();
    }
    // A script it can't read: the guard runs, because it might be the named command.
    let app = crate::app(
        hooks,
        &[],
        calls_once(
            "shell",
            json!({"command": "sh", "args": ["-c", "$PROGRAM commit -m x"]}),
        ),
    )
    .await;
    run(&app, "hidden").await;
    assert!(
        exists(&app, "guard-ran"),
        "an unreadable script skipped the guard"
    );
    app.serving.abort();
}

#[tokio::test(flavor = "multi_thread")]
async fn an_async_hook_does_not_hold_the_turn_and_cannot_block() {
    let app = app(
        json!({"PreToolUse": [{"matcher": "Bash", "hooks": [{
            "type": "command",
            "command": "sleep 3; touch late; echo '{\"decision\":\"block\",\"reason\":\"no\"}'",
            "async": true
        }]}]}),
        &[],
        calls_once("shell", json!({"command": "touch", "args": ["ran"]})),
    )
    .await;
    let started = Instant::now();
    run(&app, "go").await;
    assert!(exists(&app, "ran"), "an async hook can't block the call");
    assert!(
        !exists(&app, "late"),
        "the turn waited {:?} for the async hook",
        started.elapsed()
    );
    let project = app.project.path().to_owned();
    tokio::task::spawn_blocking(move || {
        until("the async hook never finished", || {
            project.join("late").exists()
        });
    })
    .await
    .unwrap();
    app.serving.abort();
}

/// An MCP server with `run_query` and `other`, answering each call.
async fn mcp_server() -> MockServer {
    let mcp = MockServer::start().await;
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

#[tokio::test(flavor = "multi_thread")]
async fn an_mcp_matcher_guards_the_tools_it_names_and_only_those() {
    let mcp = mcp_server().await;
    let file = json!({"mcpServers": {"stats": {"type": "http", "url": mcp.uri()}}}).to_string();
    let hooks = json!({"PreToolUse": [{"matcher": "mcp__.*__run_query", "hooks": [{
        "type": "command",
        "command": "cat > seen.json; echo 'query refused' >&2; exit 2"
    }]}]});
    let responder = |tail: &'static str| {
        move |body: &Value| {
            if tool_results(body).is_empty() {
                let name = mcp_wire_name(body, tail).expect("the tool");
                tool_call("c1", &name, &json!({}))
            } else {
                reply("Done.")
            }
        }
    };
    // The guard refuses the tool it names; the server never sees the call.
    let guarded = app(
        hooks.clone(),
        &[(".mcp.json", &file)],
        responder("_run_query"),
    )
    .await;
    run(&guarded, "query").await;
    let result = last_tool_result(&guarded).await;
    assert!(result.contains("query refused"), "{result}");
    assert!(!called(&mcp, "run_query").await, "the guard let it through");
    let seen: Value =
        serde_json::from_slice(&std::fs::read(guarded.project.path().join("seen.json")).unwrap())
            .unwrap();
    assert_eq!(seen["tool_name"], "mcp__stats__run_query", "{seen}");
    guarded.serving.abort();
    // Another tool of the same server passes.
    let free = app(hooks, &[(".mcp.json", &file)], responder("_other")).await;
    run(&free, "other").await;
    assert!(called(&mcp, "other").await);
    assert!(!free.project.path().join("seen.json").exists());
    free.serving.abort();
}

#[tokio::test(flavor = "multi_thread")]
async fn stop_failure_runs_only_when_the_run_failed() {
    let hooks = json!({"StopFailure": [{"hooks": [
        {"type": "command", "command": "touch failed-hook"}
    ]}]});
    let good = app(hooks.clone(), &[], |_| reply("Fine.")).await;
    run(&good, "hello").await;
    assert!(!exists(&good, "failed-hook"), "it ran on a good turn");
    good.serving.abort();

    let bad = app(hooks, &[], |_| {
        ResponseTemplate::new(401).set_body_string("no")
    })
    .await;
    run(&bad, "hello").await;
    let project = bad.project.path().to_owned();
    tokio::task::spawn_blocking(move || {
        until("the hook never ran on a failed run", || {
            project.join("failed-hook").exists()
        });
    })
    .await
    .unwrap();
    bad.serving.abort();
}
