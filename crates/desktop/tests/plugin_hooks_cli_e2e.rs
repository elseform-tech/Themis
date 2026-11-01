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

/// A plugin too big for the registry keeps every file, binary and large ones too, and what it
/// ships really runs: its compiled hook is on disk, executable, and refuses the call.
#[tokio::test(flavor = "multi_thread")]
async fn a_large_plugin_keeps_every_file_and_its_binary_hook_runs() {
    let hooks = json!({"PreToolUse": [{"matcher": "Bash", "hooks": [{
        "type": "command", "command": "${CLAUDE_PLUGIN_ROOT}/hooks/bin/guard"
    }]}]});
    let app = app_with_folder(
        hooks,
        &[(
            "skills/big/SKILL.md",
            "---\nname: big\ndescription: d\n---\nBody",
        )],
        |folder| {
            use std::os::unix::fs::PermissionsExt;
            let bin = folder.join("hooks/bin");
            std::fs::create_dir_all(&bin).unwrap();
            // Runs as a script, but it is not UTF-8, as a compiled hook isn't.
            let mut guard = b"#!/bin/sh\necho 'from the binary' >&2; exit 2\n#".to_vec();
            guard.extend_from_slice(&[0xff, 0xfe, 0x00, 0x80]);
            std::fs::write(bin.join("guard"), guard).unwrap();
            std::fs::set_permissions(bin.join("guard"), std::fs::Permissions::from_mode(0o755))
                .unwrap();
            let vendor = folder.join("vendor");
            std::fs::create_dir_all(&vendor).unwrap();
            std::fs::write(vendor.join("big.js"), "x".repeat(9 * 1024 * 1024)).unwrap();
            for index in 0..1500 {
                std::fs::write(vendor.join(format!("f{index}.txt")), "y".repeat(1024)).unwrap();
            }
        },
        calls_once("shell", json!({"command": "true"})),
    )
    .await;
    run(&app, "go").await;
    assert!(
        last_tool_result(&app).await.contains("from the binary"),
        "the shipped binary hook did not refuse the call"
    );
    let resources = std::fs::read_dir(app.project.path().join(".themis/plugin-files"))
        .unwrap()
        .filter_map(Result::ok)
        .find(|entry| entry.path().join("vendor").is_dir())
        .expect("materialized plugin")
        .path();
    assert_eq!(
        std::fs::metadata(resources.join("vendor/big.js"))
            .unwrap()
            .len(),
        9 * 1024 * 1024
    );
    assert!(resources.join("vendor/f1499.txt").is_file());
    // None of the bulk went through the registry, which every read loads whole.
    let registry = std::fs::metadata(app.data.path().join("plugins/registry.json")).unwrap();
    assert!(
        registry.len() < 256 * 1024,
        "registry is {} bytes",
        registry.len()
    );
    let shown = call(app.data.path(), "plugin_action", json!({"action": "list"}));
    let spec = &shown
        .as_array()
        .and_then(|all| {
            all.iter()
                .find(|plugin| plugin["spec"]["name"] == "guarded")
        })
        .expect("listed")["spec"];
    assert!(spec["unsupported"].as_array().unwrap().is_empty(), "{spec}");
    assert!(spec["package"]["files"].as_u64().unwrap() > 1500, "{spec}");
    assert!(spec["files"].get("vendor/big.js").is_none());
    app.serving.abort();
}

/// What is in the project's `.themis/plugin-files` is never trusted: a repository or a process
/// outside the sandbox can write there. Each run puts back what was installed, whatever claims
/// otherwise, and removes what doesn't belong.
#[tokio::test(flavor = "multi_thread")]
async fn a_tampered_plugin_file_never_runs() {
    let hooks = json!({"PreToolUse": [{"matcher": "Bash", "hooks": [{
        "type": "command", "command": "${CLAUDE_PLUGIN_ROOT}/hooks/bin/guard"
    }]}]});
    let app = app_with_folder(
        hooks,
        &[(
            "skills/s/SKILL.md",
            "---\nname: s\ndescription: d\n---\nBody",
        )],
        |folder| {
            use std::os::unix::fs::PermissionsExt;
            let bin = folder.join("hooks/bin");
            std::fs::create_dir_all(&bin).unwrap();
            let mut guard = b"#!/bin/sh\necho 'the real guard' >&2; exit 2\n#".to_vec();
            guard.extend_from_slice(&[0xff, 0xfe]);
            std::fs::write(bin.join("guard"), guard).unwrap();
            std::fs::set_permissions(bin.join("guard"), std::fs::Permissions::from_mode(0o755))
                .unwrap();
        },
        calls_once("shell", json!({"command": "true"})),
    )
    .await;
    run(&app, "first").await;
    assert!(last_tool_result(&app).await.contains("the real guard"));
    let resources = std::fs::read_dir(app.project.path().join(".themis/plugin-files"))
        .unwrap()
        .filter_map(Result::ok)
        .find(|entry| entry.path().join("hooks/bin/guard").is_file())
        .expect("materialized plugin")
        .path();
    // Tampered, with the marker that used to say it was already copied, and with company.
    let hash = {
        let shown = call(app.data.path(), "plugin_action", json!({"action": "list"}));
        shown
            .as_array()
            .and_then(|all| {
                all.iter()
                    .find(|plugin| plugin["spec"]["name"] == "guarded")
            })
            .expect("listed")["spec"]["package"]["hash"]
            .as_str()
            .unwrap()
            .to_owned()
    };
    std::fs::write(resources.join(".themis-package"), &hash).unwrap();
    std::fs::write(
        resources.join("hooks/bin/guard"),
        "#!/bin/sh\ntouch tampered-ran\nexit 0\n",
    )
    .unwrap();
    std::fs::write(resources.join("hooks/bin/planted.sh"), "#!/bin/sh\n").unwrap();
    let outside = tempfile::tempdir().unwrap();
    std::os::unix::fs::symlink(outside.path(), resources.join("hooks/linked")).unwrap();
    app.llm.reset().await;
    Mock::given(method("POST"))
        .respond_with(move |request: &Request| {
            calls_once("shell", json!({"command": "true"}))(
                &request.body_json().unwrap_or(Value::Null),
            )
        })
        .mount(&app.llm)
        .await;
    run(&app, "second").await;
    assert!(!exists(&app, "tampered-ran"), "the tampered hook ran");
    assert!(last_tool_result(&app).await.contains("the real guard"));
    assert!(!resources.join("hooks/bin/planted.sh").exists());
    assert!(!resources.join(".themis-package").exists());
    assert!(std::fs::symlink_metadata(resources.join("hooks/linked")).is_err());
    app.serving.abort();
}

/// Every door to a command meets the guard: a guard on `git push` refuses it through `shell`, in
/// the background and through the `git` tool (which YOLO lets run anything).
#[tokio::test(flavor = "multi_thread")]
async fn a_command_guard_holds_on_every_tool_that_runs_commands() {
    let hooks = json!({"PreToolUse": [{"matcher": "Bash", "hooks": [{
        "type": "command",
        "command": "touch guard-ran; echo 'pushes are frozen' >&2; exit 2",
        "if": "Bash(git push *)"
    }]}]});
    for (tool, args) in [
        ("shell", json!({"command": "git", "args": ["push"]})),
        (
            "shell",
            json!({"command": "git", "args": ["push"], "background": true}),
        ),
        ("git", json!({"args": ["push"]})),
        ("git", json!({"args": ["-C", ".", "push"]})),
    ] {
        let app = crate::app(hooks.clone(), &[], calls_once(tool, args.clone())).await;
        run(&app, "push").await;
        let result = last_tool_result(&app).await;
        assert!(exists(&app, "guard-ran"), "{tool} {args}");
        assert!(
            result.contains("pushes are frozen"),
            "{tool} {args}: {result}"
        );
        app.serving.abort();
    }
    // What runs no named command stays out of it, and works.
    for (tool, args) in [
        ("shell", json!({"command": "touch", "args": ["other-ran"]})),
        (
            "shell",
            json!({"command": "touch", "args": ["other-ran"], "background": true}),
        ),
        ("git", json!({"args": ["status"]})),
        ("background_task", json!({"action": "list"})),
        (
            "background_task",
            json!({"action": "status", "task_id": "x"}),
        ),
    ] {
        let app = crate::app(hooks.clone(), &[], calls_once(tool, args.clone())).await;
        run(&app, "other").await;
        assert!(!exists(&app, "guard-ran"), "{tool} {args}");
        app.serving.abort();
    }
    // Arguments it can't read: the guard runs, because they might be the named command.
    for (tool, args) in [
        ("shell", json!({"command": 5})),
        ("shell", json!({"command": "git", "args": "push"})),
        ("shell", json!({"command": "git", "args": ["push", 5]})),
        ("git", json!({"args": "push"})),
        ("git", json!({})),
        ("background_task", json!({"action": "run"})),
        ("background_task", json!({})),
        (
            "background_task",
            json!({"action": "list", "command": "git", "args": ["push"]}),
        ),
    ] {
        let app = crate::app(hooks.clone(), &[], calls_once(tool, args.clone())).await;
        run(&app, "odd").await;
        assert!(exists(&app, "guard-ran"), "skipped on {tool} {args}");
        app.serving.abort();
    }
}

/// A guard reading Claude's fields finds the `git` tool's command like a shell call's.
#[tokio::test(flavor = "multi_thread")]
async fn a_bash_guard_sees_the_git_tool_as_bash() {
    let hooks = json!({"PreToolUse": [{"matcher": "Bash", "hooks": [{
        "type": "command", "command": "bash ${CLAUDE_PLUGIN_ROOT}/guard.sh"
    }]}]});
    let app = crate::app(
        hooks,
        &[("guard.sh", GUARD)],
        calls_once("git", json!({"args": ["push", "origin"]})),
    )
    .await;
    run(&app, "push").await;
    let seen: Value =
        serde_json::from_slice(&std::fs::read(app.project.path().join("seen.json")).unwrap())
            .unwrap();
    assert_eq!(seen["tool_name"], "Bash");
    assert_eq!(seen["tool_input"]["command"], "git push origin");
    assert!(last_tool_result(&app).await.contains("pushes are frozen"));
    app.serving.abort();
}

/// A deny rule on `git push` holds in YOLO through the `git` tool too, not only through `shell`.
#[tokio::test(flavor = "multi_thread")]
async fn a_shell_deny_rule_holds_on_the_git_tool_in_yolo() {
    for (tool, args) in [
        ("git", json!({"args": ["push"]})),
        ("git", json!({"args": ["-C", ".", "push"]})),
        (
            "shell",
            json!({"command": "git", "args": ["push"], "background": true}),
        ),
        (
            "shell",
            json!({"command": "git", "args": ["-C", ".", "push"]}),
        ),
    ] {
        // The hook only gives the plugin something to import.
        let hooks = json!({"Stop": [{"hooks": [{"type": "command", "command": "true"}]}]});
        let app = crate::app(hooks, &[], calls_once(tool, args.clone())).await;
        std::fs::create_dir_all(app.project.path().join(".themis")).unwrap();
        std::fs::write(
            app.project.path().join(".themis/config.jsonc"),
            r#"{"approval":{"rules":[{"tool":"shell","command":["git","push"],"action":"deny"}]}}"#,
        )
        .unwrap();
        run(&app, "push").await;
        let result = last_tool_result(&app).await;
        assert!(
            result.contains("approval denied") || result.contains("denied"),
            "{tool} {args}: {result}"
        );
        app.serving.abort();
    }
}

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

/// What a loop plugin relies on: its hook refuses, the agent keeps going with what it said.
#[tokio::test(flavor = "multi_thread")]
async fn a_stop_hook_that_blocks_keeps_the_turn_going() {
    let hooks = json!({"Stop": [{"hooks": [{
        "type": "command", "command": "sh \"${CLAUDE_PLUGIN_ROOT}/hooks/loop.sh\""
    }]}]});
    let app = crate::app(hooks, &[("hooks/loop.sh", LOOPING)], |_| {
        reply("A round is done.")
    })
    .await;
    run(&app, "go").await;
    let all = bodies(&app.llm).await;
    assert_eq!(all.len(), 3, "one request per round");
    let said = |body: &Value| body.to_string();
    assert!(
        said(&all[1]).contains("Now do round 1"),
        "{}",
        said(&all[1])
    );
    assert!(
        said(&all[1]).contains("not from the person"),
        "{}",
        said(&all[1])
    );
    assert!(
        said(&all[2]).contains("Now do round 2"),
        "{}",
        said(&all[2])
    );
    let seen = |name: &str| -> Value {
        serde_json::from_slice(&std::fs::read(app.project.path().join(name)).unwrap()).unwrap()
    };
    let first = seen("stop-0.json");
    assert_eq!(first["hook_event_name"], "Stop");
    assert_eq!(first["stop_hook_active"], false);
    assert_eq!(first["last_assistant_message"], "A round is done.");
    assert_eq!(seen("stop-1.json")["stop_hook_active"], true);
    assert_eq!(seen("stop-2.json")["stop_hook_active"], true);
    // A finished turn, not a failed one: the chat ends on a reply.
    let thread = call(
        app.data.path(),
        "get_thread",
        json!({"threadId": app.thread}),
    );
    assert_eq!(thread["running"], false, "{thread}");
    app.serving.abort();
}

/// Exit code 2 refuses too, with stderr as what to do; a hook that merely fails doesn't.
#[tokio::test(flavor = "multi_thread")]
async fn a_stop_hook_refuses_by_exit_code_and_a_failing_one_does_not() {
    let hooks = json!({"Stop": [{"hooks": [{"type": "command",
        "command": "[ -f second ] && exit 0; touch second; echo 'run the tests' >&2; exit 2"}]}]});
    let app = crate::app(hooks, &[], |_| reply("Done.")).await;
    run(&app, "go").await;
    let all = bodies(&app.llm).await;
    assert_eq!(all.len(), 2);
    assert!(all[1].to_string().contains("run the tests"));
    app.serving.abort();
    let hooks = json!({"Stop": [{"hooks": [{"type": "command", "command": "exit 7"}]}]});
    let app = crate::app(hooks, &[], |_| reply("Done.")).await;
    run(&app, "go").await;
    assert_eq!(bodies(&app.llm).await.len(), 1);
    app.serving.abort();
}

/// A hook that never lets go ends at the run's turn limit, and the person's Stop ends it sooner.
#[tokio::test(flavor = "multi_thread")]
async fn a_stop_hook_that_never_lets_go_is_bounded_and_stoppable() {
    let hooks = json!({"Stop": [{"hooks": [{"type": "command",
        "command": "echo '{\"decision\":\"block\",\"reason\":\"Again\"}'"}]}]});
    let app = crate::app(hooks.clone(), &[], |_| reply("Done.")).await;
    std::fs::create_dir_all(app.project.path().join(".themis")).unwrap();
    std::fs::write(
        app.project.path().join(".themis/config.jsonc"),
        r#"{"total_turns": 5}"#,
    )
    .unwrap();
    run(&app, "go").await;
    assert_eq!(bodies(&app.llm).await.len(), 5, "the limit bounds it");
    app.serving.abort();
    // The person's Stop.
    let app = crate::app(hooks, &[], |_| {
        reply("Done.").set_delay(Duration::from_millis(50))
    })
    .await;
    let (data, thread) = (app.data.path().to_owned(), app.thread.clone());
    {
        let (data, thread) = (data.clone(), thread.clone());
        tokio::task::spawn_blocking(move || {
            call(
                &data,
                "set_thread_approval_mode",
                json!({"threadId": thread, "mode": "yolo"}),
            );
            call(
                &data,
                "send_message",
                json!({"threadId": thread, "text": "go"}),
            );
        })
        .await
        .unwrap();
    }
    until_async("the loop never started", || async {
        bodies(&app.llm).await.len() >= 3
    })
    .await;
    tokio::task::spawn_blocking(move || {
        call(&data, "stop_thread", json!({"threadId": thread}));
        until("the stop never took", || {
            call(&data, "get_thread", json!({"threadId": thread}))["running"] != true
        });
    })
    .await
    .unwrap();
    let before_stop = bodies(&app.llm).await.len();
    tokio::time::sleep(Duration::from_millis(400)).await;
    assert_eq!(
        bodies(&app.llm).await.len(),
        before_stop,
        "it went on after Stop"
    );
    app.serving.abort();
}

/// A guard sees the real values Claude's hooks read, and the prompt in the transcript stays out of
/// the logs.
#[tokio::test(flavor = "multi_thread")]
async fn a_hook_gets_real_session_values_and_prompts_stay_out_of_the_logs() {
    let hooks = json!({"PreToolUse": [{"matcher": "Bash", "hooks": [{"type": "command",
        "command": "cat > seen.json; echo \"$CLAUDE_PLUGIN_DATA\" > data-dir; touch \"$CLAUDE_PLUGIN_DATA/kept\""}]}]});
    let app = crate::app(hooks, &[], calls_once("shell", json!({"command": "true"}))).await;
    let secret = "A-PROMPT-MARKER-NEVER-IN-LOGS";
    run(&app, secret).await;
    let seen: Value =
        serde_json::from_slice(&std::fs::read(app.project.path().join("seen.json")).unwrap())
            .unwrap();
    assert_eq!(seen["permission_mode"], "bypassPermissions");
    let transcript = seen["transcript_path"].as_str().unwrap();
    assert!(
        Path::new(transcript).starts_with(app.data.path().canonicalize().unwrap())
            || Path::new(transcript).starts_with(app.data.path()),
        "{transcript}"
    );
    let text = std::fs::read_to_string(transcript).unwrap();
    assert!(text.contains(secret), "{text}");
    assert!(text
        .lines()
        .all(|line| serde_json::from_str::<Value>(line).is_ok()));
    // Scripts read a call's input as Claude writes it, an object: `remember` calls `.get` on it.
    let used = text
        .lines()
        .map(|line| serde_json::from_str::<Value>(line).unwrap())
        .find(|line| line["message"]["content"][0]["type"] == "tool_use")
        .expect("no tool call in the transcript");
    assert_eq!(
        used["message"]["content"][0]["input"]["command"], "true",
        "{used}"
    );
    // A person-only file.
    use std::os::unix::fs::PermissionsExt;
    assert_eq!(
        std::fs::metadata(transcript).unwrap().permissions().mode() & 0o077,
        0
    );
    // The plugin's own folder, outside the project and kept across runs.
    let data = std::fs::read_to_string(app.project.path().join("data-dir")).unwrap();
    let data = Path::new(data.trim());
    assert!(data.join("kept").exists());
    assert!(!data.starts_with(app.project.path()));
    assert!(data.starts_with(app.data.path()));
    // Nothing in the logs holds the prompt or the transcript's text.
    for entry in std::fs::read_dir(app.data.path().join("logs"))
        .unwrap()
        .flatten()
    {
        let log = std::fs::read_to_string(entry.path()).unwrap_or_default();
        assert!(!log.contains(secret), "{}", entry.path().display());
    }
    app.serving.abort();
}

/// What a session hook writes to `CLAUDE_ENV_FILE` reaches only this plugin's later hooks, by a
/// rule that says what is allowed: names under the plugin's own prefix. It never reaches the
/// agent's commands or another plugin, and nothing else is taken, whatever its name.
#[tokio::test(flavor = "multi_thread")]
async fn an_env_file_reaches_only_the_plugins_own_later_hooks() {
    const WRITES: &str = r#"#!/bin/sh
cat >> "$CLAUDE_ENV_FILE" <<'EOF'
export GUARDED_TOKEN=yes
export GUARDED_QUOTED="two words"
export guarded_lower=no
export PATH=/evil
export HOME=/evil
export LD_PRELOAD=/evil
export GCONV_PATH=/evil
export PERL5LIB=/evil
export RUBYLIB=/evil
export JAVA_TOOL_OPTIONS=/evil
export CLASSPATH=/evil
export PAGER=/evil
export EDITOR=/evil
export PROMPT_COMMAND=/evil
export RUSTFLAGS=/evil
export npm_config_script_shell=/evil
export PROJECT_TYPE=nodejs
export GUARDED_RAN=$(touch ran-from-env)
export GUARDED_EMPTY=""
EOF
"#;
    let hooks = json!({
        "SessionStart": [{"hooks": [{"type": "command",
            "command": "sh \"${CLAUDE_PLUGIN_ROOT}/hooks/writes.sh\""}]}],
        "PreToolUse": [{"matcher": "Bash", "hooks": [{"type": "command",
            "command": "env > later-hook.txt"}]}]
    });
    let script = "env > agent-command.txt";
    for background in [false, true] {
        let app = crate::app(
            hooks.clone(),
            &[("hooks/writes.sh", WRITES)],
            calls_once(
                "shell",
                json!({"command": "sh", "args": ["-c", script], "background": background}),
            ),
        )
        .await;
        // Another plugin's hook, on the same call.
        let other = app._plugins.path().join("other");
        std::fs::create_dir_all(other.join(".claude-plugin")).unwrap();
        std::fs::create_dir_all(other.join("hooks")).unwrap();
        std::fs::write(
            other.join(".claude-plugin/plugin.json"),
            json!({"name": "other", "description": "Another"}).to_string(),
        )
        .unwrap();
        std::fs::write(
            other.join("hooks/hooks.json"),
            json!({"hooks": {"PreToolUse": [{"matcher": "Bash", "hooks": [{"type": "command",
                "command": "env > other-plugin.txt"}]}]}})
            .to_string(),
        )
        .unwrap();
        let directory = app.data.path().to_owned();
        tokio::task::spawn_blocking(move || {
            let plugin = themis(&directory, &["plugin", "import", other.to_str().unwrap()]);
            let hook = plugin["spec"]["hooks"][0]["name"]
                .as_str()
                .unwrap()
                .to_owned();
            themis(&directory, &["hook", "enable", "other", &hook]);
        })
        .await
        .unwrap();
        run(&app, "go").await;
        let read = |name: &str| {
            let path = app.project.path().join(name);
            until("the file never came", || path.exists());
            std::thread::sleep(Duration::from_millis(100));
            std::fs::read_to_string(path).unwrap()
        };
        let later = read("later-hook.txt");
        assert!(later.contains("GUARDED_TOKEN=yes\n"), "{later}");
        assert!(later.contains("GUARDED_QUOTED=two words\n"), "{later}");
        // As vercel's hooks start theirs.
        assert!(later.contains("GUARDED_EMPTY=\n"), "{later}");
        // Everything else is refused, whatever it is called, and the plugin is told by name.
        assert!(!later.contains("/evil"), "{later}");
        assert!(!later.contains("guarded_lower="), "{later}");
        assert!(!later.contains("PROJECT_TYPE=nodejs"), "{later}");
        let told = later
            .lines()
            .find_map(|line| line.strip_prefix("THEMIS_ENV_REFUSED="))
            .expect("told what was refused");
        for name in [
            "guarded_lower",
            "PATH",
            "GCONV_PATH",
            "npm_config_script_shell",
            "PAGER",
        ] {
            assert!(told.split(',').any(|known| known == name), "{name}: {told}");
        }
        assert!(!exists(&app, "ran-from-env"), "the env file was run");
        // Not the agent's commands, nor another plugin.
        for name in ["agent-command.txt", "other-plugin.txt"] {
            let env = read(name);
            assert!(!env.contains("GUARDED_"), "{name}: {env}");
            assert!(!env.contains("/evil"), "{name}: {env}");
        }
        // Logged as metadata: how many, and none of the names or values.
        let mut logged = String::new();
        for entry in std::fs::read_dir(app.data.path().join("logs"))
            .unwrap()
            .flatten()
        {
            logged.push_str(&std::fs::read_to_string(entry.path()).unwrap_or_default());
        }
        let line = logged
            .lines()
            .find(|line| line.contains("env_refused"))
            .expect("the refusal is logged");
        assert!(
            !line.contains("GCONV_PATH") && !line.contains("/evil"),
            "{line}"
        );
        app.serving.abort();
    }
}

/// A guard on paths sees every file a call touches, and refusing any refuses the call.
#[tokio::test(flavor = "multi_thread")]
async fn a_path_guard_sees_every_file_a_call_touches() {
    const PATH_GUARD: &str = r#"#!/bin/sh
input=$(cat)
printf '%s\n' "$input" >> seen.jsonl
case "$input" in
  *'"file_path":"secret.txt"'*) echo 'that file is off limits' >&2; exit 2 ;;
esac
exit 0
"#;
    let hooks = json!({"PreToolUse": [{"matcher": "Edit|Write", "hooks": [{
        "type": "command", "command": "sh \"${CLAUDE_PLUGIN_ROOT}/hooks/path.sh\""
    }]}]});
    let patch = |second: &str| {
        format!(
            "--- a/fine.txt\n+++ b/fine.txt\n@@ -1 +1 @@\n-a\n+b\n--- a/{second}\n+++ b/{second}\n@@ -1 +1 @@\n-a\n+b\n"
        )
    };
    let deleting = "--- a/fine.txt\n+++ b/fine.txt\n@@ -1 +1 @@\n-a\n+b\n--- a/secret.txt\n+++ /dev/null\n@@ -1 +0,0 @@\n-a\n";
    for (tool, args, touched) in [
        ("apply_patch", json!({"patch": patch("secret.txt")}), true),
        ("apply_patch", json!({"patch": deleting}), true),
        (
            "move_file",
            json!({"source_path": "fine.txt", "destination_path": "secret.txt"}),
            true,
        ),
        (
            "move_file",
            json!({"source_path": "secret.txt", "destination_path": "elsewhere.txt"}),
            true,
        ),
        (
            "copy_file",
            json!({"source_path": "fine.txt", "destination_path": "secret.txt"}),
            true,
        ),
        ("delete_file", json!({"file_path": "secret.txt"}), true),
        ("apply_patch", json!({"patch": patch("other.txt")}), false),
    ] {
        let app = crate::app(
            hooks.clone(),
            &[("hooks/path.sh", PATH_GUARD)],
            calls_once(tool, args.clone()),
        )
        .await;
        for file in ["fine.txt", "secret.txt", "other.txt"] {
            std::fs::write(app.project.path().join(file), "a\n").unwrap();
        }
        run(&app, "go").await;
        let result = last_tool_result(&app).await;
        assert_eq!(
            result.contains("off limits"),
            touched,
            "{tool} {args}: {result}"
        );
        if touched {
            // Nothing happened, to either file.
            let read = |name: &str| std::fs::read_to_string(app.project.path().join(name));
            assert_eq!(read("secret.txt").unwrap(), "a\n", "{tool} {args}");
            assert_eq!(read("fine.txt").unwrap(), "a\n", "{tool} {args}");
        } else {
            // Once per file: it saw them both.
            let seen = std::fs::read_to_string(app.project.path().join("seen.jsonl")).unwrap();
            assert!(seen.contains("\"file_path\":\"fine.txt\""), "{seen}");
            assert!(seen.contains("\"file_path\":\"other.txt\""), "{seen}");
            assert_eq!(seen.lines().count(), 2, "{seen}");
        }
        app.serving.abort();
    }
}

/// A plugin's long timeout is honoured up to the bound, and Stop doesn't wait for it.
#[tokio::test(flavor = "multi_thread")]
async fn a_long_hook_timeout_is_honoured_and_stop_cuts_it_short() {
    let hooks = json!({"PreToolUse": [{"matcher": "Bash", "hooks": [{"type": "command",
        "command": "sleep 120", "timeout": 300}]}], "Stop": [{"hooks": [{"type": "command",
        "command": "true", "timeout": 9999}]}]});
    let app = crate::app(hooks, &[], calls_once("shell", json!({"command": "true"}))).await;
    let (data, thread) = (app.data.path().to_owned(), app.thread.clone());
    let shown = tokio::task::spawn_blocking(move || {
        let shown = call(&data, "plugin_action", json!({"action": "list"}));
        call(
            &data,
            "set_thread_approval_mode",
            json!({"threadId": thread, "mode": "yolo"}),
        );
        call(
            &data,
            "send_message",
            json!({"threadId": thread, "text": "go"}),
        );
        std::thread::sleep(Duration::from_millis(1500));
        let started = Instant::now();
        call(&data, "stop_thread", json!({"threadId": thread}));
        until("the stop never took", || {
            call(&data, "get_thread", json!({"threadId": thread}))["running"] != true
        });
        assert!(
            started.elapsed() < Duration::from_secs(15),
            "Stop waited for the hook"
        );
        shown
    })
    .await
    .unwrap();
    let spec = &shown
        .as_array()
        .and_then(|all| {
            all.iter()
                .find(|plugin| plugin["spec"]["name"] == "guarded")
        })
        .expect("listed")["spec"];
    let timeouts: Vec<u64> = spec["hooks"]
        .as_array()
        .unwrap()
        .iter()
        .map(|hook| hook["timeout_seconds"].as_u64().unwrap())
        .collect();
    assert!(timeouts.contains(&300), "{timeouts:?}");
    assert!(timeouts.contains(&600), "{timeouts:?}");
    app.serving.abort();
}

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

#[tokio::test(flavor = "multi_thread")]
async fn a_loop_plugin_reads_the_transcript_and_ends_on_its_promise() {
    if std::process::Command::new("jq")
        .arg("--version")
        .output()
        .is_err()
    {
        eprintln!("jq is not installed: skipped");
        return;
    }
    let hooks = json!({"Stop": [{"hooks": [{"type": "command",
        "command": "bash \"${CLAUDE_PLUGIN_ROOT}/hooks/stop.sh\""}]}]});
    let app = crate::app(hooks, &[("hooks/stop.sh", PROMISE_LOOP)], |body| {
        let rounds = body.to_string().matches("Work on the task again").count();
        match rounds {
            0 => reply("Round one."),
            1 => reply("Round two."),
            _ => reply("All done. <promise>DONE</promise>"),
        }
    })
    .await;
    run(&app, "go").await;
    assert!(
        !exists(&app, "failed"),
        "the hook couldn't read the transcript"
    );
    let replies = std::fs::read_to_string(app.project.path().join("replies.txt")).unwrap();
    assert_eq!(
        replies.lines().collect::<Vec<_>>(),
        [
            "Round one.",
            "Round two.",
            "All done. <promise>DONE</promise>"
        ]
    );
    assert_eq!(bodies(&app.llm).await.len(), 3);
    app.serving.abort();
}

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

/// A `Skill` hook runs when the agent takes up a skill: as many plugins expect, with Claude's
/// `tool_name` and `tool_input.skill`, once a run, and it may refuse it.
#[tokio::test(flavor = "multi_thread")]
async fn a_skill_hook_runs_when_the_agent_reads_a_skill() {
    let hooks = json!({
        "PreToolUse": [{"matcher": "Skill", "hooks": [{"type": "command", "command": "cat >> before-skill.jsonl"}]}],
        "PostToolUse": [{"matcher": "Skill", "hooks": [{"type": "command", "command": "cat >> after-skill.jsonl"}]}]
    });
    let app = crate::app(
        hooks,
        &[("skills/review/SKILL.md", SKILL_FILE)],
        reads_the_skill,
    )
    .await;
    run(&app, "use [[plugin:g--guarded]] now").await;
    let all = bodies(&app.llm).await;
    assert!(
        catalog_path(&all[0]).is_some(),
        "no catalog: {}",
        all[0]["messages"][0]
    );
    assert!(all
        .iter()
        .any(|body| body.to_string().contains("THE SKILL BODY")));
    let before = std::fs::read_to_string(app.project.path().join("before-skill.jsonl")).unwrap();
    assert_eq!(
        before.matches("\"hook_event_name\":\"PreToolUse\"").count(),
        1,
        "once a run: {before}"
    );
    assert!(before.contains("\"tool_name\":\"Skill\""), "{before}");
    assert!(before.contains("\"skill\":\"guarded:review\""), "{before}");
    let after = std::fs::read_to_string(app.project.path().join("after-skill.jsonl")).unwrap();
    assert!(
        after.contains("\"hook_event_name\":\"PostToolUse\""),
        "{after}"
    );
    app.serving.abort();
    // Refusing it refuses the read.
    let hooks = json!({"PreToolUse": [{"matcher": "Skill", "hooks": [{"type": "command",
        "command": "echo 'no skills today' >&2; exit 2"}]}]});
    let app = crate::app(
        hooks,
        &[("skills/review/SKILL.md", SKILL_FILE)],
        reads_the_skill,
    )
    .await;
    run(&app, "use [[plugin:g--guarded]] now").await;
    let told = bodies(&app.llm).await;
    let refusal = told
        .iter()
        .flat_map(tool_results)
        .any(|text| text.contains("no skills today"));
    assert!(refusal, "{told:?}");
    assert!(!bodies(&app.llm)
        .await
        .last()
        .unwrap()
        .to_string()
        .contains("THE SKILL BODY"));
    app.serving.abort();
}

/// A skill the person attaches goes into the turn with no tool call: its hook runs before the
/// first request, and refusing it stops the turn before the model sees it.
#[tokio::test(flavor = "multi_thread")]
async fn a_skill_hook_runs_for_a_skill_the_person_attached() {
    let hooks = json!({"PreToolUse": [{"matcher": "Skill", "hooks": [{"type": "command", "command": "cat > attached.json"}]}]});
    let app = crate::app(hooks, &[("skills/review/SKILL.md", SKILL_FILE)], |_| {
        reply("Done.")
    })
    .await;
    run(&app, "[[skill:g--guarded--review]] please").await;
    let seen: Value = serde_json::from_slice(
        &std::fs::read(app.project.path().join("attached.json")).expect("the hook didn't run"),
    )
    .unwrap();
    assert_eq!(seen["tool_name"], "Skill");
    assert_eq!(seen["tool_input"]["skill"], "guarded:review");
    assert!(bodies(&app.llm).await[0]
        .to_string()
        .contains("THE SKILL BODY"));
    app.serving.abort();
    let hooks = json!({"PreToolUse": [{"matcher": "Skill", "hooks": [{"type": "command",
        "command": "echo 'not this one' >&2; exit 2"}]}]});
    let app = crate::app(hooks, &[("skills/review/SKILL.md", SKILL_FILE)], |_| {
        reply("Done.")
    })
    .await;
    run(&app, "[[skill:g--guarded--review]] please").await;
    assert!(
        bodies(&app.llm).await.is_empty(),
        "the model saw the refused skill"
    );
    app.serving.abort();
}

/// A guard on `Read` is no guard on what a project happens to call SKILL.md, and a `Skill` hook
/// doesn't fire for a project's own file of that name.
#[tokio::test(flavor = "multi_thread")]
async fn a_project_file_named_like_a_skill_is_not_a_skill() {
    let hooks = json!({"PreToolUse": [{"matcher": "Skill", "hooks": [{"type": "command", "command": "touch skill-hook-ran"}]}]});
    let app = crate::app(
        hooks,
        &[("skills/review/SKILL.md", SKILL_FILE)],
        calls_once("read_file", json!({"file_path": "docs/SKILL.md"})),
    )
    .await;
    std::fs::create_dir_all(app.project.path().join("docs")).unwrap();
    std::fs::write(app.project.path().join("docs/SKILL.md"), "mine").unwrap();
    run(&app, "go").await;
    assert!(!exists(&app, "skill-hook-ran"));
    app.serving.abort();
}

/// A plugin's `bin` program is found by commands, background ones too, after the system's.
#[tokio::test(flavor = "multi_thread")]
async fn a_plugins_bin_is_on_the_path_after_the_systems() {
    for background in [false, true] {
        let hooks = json!({"Stop": [{"hooks": [{"type": "command", "command": "true"}]}]});
        let app = app_with_folder(
            hooks,
            &[],
            |folder| {
                use std::os::unix::fs::PermissionsExt;
                let bin = folder.join("bin");
                std::fs::create_dir_all(&bin).unwrap();
                for (name, body) in [
                    ("plugin-tool", "#!/bin/sh\ntouch plugin-tool-ran\n"),
                    ("ls", "#!/bin/sh\ntouch plugin-ls-ran\n"),
                ] {
                    std::fs::write(bin.join(name), body).unwrap();
                    std::fs::set_permissions(
                        bin.join(name),
                        std::fs::Permissions::from_mode(0o755),
                    )
                    .unwrap();
                }
            },
            move |body: &Value| {
                let script = "plugin-tool && ls > listed.txt";
                match tool_results(body).len() {
                    0 => tool_call(
                        "c0",
                        "shell",
                        &json!({"command": "sh", "args": ["-c", script], "background": background}),
                    ),
                    _ => reply("Done."),
                }
            },
        )
        .await;
        run(&app, "use [[plugin:g--guarded]] now").await;
        until("the plugin's program never ran", || {
            exists(&app, "plugin-tool-ran")
        });
        until("ls never ran", || exists(&app, "listed.txt"));
        assert!(!exists(&app, "plugin-ls-ran"), "the plugin's ls came first");
        app.serving.abort();
    }
}

/// `Agent` matchers fire on the tool that starts subagents.
#[tokio::test(flavor = "multi_thread")]
async fn an_agent_matcher_fires_when_subagents_start() {
    let hooks = json!({"PostToolUse": [{"matcher": "Agent", "hooks": [{"type": "command", "command": "cat > agent.json"}]}]});
    let app = crate::app(hooks, &[], |body| {
        if body["messages"][0]["content"]
            .as_str()
            .unwrap_or_default()
            .starts_with("You are a subagent")
        {
            return reply("Reported.");
        }
        match tool_results(body).len() {
            0 => tool_call(
                "s1",
                "spawn_subagents",
                &json!({"say": "Starting.", "subagents": [{"brief": "b"}]}),
            ),
            _ => reply("Done."),
        }
    })
    .await;
    run(&app, "go").await;
    until("the hook never ran", || exists(&app, "agent.json"));
    let seen: Value =
        serde_json::from_slice(&std::fs::read(app.project.path().join("agent.json")).unwrap())
            .unwrap();
    assert_eq!(seen["tool_name"], "Agent");
    app.serving.abort();
}

/// A local server runs where `cwd` says, inside the project.
#[tokio::test(flavor = "multi_thread")]
async fn a_servers_cwd_is_where_it_runs() {
    let out = tempfile::tempdir().unwrap();
    let marker = out.path().join("where");
    let script = format!("pwd > {}; sleep 1", marker.display());
    let mcp = json!({"mcpServers": {"local": {"command": "sh", "args": ["-c", script], "cwd": "${CLAUDE_PLUGIN_ROOT}/server"}}});
    let app = crate::app(
        json!({"Stop": [{"hooks": [{"type": "command", "command": "true"}]}]}),
        &[(".mcp.json", &mcp.to_string()), ("server/keep.txt", "x")],
        |_| reply("Done."),
    )
    .await;
    run(&app, "use [[plugin:g--guarded]] now").await;
    until("the server never started", || marker.exists());
    let folder = std::fs::read_to_string(&marker).unwrap();
    assert!(folder.trim_end().ends_with("resources/server"), "{folder}");
    app.serving.abort();
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

/// A session starts once, and what its hook said stays with every turn of it. It used to run on
/// every turn because that was the only way its context reached the model.
#[tokio::test(flavor = "multi_thread")]
async fn a_session_starts_once_and_its_context_stays_for_every_turn() {
    let hooks = json!({"SessionStart": [{"hooks": [{"type": "command",
        "command": "(cat; echo) >> started.jsonl; echo SESSION_CONTEXT_MARK"}]}]});
    let app = crate::app(hooks, &[], |_| reply("Done.")).await;
    run(&app, "first").await;
    run(&app, "second").await;
    run(&app, "third").await;
    let started = lines(&app, "started.jsonl");
    assert_eq!(started.len(), 1, "{started:?}");
    assert_eq!(started[0]["hook_event_name"], "SessionStart");
    assert_eq!(started[0]["source"], "startup");
    assert_eq!(started[0]["session_id"], app.thread.as_str());
    assert_eq!(started[0]["model"], "test-model");
    assert!(started[0].get("prompt").is_none(), "{:?}", started[0]);
    let all = bodies(&app.llm).await;
    assert_eq!(all.len(), 3);
    for body in &all {
        assert_eq!(
            body.to_string().matches("SESSION_CONTEXT_MARK").count(),
            1,
            "{body}"
        );
    }
    app.serving.abort();
}

/// A hook turned on partway through a chat starts on the next turn, and only that hook does.
#[tokio::test(flavor = "multi_thread")]
async fn a_hook_turned_on_in_a_running_session_starts_on_the_next_turn() {
    let hooks = json!({"SessionStart": [
        {"hooks": [{"type": "command", "command": "(cat; echo) >> one.jsonl"}]},
        {"hooks": [{"type": "command", "command": "(cat; echo) >> two.jsonl"}]},
    ]});
    let app = crate::app(hooks, &[], |_| reply("Done.")).await;
    let two = hook_named(&app, "two.jsonl");
    themis(app.data.path(), &["hook", "disable", "guarded", &two]);
    run(&app, "first").await;
    assert_eq!(lines(&app, "one.jsonl").len(), 1);
    assert!(lines(&app, "two.jsonl").is_empty());
    themis(app.data.path(), &["hook", "enable", "guarded", &two]);
    run(&app, "second").await;
    assert_eq!(lines(&app, "one.jsonl").len(), 1, "one started already");
    assert_eq!(lines(&app, "two.jsonl").len(), 1);
    assert_eq!(lines(&app, "two.jsonl")[0]["source"], "startup");
    app.serving.abort();
}

/// `SessionEnd` runs when the chat is deleted, once, with the transcript still there, and never
/// at the end of a turn.
#[tokio::test(flavor = "multi_thread")]
async fn deleting_a_chat_ends_its_session_once_and_a_turn_never_does() {
    const ENDS: &str = r#"#!/bin/sh
input=$(cat)
printf '%s\n' "$input" >> ended.jsonl
path=$(printf '%s' "$input" | sed 's/.*"transcript_path":"\([^"]*\)".*/\1/')
grep -c first "$path" > transcript-had.txt
"#;
    let hooks = json!({"SessionEnd": [{"hooks": [{"type": "command",
        "command": "sh \"${CLAUDE_PLUGIN_ROOT}/hooks/ends.sh\""}]}]});
    let app = crate::app(hooks, &[("hooks/ends.sh", ENDS)], |_| reply("Done.")).await;
    run(&app, "first").await;
    run(&app, "second").await;
    assert!(
        lines(&app, "ended.jsonl").is_empty(),
        "a turn isn't a session"
    );
    let (data, thread) = (app.data.path().to_owned(), app.thread.clone());
    tokio::task::spawn_blocking(move || {
        call(&data, "discard_thread", json!({"threadId": thread}));
    })
    .await
    .unwrap();
    until("SessionEnd never ran", || {
        exists(&app, "transcript-had.txt")
    });
    let ended = lines(&app, "ended.jsonl");
    assert_eq!(ended.len(), 1);
    assert_eq!(ended[0]["hook_event_name"], "SessionEnd");
    assert_eq!(ended[0]["reason"], "other");
    assert_eq!(ended[0]["themis_reason"], "chat_deleted");
    assert_eq!(ended[0]["session_id"], app.thread.as_str());
    assert!(ended[0].get("prompt").is_none());
    assert_ne!(
        std::fs::read_to_string(app.project.path().join("transcript-had.txt"))
            .unwrap()
            .trim(),
        "0"
    );
    let dir = app.data.path().join("hook-sessions").join(&app.thread);
    until("the chat's hook folder stayed", || !dir.exists());
    app.serving.abort();
}

/// Quitting ends every open session within a short budget: a slow hook is cut off rather than
/// holding the app.
#[tokio::test(flavor = "multi_thread")]
async fn quitting_ends_open_sessions_within_a_short_budget() {
    let hooks = json!({"SessionEnd": [
        {"hooks": [{"type": "command", "command": logging("ended.jsonl")}]},
        {"hooks": [{"type": "command", "command": "sleep 58; touch slow-finished", "timeout": 60}]},
    ]});
    let app = crate::app(hooks, &[], |_| reply("Done.")).await;
    run(&app, "first").await;
    let started = Instant::now();
    let data = app.data.path().to_owned();
    tokio::task::spawn_blocking(move || call(&data, "shutdown", json!({})))
        .await
        .unwrap();
    app.serving.await.unwrap();
    assert!(
        started.elapsed() < Duration::from_secs(20),
        "{:?}",
        started.elapsed()
    );
    let ended = lines_in(app.project.path(), "ended.jsonl");
    assert_eq!(ended.len(), 1, "{ended:?}");
    assert_eq!(ended[0]["reason"], "prompt_input_exit");
    assert_eq!(ended[0]["themis_reason"], "app_quit");
    assert!(!app.project.path().join("slow-finished").exists());
}

/// A chat left alone ends its session after `hooks.session_idle_minutes`; writing again starts
/// the next one, as a resume. With 0 it never ends for being idle.
#[tokio::test(flavor = "multi_thread")]
async fn an_idle_chat_ends_its_session_and_the_next_turn_resumes() {
    let hooks = json!({
        "SessionStart": [{"hooks": [{"type": "command", "command": logging("started.jsonl")}]}],
        "SessionEnd": [{"hooks": [{"type": "command", "command": logging("ended.jsonl")}]}],
    });
    let app = crate::app(hooks, &[], |_| reply("Done.")).await;
    std::fs::create_dir_all(app.project.path().join(".themis")).unwrap();
    std::fs::write(
        app.project.path().join(".themis/config.jsonc"),
        r#"{"hooks": {"session_idle_minutes": 5}}"#,
    )
    .unwrap();
    run(&app, "first").await;
    let soon = Instant::now() + Duration::from_secs(60);
    app.state.end_idle_hook_sessions_at(soon).await;
    assert!(
        lines(&app, "ended.jsonl").is_empty(),
        "not idle long enough"
    );
    let later = Instant::now() + Duration::from_secs(10 * 60);
    app.state.end_idle_hook_sessions_at(later).await;
    assert_eq!(lines(&app, "ended.jsonl").len(), 1);
    app.state.end_idle_hook_sessions_at(later).await;
    let ended = lines(&app, "ended.jsonl");
    assert_eq!(ended.len(), 1, "once a session");
    assert_eq!(ended[0]["reason"], "other");
    assert_eq!(ended[0]["themis_reason"], "idle");
    run(&app, "second").await;
    let started = lines(&app, "started.jsonl");
    assert_eq!(started.len(), 2, "{started:?}");
    assert_eq!(started[1]["source"], "resume");
    assert_eq!(started[1]["session_id"], started[0]["session_id"]);
    std::fs::write(
        app.project.path().join(".themis/config.jsonc"),
        r#"{"hooks": {"session_idle_minutes": 0}}"#,
    )
    .unwrap();
    run(&app, "third").await;
    let much_later = Instant::now() + Duration::from_secs(100 * 60 * 60);
    app.state.end_idle_hook_sessions_at(much_later).await;
    assert_eq!(lines(&app, "ended.jsonl").len(), 1, "0 means never");
    app.serving.abort();
}

/// With nobody there to ask, a session's end runs only what was allowed when it opened: a deny
/// rule stops it, in YOLO too, and so does turning it off afterwards.
#[tokio::test(flavor = "multi_thread")]
async fn a_session_end_hook_keeps_to_deny_rules_and_its_switch() {
    let hooks = logs("SessionEnd", "ended.jsonl");
    let later = Instant::now() + Duration::from_secs(100 * 60 * 60);
    let app = crate::app(hooks.clone(), &[], |_| reply("Done.")).await;
    run(&app, "first").await;
    let name = hook_named(&app, "ended.jsonl");
    themis(app.data.path(), &["hook", "disable", "guarded", &name]);
    app.state.end_idle_hook_sessions_at(later).await;
    assert!(lines(&app, "ended.jsonl").is_empty(), "turned off since");
    app.serving.abort();

    let app = crate::app(hooks, &[], |_| reply("Done.")).await;
    std::fs::create_dir_all(app.project.path().join(".themis")).unwrap();
    std::fs::write(
        app.project.path().join(".themis/config.jsonc"),
        r#"{"approval":{"rules":[{"tool":"hook_*","action":"deny"}]}}"#,
    )
    .unwrap();
    run(&app, "first").await;
    app.state.end_idle_hook_sessions_at(later).await;
    assert!(lines(&app, "ended.jsonl").is_empty(), "a deny rule holds");
    app.serving.abort();
}

/// In Custom a session's end is asked about once, on the turn that opens the session, so that it
/// can run later with nobody there. A person's no holds, and it isn't asked again.
#[tokio::test(flavor = "multi_thread")]
async fn in_custom_a_session_end_is_asked_about_when_the_session_opens() {
    let later = Instant::now() + Duration::from_secs(100 * 60 * 60);
    for (answer, runs) in [("once", 1), ("deny", 0)] {
        let hooks = logs("SessionEnd", "ended.jsonl");
        let app = crate::app(hooks, &[], |_| reply("Done.")).await;
        let (data, thread) = (app.data.path().to_owned(), app.thread.clone());
        let summaries = tokio::task::spawn_blocking(move || {
            call(
                &data,
                "set_thread_approval_mode",
                json!({"threadId": thread, "mode": "custom"}),
            );
            let mut summaries = Vec::new();
            for text in ["first", "second"] {
                call(
                    &data,
                    "send_message",
                    json!({"threadId": thread, "text": text}),
                );
                let deadline = Instant::now() + Duration::from_secs(30);
                while call(&data, "get_thread", json!({"threadId": thread}))["running"] == true {
                    assert!(Instant::now() < deadline, "the run never finished");
                    let pending = call(&data, "get_pending_approvals", json!({}));
                    for approval in pending.as_array().into_iter().flatten() {
                        summaries.push(approval["summary"].as_str().unwrap_or_default().to_owned());
                        call(
                            &data,
                            "approve_action",
                            json!({"threadId": thread,
                                "approvalId": approval["approval_id"], "decision": answer}),
                        );
                    }
                    std::thread::sleep(Duration::from_millis(20));
                }
            }
            summaries
        })
        .await
        .unwrap();
        assert_eq!(summaries.len(), 1, "{summaries:?}");
        assert!(summaries[0].starts_with("SessionEnd:"), "{summaries:?}");
        app.state.end_idle_hook_sessions_at(later).await;
        assert_eq!(lines(&app, "ended.jsonl").len(), runs, "{answer}");
        app.serving.abort();
    }
}

/// Some hooks hand their work to a detached process and exit at once, as Claude's short budget
/// makes them. That process must live on.
#[tokio::test(flavor = "multi_thread")]
async fn a_process_a_hook_detaches_outlives_the_hook() {
    let hooks = json!({"PostToolUse": [{"hooks": [{"type": "command",
        "command": "nohup sh -c 'sleep 1; touch detached-finished' >/dev/null 2>&1 </dev/null &"}]}]});
    let app = crate::app(hooks, &[], calls_once("list_dir", json!({"path": "."}))).await;
    run(&app, "go").await;
    until("the detached process was killed with its hook", || {
        exists(&app, "detached-finished")
    });
    app.serving.abort();
}

/// `/skill` in the person's message is expanded with hooks told which, on what they wrote; what
/// they add reaches the model, and a matcher takes the skill by name.
#[tokio::test(flavor = "multi_thread")]
async fn a_written_skill_runs_the_expansion_hooks() {
    let hooks = json!({"UserPromptExpansion": [
        {"matcher": "^guarded:review$", "hooks": [{"type": "command",
            "command": "(cat; echo) >> expanded.jsonl; echo EXPANSION_MARK"}]},
        {"matcher": "other", "hooks": [{"type": "command", "command": "touch wrong-skill"}]},
    ]});
    let app = crate::app(hooks, &[("skills/review/SKILL.md", SKILL_FILE)], |_| {
        reply("Done.")
    })
    .await;
    run(&app, "plain message").await;
    assert!(lines(&app, "expanded.jsonl").is_empty(), "nothing expanded");
    run(&app, "[[skill:g--guarded--review]] the parser").await;
    let seen = lines(&app, "expanded.jsonl");
    assert_eq!(seen.len(), 1, "{seen:?}");
    assert_eq!(seen[0]["hook_event_name"], "UserPromptExpansion");
    assert_eq!(seen[0]["expansion_type"], "slash_command");
    assert_eq!(seen[0]["command_name"], "guarded:review");
    assert_eq!(seen[0]["command_args"], "the parser");
    assert_eq!(seen[0]["command_source"], "plugin");
    assert!(!exists(&app, "wrong-skill"));
    assert!(bodies(&app.llm).await[1]
        .to_string()
        .contains("EXPANSION_MARK"));
    app.serving.abort();
    let hooks = json!({"UserPromptExpansion": [{"hooks": [{"type": "command",
        "command": "echo 'that command is off' >&2; exit 2"}]}]});
    let app = crate::app(hooks, &[("skills/review/SKILL.md", SKILL_FILE)], |_| {
        reply("Done.")
    })
    .await;
    run(&app, "[[skill:g--guarded--review]] please").await;
    assert!(
        bodies(&app.llm).await.is_empty(),
        "a refused expansion never reaches the model"
    );
    app.serving.abort();
}

/// After a step's tool calls and before the next request, hooks hear about the whole batch once;
/// what they add reaches that next request, labelled as the plugin's.
#[tokio::test(flavor = "multi_thread")]
async fn a_batch_of_tool_calls_runs_its_hook_once_and_adds_context() {
    let hooks = json!({"PostToolBatch": [{"hooks": [{"type": "command", "command":
        "(cat; echo) >> batch.jsonl; echo '{\"hookSpecificOutput\":{\"additionalContext\":\"BATCH_MARK\"}}'"}]}]});
    let app = crate::app(hooks, &[], |body| {
        if tool_results(body).is_empty() {
            ResponseTemplate::new(200).set_body_json(json!({"choices": [{"message": {
                "role": "assistant", "content": "", "tool_calls": [
                    {"id": "a", "type": "function", "function": {"name": "list_dir",
                        "arguments": "{\"path\":\".\"}"}},
                    {"id": "b", "type": "function", "function": {"name": "search_file",
                        "arguments": "{\"pattern\":\"x\",\"path\":\".\"}"}},
                ]}, "finish_reason": "tool_calls"}]}))
        } else {
            reply("Done.")
        }
    })
    .await;
    run(&app, "go").await;
    let batches = lines(&app, "batch.jsonl");
    assert_eq!(batches.len(), 1, "{batches:?}");
    assert_eq!(batches[0]["hook_event_name"], "PostToolBatch");
    let calls = batches[0]["tool_calls"].as_array().unwrap();
    assert_eq!(calls.len(), 2);
    assert_eq!(calls[0]["tool_name"], "LS");
    assert_eq!(calls[1]["tool_name"], "Grep");
    assert!(calls[0].get("tool_response").is_some());
    let all = bodies(&app.llm).await;
    assert!(!all[0].to_string().contains("BATCH_MARK"));
    assert!(all[1].to_string().contains("BATCH_MARK"));
    app.serving.abort();
    let hooks = json!({"PostToolBatch": [{"hooks": [{"type": "command",
        "command": "echo 'enough of that' >&2; exit 2"}]}]});
    let app = crate::app(hooks, &[], calls_once("list_dir", json!({"path": "."}))).await;
    run(&app, "go").await;
    assert_eq!(
        bodies(&app.llm).await.len(),
        1,
        "a refused batch stops before the next request"
    );
    app.serving.abort();
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

/// A hook runs when a question is about to be asked. Deny is honoured with no question asked;
/// allow is ignored, because hooks never grant permissions: the person is still asked.
#[tokio::test(flavor = "multi_thread")]
async fn a_permission_hook_may_deny_and_its_allow_changes_nothing() {
    const DENIES: &str = r#"(cat; echo) >> asked.jsonl; echo '{"hookSpecificOutput":{"hookEventName":"PermissionRequest","decision":{"behavior":"deny","message":"no shells today"}}}'"#;
    let touch = json!({"command": "touch", "args": ["ran.txt"]});
    let hooks = json!({
        "PermissionRequest": [{"matcher": "Bash", "hooks": [{"type": "command", "command": DENIES}]}],
        "PermissionDenied": [{"matcher": "Bash", "hooks": [{"type": "command", "command": logging("denied.jsonl")}]}],
    });
    let app = crate::app(hooks, &[], calls_once("shell", touch.clone())).await;
    assert_eq!(
        run_custom_until_asked(&app, "go").await,
        None,
        "nobody asked"
    );
    until("the run never ended", || {
        !lines(&app, "denied.jsonl").is_empty()
    });
    let (data, thread) = (app.data.path().to_owned(), app.thread.clone());
    tokio::task::spawn_blocking(move || {
        until("the run never finished", || {
            call(&data, "get_thread", json!({"threadId": thread}))["running"] != true
        });
    })
    .await
    .unwrap();
    let asked = lines(&app, "asked.jsonl");
    assert_eq!(asked.len(), 1);
    assert_eq!(asked[0]["hook_event_name"], "PermissionRequest");
    assert_eq!(asked[0]["tool_name"], "Bash");
    assert_eq!(asked[0]["tool_input"]["command"], "touch ran.txt");
    assert_eq!(lines(&app, "denied.jsonl")[0]["reason"], "no shells today");
    assert!(!exists(&app, "ran.txt"));
    assert!(
        last_tool_result(&app).await.contains("no shells today"),
        "the agent hears why"
    );
    app.serving.abort();

    // An answer of allow, even asking to change the command: nothing runs until the person says.
    const ALLOWS: &str = r#"echo '{"hookSpecificOutput":{"decision":{"behavior":"allow","updatedInput":{"command":"touch hook-chose.txt"}}}}'"#;
    let hooks = json!({
        "PermissionRequest": [{"hooks": [{"type": "command", "command": ALLOWS}]}],
        "PermissionDenied": [{"hooks": [{"type": "command", "command": logging("denied.jsonl")}]}],
        "Notification": [{"matcher": "permission_prompt", "hooks": [{"type": "command", "command": logging("told.jsonl")}]}],
    });
    let app = crate::app(hooks, &[], calls_once("shell", touch)).await;
    let id = run_custom_until_asked(&app, "go")
        .await
        .expect("the person wasn't asked");
    assert!(!exists(&app, "ran.txt"));
    assert!(!exists(&app, "hook-chose.txt"));
    let told = lines(&app, "told.jsonl");
    assert_eq!(told.len(), 1, "{told:?}");
    assert_eq!(told[0]["notification_type"], "permission_prompt");
    let (data, thread) = (app.data.path().to_owned(), app.thread.clone());
    tokio::task::spawn_blocking(move || {
        call(
            &data,
            "approve_action",
            json!({"threadId": thread, "approvalId": id, "decision": "deny"}),
        );
        until("the run never finished", || {
            call(&data, "get_thread", json!({"threadId": thread}))["running"] != true
        });
    })
    .await
    .unwrap();
    assert!(!exists(&app, "ran.txt"));
    assert!(!exists(&app, "hook-chose.txt"));
    let denied = lines(&app, "denied.jsonl");
    assert_eq!(denied.len(), 1, "the person's no: {denied:?}");
    assert_eq!(denied[0]["tool_name"], "Bash");
    app.serving.abort();
}

/// Between turns of a session, a configuration file that changed is reported on the turn that
/// picks it up; instructions count as loaded once a session.
#[tokio::test(flavor = "multi_thread")]
async fn a_changed_configuration_and_loaded_instructions_reach_their_hooks() {
    let hooks = json!({
        "ConfigChange": [{"matcher": "project_settings", "hooks": [{"type": "command", "command": logging("changed.jsonl")}]}],
        "InstructionsLoaded": [{"hooks": [{"type": "command", "command": logging("loaded.jsonl")}]}],
    });
    let app = crate::app(hooks, &[], |_| reply("Done.")).await;
    std::fs::write(app.project.path().join("RULES.md"), "Keep it short.").unwrap();
    std::fs::create_dir_all(app.project.path().join(".themis")).unwrap();
    let config = app.project.path().join(".themis/config.jsonc");
    std::fs::write(&config, r#"{"instructions": ["RULES.md"]}"#).unwrap();
    run(&app, "first").await;
    run(&app, "second").await;
    assert!(lines(&app, "changed.jsonl").is_empty(), "nothing changed");
    let loaded = lines(&app, "loaded.jsonl");
    assert_eq!(loaded.len(), 1, "{loaded:?}");
    assert_eq!(loaded[0]["load_reason"], "session_start");
    assert!(loaded[0]["file_path"]
        .as_str()
        .unwrap()
        .ends_with("RULES.md"));
    std::fs::write(
        &config,
        r#"{"instructions": ["RULES.md"], "hooks": {"max_wakes": 1}}"#,
    )
    .unwrap();
    run(&app, "third").await;
    let changed = lines(&app, "changed.jsonl");
    assert_eq!(changed.len(), 1, "{changed:?}");
    assert_eq!(changed[0]["source"], "project_settings");
    assert!(changed[0]["file_path"]
        .as_str()
        .unwrap()
        .ends_with("config.jsonc"));
    run(&app, "fourth").await;
    assert_eq!(lines(&app, "changed.jsonl").len(), 1, "once for one change");
    app.serving.abort();
}

/// A subagent's plan is Themis's task list: a new item is `TaskCreated`, one newly done is
/// `TaskCompleted`, and a hook that refuses tells the subagent why and leaves the plan alone.
#[tokio::test(flavor = "multi_thread")]
async fn a_subagents_plan_runs_the_task_hooks() {
    const REFUSES: &str = r#"#!/bin/sh
input=$(cat)
printf '%s\n' "$input" >> completed.jsonl
case "$input" in *'Refuse this'*) echo 'tests are not green' >&2; exit 2 ;; esac
"#;
    let hooks = json!({
        "TaskCreated": [{"hooks": [{"type": "command", "command": logging("created.jsonl")}]}],
        "TaskCompleted": [{"hooks": [{"type": "command",
            "command": "sh \"${CLAUDE_PLUGIN_ROOT}/hooks/refuses.sh\""}]}],
    });
    let plan = |status_read: &str, status_write: &str| {
        json!({"todos": [
            {"content": "Read the code", "status": status_read},
            {"content": "Refuse this", "status": status_write},
        ]})
    };
    let (first, second, third) = (
        plan("in_progress", "pending"),
        plan("completed", "pending"),
        plan("completed", "completed"),
    );
    let app = crate::app(
        hooks,
        &[
            ("hooks/refuses.sh", REFUSES),
            (
                "agents/planner.md",
                "---\nname: planner\ndescription: Plans\ntools: Read, TodoWrite\n---\nPlan it.",
            ),
        ],
        move |body| {
            let child = body["messages"][0]["content"]
                .as_str()
                .unwrap_or_default()
                .starts_with("You are a subagent");
            let done = tool_results(body).len();
            if child {
                return match done {
                    0 => tool_call("t0", "todo_write", &first),
                    1 => tool_call("t1", "todo_write", &second),
                    2 => tool_call("t2", "todo_write", &third),
                    _ => reply("Reported."),
                };
            }
            match done {
                0 => tool_call(
                    "s1",
                    "spawn_subagents",
                    &json!({"say": "Starting.", "subagents": [{"brief": "b", "profile": "planner"}]}),
                ),
                _ => reply("Done."),
            }
        },
    )
    .await;
    run(&app, "go").await;
    until("the refusal never came", || {
        lines(&app, "completed.jsonl").len() >= 2
    });
    let created = lines(&app, "created.jsonl");
    assert_eq!(created.len(), 2, "{created:?}");
    assert_eq!(created[0]["hook_event_name"], "TaskCreated");
    assert_eq!(created[0]["task_subject"], "Read the code");
    assert_eq!(created[0]["teammate_name"], created[0]["agent"]["name"]);
    let completed = lines(&app, "completed.jsonl");
    assert_eq!(completed.len(), 2, "{completed:?}");
    assert_eq!(completed[0]["task_subject"], "Read the code");
    assert_eq!(completed[1]["task_subject"], "Refuse this");
    let refusal = bodies(&app.llm)
        .await
        .iter()
        .flat_map(tool_results)
        .any(|result| result.contains("tests are not green"));
    assert!(refusal, "the subagent never heard why");
    app.serving.abort();
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

/// A prompt hook asks the chat's model, with no tools, what the plugin wants checked. A guard's no
/// refuses the call and says why; its plugin's text and the input reach only that request.
#[tokio::test(flavor = "multi_thread")]
async fn a_prompt_hook_asks_the_model_and_its_no_refuses_the_call() {
    let hooks = prompt_hook("PreToolUse", "Write", "Refuse writes of $ARGUMENTS please");
    let app = crate::app(hooks, &[], |body| {
        if is_prompt_hook(body) {
            return reply(r#"{"ok": false, "reason": "not in this folder"}"#);
        }
        if tool_results(body).is_empty() {
            tool_call("c1", "write_file", &writes())
        } else {
            reply("Done.")
        }
    })
    .await;
    run(&app, "write it").await;
    assert!(!exists(&app, "new.txt"), "the guard said no");
    assert!(last_tool_result(&app).await.contains("not in this folder"));
    let all = bodies(&app.llm).await;
    let asked = all.iter().find(|body| is_prompt_hook(body)).unwrap();
    assert_eq!(asked["model"], "test-model", "the chat's model");
    assert!(
        asked.get("tools").is_none_or(Value::is_null),
        "read-only: {asked}"
    );
    let text = asked.to_string();
    assert!(text.contains("Refuse writes of"), "{text}");
    assert!(
        text.contains("THE_FILE_TEXT"),
        "$ARGUMENTS is the input: {text}"
    );
    assert!(
        !text.contains("write it"),
        "not the person's message: {text}"
    );
    assert!(!all
        .iter()
        .filter(|body| !is_prompt_hook(body))
        .any(|body| body.to_string().contains("Refuse writes of")));
    app.serving.abort();
}

/// A guard that can't get a clear yes holds: an answer that can't be read, a request that fails, or
/// a budget that's gone all refuse the call.
#[tokio::test(flavor = "multi_thread")]
async fn a_prompt_guard_fails_closed() {
    for (answer, status) in [("I am not sure about this one.", 200), ("{}", 400)] {
        let hooks = prompt_hook("PreToolUse", "Write", "Check it");
        let app = crate::app(hooks, &[], move |body| {
            if is_prompt_hook(body) {
                return if status == 200 {
                    reply(answer)
                } else {
                    ResponseTemplate::new(status).set_body_string(answer)
                };
            }
            if tool_results(body).is_empty() {
                tool_call("c1", "write_file", &writes())
            } else {
                reply("Done.")
            }
        })
        .await;
        run(&app, "write it").await;
        assert!(!exists(&app, "new.txt"), "{answer}");
        app.serving.abort();
    }
    // One call a run: the first write is checked and passes, the second finds nothing left.
    let hooks = prompt_hook("PreToolUse", "Write", "Check it");
    let app = crate::app(hooks, &[], |body| {
        if is_prompt_hook(body) {
            return reply(r#"{"ok": true}"#);
        }
        match tool_results(body).len() {
            0 => tool_call("c1", "write_file", &writes()),
            1 => tool_call(
                "c2",
                "write_file",
                &json!({"file_path": "second.txt", "content": "x"}),
            ),
            _ => reply("Done."),
        }
    })
    .await;
    std::fs::write(
        app.data.path().join("runtime.jsonc"),
        r#"{"hooks": {"prompt_hook_calls": 1}}"#,
    )
    .unwrap();
    run(&app, "write both").await;
    assert!(exists(&app, "new.txt"), "a yes lets it through");
    assert!(!exists(&app, "second.txt"), "no budget, no write");
    assert_eq!(
        bodies(&app.llm)
            .await
            .iter()
            .filter(|body| is_prompt_hook(body))
            .count(),
        1
    );
    app.serving.abort();
}

/// `hooks.prompt_hook_calls: 0` is the person turning these off: nothing is asked and nothing is
/// refused for it. A project can't do that to them.
#[tokio::test(flavor = "multi_thread")]
async fn prompt_hooks_can_be_turned_off_by_the_person_only() {
    let hooks = prompt_hook("PreToolUse", "Write", "Check it");
    let app = crate::app(hooks.clone(), &[], |body| {
        if is_prompt_hook(body) {
            return reply(r#"{"ok": false, "reason": "no"}"#);
        }
        if tool_results(body).is_empty() {
            tool_call("c1", "write_file", &writes())
        } else {
            reply("Done.")
        }
    })
    .await;
    std::fs::write(
        app.data.path().join("runtime.jsonc"),
        r#"{"hooks": {"prompt_hook_calls": 0}}"#,
    )
    .unwrap();
    run(&app, "write it").await;
    assert!(exists(&app, "new.txt"));
    assert!(!bodies(&app.llm).await.iter().any(is_prompt_hook));
    app.serving.abort();

    let app = crate::app(hooks, &[], |_| reply("Done.")).await;
    std::fs::create_dir_all(app.project.path().join(".themis")).unwrap();
    std::fs::write(
        app.project.path().join(".themis/config.jsonc"),
        r#"{"hooks": {"prompt_hook_calls": 0}}"#,
    )
    .unwrap();
    let (data, thread) = (app.data.path().to_owned(), app.thread.clone());
    let refused = tokio::task::spawn_blocking(move || {
        std::process::Command::new(env!("CARGO_BIN_EXE_themis"))
            .arg("--data-dir")
            .arg(&data)
            .args([
                "call",
                "send_message",
                &json!({"threadId": thread, "text": "hi"}).to_string(),
            ])
            .env_remove("OPENCODE_KEY")
            .output()
            .unwrap()
    })
    .await
    .unwrap();
    assert!(!refused.status.success());
    assert!(String::from_utf8_lossy(&refused.stderr).contains("hooks.prompt_hook_calls"));
    app.serving.abort();
}

/// After a call has run, a no can't undo it: the agent hears the plugin's reason, labelled, and
/// goes on. A yes says nothing.
#[tokio::test(flavor = "multi_thread")]
async fn a_prompt_hook_after_a_tool_tells_the_agent() {
    let hooks = prompt_hook("PostToolUse", "LS", "Was the listing as expected?");
    let app = crate::app(hooks, &[], |body| {
        if is_prompt_hook(body) {
            return reply(r#"{"ok": false, "reason": "Confirm the row count"}"#);
        }
        if tool_results(body).is_empty() {
            tool_call("c1", "list_dir", &json!({"path": "."}))
        } else {
            reply("Done.")
        }
    })
    .await;
    run(&app, "look").await;
    let all = bodies(&app.llm).await;
    let last = all
        .iter()
        .rev()
        .find(|body| !is_prompt_hook(body))
        .unwrap()
        .to_string();
    assert!(last.contains("Confirm the row count"), "{last}");
    assert!(last.contains("not from the person"), "{last}");
    app.serving.abort();
}

/// What the desktop notified about, for hooks that want to hear, whether or not a run is going.
#[tokio::test(flavor = "multi_thread")]
async fn themis_notifying_runs_the_notification_hooks() {
    let hooks = json!({"Notification": [{"matcher": "goal_reached", "hooks": [{"type": "command",
        "command": logging("told.jsonl")}]}]});
    let app = crate::app(hooks, &[], |_| reply("Done.")).await;
    run(&app, "first").await;
    let (data, thread) = (app.data.path().to_owned(), app.thread.clone());
    tokio::task::spawn_blocking(move || {
        for (kind, outcome) in [
            ("stuck", "sent"),
            ("reached", "denied"),
            ("reached", "sent"),
        ] {
            call(
                &data,
                "record_notification",
                json!({"record": {"thread_id": thread, "kind": kind, "outcome": outcome}}),
            );
        }
    })
    .await
    .unwrap();
    until("the hook never heard", || {
        !lines(&app, "told.jsonl").is_empty()
    });
    tokio::time::sleep(Duration::from_millis(200)).await;
    let told = lines(&app, "told.jsonl");
    assert_eq!(told.len(), 1, "{told:?}");
    assert_eq!(told[0]["notification_type"], "goal_reached");
    assert_eq!(told[0]["hook_event_name"], "Notification");
    app.serving.abort();
}

/// A chat continued after a restart is what a `resume` hook runs on, and only then.
#[tokio::test(flavor = "multi_thread")]
async fn a_resume_hook_runs_on_a_chats_first_turn_after_a_restart() {
    let hooks = json!({"SessionStart": [{"matcher": "resume", "hooks": [{"type": "command",
        "command": "cat >> resumed.jsonl; echo >> resumed.jsonl"}]}]});
    let app = crate::app(hooks, &[], |_| reply("Done.")).await;
    run(&app, "first").await;
    assert!(
        !exists(&app, "resumed.jsonl"),
        "a new chat isn't a resumed one"
    );
    run(&app, "second").await;
    assert!(!exists(&app, "resumed.jsonl"), "nor is its next turn");
    app.serving.abort();
    tokio::time::sleep(Duration::from_millis(200)).await;
    // The app starts again on the same data.
    let state = AppState::new_for_test(app.data.path().join("settings.json"));
    state
        .set_secret("go".into(), "synthetic-test-key".into())
        .await
        .unwrap();
    state.set_go_base_url_override(Some(app.llm.uri()));
    let server = Server::bind(state, app.data.path()).await.unwrap();
    let serving = tokio::spawn(async move {
        let _ = server.run().await;
    });
    run(&app, "third").await;
    let first = std::fs::read_to_string(app.project.path().join("resumed.jsonl")).unwrap();
    assert!(first.contains("\"source\":\"resume\""), "{first}");
    run(&app, "fourth").await;
    let later = std::fs::read_to_string(app.project.path().join("resumed.jsonl")).unwrap();
    assert_eq!(first, later, "it runs once");
    serving.abort();
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
                0 => tool_call("c", "list_dir", &json!({"path": "."})),
                _ => reply("Done."),
            }
        },
    )
    .await
}

/// The real `remember` plugin through a session: its `SessionEnd` hook exits at once and its
/// detached flush goes on to write memory, reading the transcript Themis keeps. It isn't in this
/// repository and its summarizer is `claude -p`, so this is opt-in. To keep it off any account, run
/// it with a stand-in `claude` first on `PATH` and a throwaway `HOME`:
///
/// ```sh
/// HOME=/tmp/empty PATH=/tmp/fake-claude:$PATH THEMIS_REMEMBER_PLUGIN=/path/to/remember \
///   cargo test -p themis-desktop --test plugin_hooks_cli_e2e -- --ignored the_real_remember --nocapture
/// ```
#[tokio::test(flavor = "multi_thread")]
#[ignore = "needs a copy of the remember plugin and a stand-in claude"]
async fn the_real_remember_flushes_when_a_session_ends() {
    let real = std::path::PathBuf::from(std::env::var_os("THEMIS_REMEMBER_PLUGIN").unwrap());
    let app = real_plugin(real).await;
    run(&app, "Discuss the parser design").await;
    run(&app, "And the lexer").await;
    let memory = app.project.path().join(".remember");
    let tree = |dir: &Path| {
        let output = std::process::Command::new("find")
            .arg(dir)
            .arg("-type")
            .arg("f")
            .output()
            .unwrap();
        String::from_utf8_lossy(&output.stdout).into_owned()
    };
    println!("before the end:\n{}", tree(&memory));
    let (data, thread) = (app.data.path().to_owned(), app.thread.clone());
    tokio::task::spawn_blocking(move || {
        call(&data, "discard_thread", json!({"threadId": thread}));
    })
    .await
    .unwrap();
    tokio::time::sleep(Duration::from_secs(25)).await;
    println!("after the end:\n{}", tree(&memory));
    let text = |name: &str| std::fs::read_to_string(memory.join(name)).unwrap_or_default();
    for name in ["logs/hook-errors.log", "now.md"] {
        println!("--- {name}\n{}", text(name));
    }
    let output = std::process::Command::new("sh")
        .arg("-c")
        .arg("tail -n 40 .remember/logs/*.log")
        .current_dir(app.project.path())
        .output()
        .unwrap();
    println!("{}", String::from_utf8_lossy(&output.stdout));
    let log = String::from_utf8_lossy(&output.stdout);
    assert!(
        log.contains("SessionEnd") || log.contains("session-end"),
        "{log}"
    );
    app.serving.abort();
}

/// The real `vercel` plugin through a session: its session state stays across turns and is cleaned
/// up once, when the session ends. Opt-in, as it isn't in this repository:
///
/// ```sh
/// HOME=/tmp/empty TMPDIR=/tmp/empty THEMIS_VERCEL_PLUGIN=/path/to/vercel cargo test -p themis-desktop \
///   --test plugin_hooks_cli_e2e -- --ignored the_real_vercel --nocapture
/// ```
#[tokio::test(flavor = "multi_thread")]
#[ignore = "needs a copy of the vercel plugin"]
async fn the_real_vercel_keeps_its_state_until_the_session_ends() {
    let real = std::path::PathBuf::from(std::env::var_os("THEMIS_VERCEL_PLUGIN").unwrap());
    let app = real_plugin(real).await;
    let temp = std::env::temp_dir();
    let prefix = format!("vercel-plugin-{}-", app.thread);
    // The kind of thing its hooks leave as they go, as they name it.
    let state = temp.join(format!("{prefix}seen-skills.txt"));
    run(&app, "Deploy my nextjs app").await;
    std::fs::write(&state, "nextjs").unwrap();
    run(&app, "And again").await;
    assert_eq!(
        std::fs::read_to_string(&state).unwrap(),
        "nextjs",
        "a turn doesn't reset the session"
    );
    let env_files: Vec<_> =
        std::fs::read_dir(app.data.path().join("hook-sessions").join(&app.thread))
            .unwrap()
            .map(|entry| entry.unwrap().file_name().to_string_lossy().into_owned())
            .collect();
    println!("session folder: {env_files:?}");
    let (data, thread) = (app.data.path().to_owned(), app.thread.clone());
    tokio::task::spawn_blocking(move || {
        call(&data, "discard_thread", json!({"threadId": thread}));
    })
    .await
    .unwrap();
    until("its session state stayed", || !state.exists());
    let all = bodies(&app.llm).await;
    println!(
        "first request's hook context: {}",
        all[0].to_string().contains("hook-context")
    );
    app.serving.abort();
}

/// The real `ralph-loop` stop hook, from a copy of the plugin. It isn't in this repository, so
/// this is opt-in:
///
/// ```sh
/// THEMIS_RALPH_PLUGIN=/path/to/plugins/ralph-loop cargo test -p themis-desktop \\
///   --test plugin_hooks_cli_e2e -- --ignored the_real_ralph_loop --nocapture
/// ```
#[tokio::test(flavor = "multi_thread")]
#[ignore = "needs a copy of the ralph-loop plugin"]
async fn the_real_ralph_loop_goes_round_and_ends_on_its_promise() {
    let real = std::path::PathBuf::from(std::env::var_os("THEMIS_RALPH_PLUGIN").unwrap());
    let script = std::fs::read_to_string(real.join("hooks/stop-hook.sh")).unwrap();
    let hooks: Value =
        serde_json::from_str(&std::fs::read_to_string(real.join("hooks/hooks.json")).unwrap())
            .unwrap();
    let app = crate::app(
        hooks["hooks"].clone(),
        &[("hooks/stop-hook.sh", &script)],
        |body| {
            let rounds = body.to_string().matches("Polish the notes").count();
            // Once in the state file's prompt, handed back each round.
            match rounds {
                0 | 1 => reply("Round one."),
                2 => reply("Round two."),
                _ => reply("All done. <promise>FINISHED</promise>"),
            }
        },
    )
    .await;
    let state = app.project.path().join(".claude/ralph-loop.local.md");
    std::fs::create_dir_all(state.parent().unwrap()).unwrap();
    std::fs::write(
        &state,
        "---\nactive: true\niteration: 1\nmax_iterations: 8\ncompletion_promise: \"FINISHED\"\nstarted_at: \"x\"\n---\n\nPolish the notes\n",
    )
    .unwrap();
    run(&app, "go").await;
    let all = bodies(&app.llm).await;
    assert!(all.len() >= 3, "it never looped: {}", all.len());
    assert!(!state.exists(), "the promise didn't end the loop");
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
