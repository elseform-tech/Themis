//! What a subagent can really do with the tools a plugin's profile names, through the real shared
//! server and the `themis` CLI with a scripted model: web tools under the same rules as the main
//! agent, and the rest of Claude's tool names. Unix only: the guards are shell scripts.
#![cfg(unix)]

use std::path::Path;
use std::time::{Duration, Instant};

use serde_json::{json, Value};
use themis_desktop::{server::Server, state::AppState, types::ProviderKind};
use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, Request, ResponseTemplate};

const SUBAGENT: &str = "You are a subagent";
const NOTE: &str = "A subagent you started has ended.";

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

fn results(body: &Value) -> Vec<&str> {
    messages(body)
        .iter()
        .filter(|m| m["role"] == "tool")
        .map(text)
        .collect()
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

struct App {
    llm: MockServer,
    data: tempfile::TempDir,
    project: tempfile::TempDir,
    _plugins: tempfile::TempDir,
    thread: String,
    serving: tokio::task::JoinHandle<()>,
}

#[derive(Default)]
struct Setup<'a> {
    /// `(name, frontmatter lines)` of the plugin's subagent profiles.
    agents: &'a [(&'a str, &'a str)],
    hooks: Option<Value>,
    /// The project's `.themis/config.jsonc`.
    config: Option<&'a str>,
    /// The user's `runtime.jsonc`.
    user_config: Option<&'a str>,
}

async fn app(
    setup: Setup<'_>,
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
    if let Some(config) = setup.config {
        std::fs::create_dir_all(project.path().join(".themis")).unwrap();
        std::fs::write(project.path().join(".themis/config.jsonc"), config).unwrap();
    }
    if let Some(config) = setup.user_config {
        std::fs::write(data.path().join("runtime.jsonc"), config).unwrap();
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
    let plugins = tempfile::tempdir().unwrap();
    let root = plugins.path().join("kit");
    std::fs::create_dir_all(root.join(".claude-plugin")).unwrap();
    std::fs::create_dir_all(root.join("agents")).unwrap();
    std::fs::write(
        root.join(".claude-plugin/plugin.json"),
        json!({"name": "kit", "description": "A test plugin"}).to_string(),
    )
    .unwrap();
    // Something to import even when a test names no profile.
    std::fs::write(
        root.join("agents/plain.md"),
        "---\nname: plain\ndescription: A role.\n---\nPlay the role.\n",
    )
    .unwrap();
    for (name, frontmatter) in setup.agents {
        std::fs::write(
            root.join(format!("agents/{name}.md")),
            format!(
                "---\nname: {name}\ndescription: A role.\n{frontmatter}\n---\nPlay the role.\n"
            ),
        )
        .unwrap();
    }
    if let Some(hooks) = &setup.hooks {
        std::fs::create_dir_all(root.join("hooks")).unwrap();
        std::fs::write(
            root.join("hooks/hooks.json"),
            json!({ "hooks": hooks }).to_string(),
        )
        .unwrap();
    }
    let directory = data.path().to_owned();
    tokio::task::spawn_blocking(move || {
        let plugin = themis(
            &directory,
            &[
                "plugin",
                "import",
                root.to_str().unwrap(),
                "--allow-partial",
            ],
        );
        for hook in plugin["spec"]["hooks"].as_array().into_iter().flatten() {
            themis(
                &directory,
                &["hook", "enable", "kit", hook["name"].as_str().unwrap()],
            );
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

/// Sends `task` in `mode` and waits until the run is over.
async fn run(app: &App, mode: &'static str, task: &'static str) {
    let (data, thread) = (app.data.path().to_owned(), app.thread.clone());
    tokio::task::spawn_blocking(move || {
        call(
            &data,
            "set_thread_approval_mode",
            json!({"threadId": thread, "mode": mode}),
        );
        call(
            &data,
            "send_message",
            json!({"threadId": thread, "text": task}),
        );
        let deadline = Instant::now() + Duration::from_secs(60);
        while call(&data, "get_thread", json!({"threadId": thread}))["running"] == true {
            assert!(Instant::now() < deadline, "the run never finished");
            std::thread::sleep(Duration::from_millis(50));
        }
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

/// The main agent starts one subagent as `profile`, if any, and `child` answers for it.
fn spawning(
    profile: Option<&'static str>,
    child: impl Fn(&Value) -> ResponseTemplate + Send + Sync + 'static,
) -> impl Fn(&Value) -> ResponseTemplate + Send + Sync + 'static {
    move |body| {
        if is_subagent(body) {
            return child(body);
        }
        if results(body).is_empty() {
            let mut one = json!({"name": "Helper", "brief": "Do the work."});
            if let Some(profile) = profile {
                one["profile"] = json!(profile);
            }
            return tool_call(
                "s1",
                "spawn_subagents",
                &json!({"say": "Starting.", "subagents": [one]}),
            );
        }
        if mentions(body, NOTE) {
            reply("Done.")
        } else {
            reply("Waiting.")
        }
    }
}

/// A subagent making these calls in order, then reporting.
fn calling(
    calls: Vec<(&'static str, Value)>,
) -> impl Fn(&Value) -> ResponseTemplate + Send + Sync + 'static {
    move |body| match calls.get(results(body).len()) {
        Some((tool, args)) => tool_call(&format!("c{}", results(body).len()), tool, args),
        None => reply("Reported."),
    }
}

/// What the subagent's tools answered, in order, from its last request.
async fn child_results(app: &App) -> Vec<String> {
    bodies(&app.llm)
        .await
        .iter()
        .rfind(|body| is_subagent(body))
        .map(|body| {
            results(body)
                .iter()
                .map(|text| (*text).to_owned())
                .collect()
        })
        .unwrap_or_default()
}

/// The tools the subagent was offered, and all of its system message.
async fn child_request(app: &App) -> (Vec<String>, String) {
    let all = bodies(&app.llm).await;
    let body = all
        .iter()
        .find(|body| is_subagent(body))
        .expect("no subagent request");
    (tool_names(body), text(&messages(body)[0]).to_owned())
}

async fn web_page() -> (MockServer, String) {
    let web = MockServer::start().await;
    Mock::given(path("/doc"))
        .respond_with(
            ResponseTemplate::new(200)
                .insert_header("content-type", "text/plain")
                .set_body_string("THE PAGE TEXT"),
        )
        .mount(&web)
        .await;
    let url = format!("{}/doc", web.uri());
    (web, url)
}

fn fetch(url: &str) -> (&'static str, Value) {
    ("fetch_url", json!({"url": url}))
}

#[tokio::test(flavor = "multi_thread")]
async fn a_subagent_reads_a_page_with_the_web_tools() {
    let (web, url) = web_page().await;
    let app = app(Setup::default(), spawning(None, calling(vec![fetch(&url)]))).await;
    run(&app, "yolo", "go").await;
    let (tools, system) = child_request(&app).await;
    assert!(tools.iter().any(|tool| tool == "fetch_url"), "{tools:?}");
    assert!(system.contains("fetch_url"), "it isn't told: {system}");
    assert!(child_results(&app).await[0].contains("THE PAGE TEXT"));
    assert_eq!(web.received_requests().await.unwrap().len(), 1);
    // The Tasks detail says so.
    let listed = call(
        app.data.path(),
        "list_subagents",
        json!({"threadId": app.thread}),
    );
    assert_eq!(listed[0]["web"], true, "{listed}");
    app.serving.abort();
}

#[tokio::test(flavor = "multi_thread")]
async fn a_deny_rule_holds_for_a_subagents_web_call_in_yolo() {
    let (web, url) = web_page().await;
    let app = app(
        Setup {
            config: Some(
                r#"{"approval":{"rules":[{"tool":"fetch_url","command":["127.0.0.1"],"action":"deny"}]}}"#,
            ),
            ..Setup::default()
        },
        spawning(None, calling(vec![fetch(&url)])),
    )
    .await;
    run(&app, "yolo", "go").await;
    let answered = &child_results(&app).await[0];
    assert!(!answered.contains("THE PAGE TEXT"), "{answered}");
    assert!(web.received_requests().await.unwrap().is_empty());
    app.serving.abort();
}

#[tokio::test(flavor = "multi_thread")]
async fn a_subagents_web_call_asks_under_its_name_in_custom() {
    let (web, url) = web_page().await;
    let app = app(Setup::default(), spawning(None, calling(vec![fetch(&url)]))).await;
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
            json!({"threadId": thread, "text": "go"}),
        );
        let deadline = Instant::now() + Duration::from_secs(30);
        loop {
            assert!(Instant::now() < deadline, "no approval was asked for");
            let pending = call(&data, "get_pending_approvals", json!({}));
            if let Some(found) = pending.as_array().and_then(|all| {
                all.iter().find(|approval| {
                    approval["thread_id"] == thread.as_str()
                        && approval.to_string().contains("fetch_url")
                })
            }) {
                assert_eq!(found["subagent"]["name"], "Helper", "{found}");
                break;
            }
            std::thread::sleep(Duration::from_millis(50));
        }
    })
    .await
    .unwrap();
    assert!(web.received_requests().await.unwrap().is_empty());
    app.serving.abort();
}

#[tokio::test(flavor = "multi_thread")]
async fn allow_web_off_keeps_web_tools_from_subagents_only() {
    let (web, url) = web_page().await;
    let app = app(
        Setup {
            config: Some(r#"{"subagents": {"allow_web": false}}"#),
            agents: &[("reader", "tools: Read, WebFetch")],
            ..Setup::default()
        },
        spawning(Some("reader"), calling(vec![fetch(&url)])),
    )
    .await;
    run(&app, "yolo", "go").await;
    let (tools, system) = child_request(&app).await;
    assert!(!tools.iter().any(|tool| tool == "fetch_url"), "{tools:?}");
    assert!(system.contains("aren't available"), "not told: {system}");
    assert!(web.received_requests().await.unwrap().is_empty());
    let main = bodies(&app.llm).await;
    let main = main.iter().find(|body| !is_subagent(body)).unwrap();
    assert!(tool_names(main).iter().any(|tool| tool == "fetch_url"));
    let listed = call(
        app.data.path(),
        "list_subagents",
        json!({"threadId": app.thread}),
    );
    assert_eq!(listed[0]["web"], false, "{listed}");
    app.serving.abort();
}

#[tokio::test(flavor = "multi_thread")]
async fn a_blocked_network_keeps_web_tools_from_subagents() {
    let (_web, url) = web_page().await;
    let app = app(
        Setup {
            config: Some(r#"{"sandbox": {"network": "block"}}"#),
            ..Setup::default()
        },
        spawning(None, calling(vec![fetch(&url)])),
    )
    .await;
    run(&app, "yolo", "go").await;
    let (tools, _) = child_request(&app).await;
    assert!(!tools.iter().any(|tool| tool == "fetch_url"), "{tools:?}");
    app.serving.abort();
}

#[tokio::test(flavor = "multi_thread")]
async fn an_explicit_tool_list_narrows_the_web_tools_too() {
    let (_web, url) = web_page().await;
    let hooks = json!({"PreToolUse": [{"matcher": "WebFetch", "hooks": [{
        "type": "command", "command": "cat > web-hook.json"
    }]}]});
    let app = app(
        Setup {
            agents: &[
                ("reader", "tools: Read"),
                ("fetcher", "tools: WebFetch"),
                ("denied", "disallowedTools: WebFetch"),
            ],
            hooks: Some(hooks),
            ..Setup::default()
        },
        spawning(Some("fetcher"), calling(vec![fetch(&url)])),
    )
    .await;
    run(&app, "yolo", "go").await;
    let (tools, _) = child_request(&app).await;
    // Only what it names: no shell, no files, no search.
    assert_eq!(tools, ["fetch_url"], "{tools:?}");
    // The plugin's hook fired for it, naming the subagent.
    let seen: Value =
        serde_json::from_slice(&std::fs::read(app.project.path().join("web-hook.json")).unwrap())
            .unwrap();
    assert_eq!(seen["tool_name"], "WebFetch");
    assert_eq!(seen["agent"]["name"], "Helper");
    app.serving.abort();
    for profile in ["reader", "denied"] {
        let (_web, url) = web_page().await;
        let agents: &[(&str, &str)] = &[
            ("reader", "tools: Read"),
            ("denied", "disallowedTools: WebFetch"),
        ];
        let app = crate::app(
            Setup {
                agents,
                ..Setup::default()
            },
            spawning(Some(profile), calling(vec![fetch(&url)])),
        )
        .await;
        run(&app, "yolo", "go").await;
        let (tools, _) = child_request(&app).await;
        assert!(
            !tools.iter().any(|tool| tool == "fetch_url"),
            "{profile}: {tools:?}"
        );
        app.serving.abort();
    }
}

/// A scoped `Bash(...)` gives the shell for those commands and refuses the rest, however they are
/// wrapped, in YOLO too.
#[tokio::test(flavor = "multi_thread")]
async fn a_scoped_shell_runs_only_the_commands_it_names() {
    let calls = vec![
        ("shell", json!({"command": "git", "args": ["status"]})),
        ("shell", json!({"command": "touch", "args": ["plain-ran"]})),
        (
            "shell",
            json!({"command": "sh", "args": ["-c", "git status && touch chained-ran"]}),
        ),
        (
            "shell",
            json!({"command": "sh", "args": ["-c", "touch wrapped-ran"]}),
        ),
        (
            "shell",
            json!({"command": "git", "args": ["-C", ".", "push"]}),
        ),
        (
            "shell",
            json!({"command": "env", "args": ["PATH=.", "git", "status"]}),
        ),
        (
            "shell",
            json!({"command": "sh", "args": ["-c", "PATH=. git status"]}),
        ),
        (
            "shell",
            json!({"command": "sh", "args": ["-c", "git status > redirected-ran"]}),
        ),
        (
            "shell",
            json!({"command": "touch", "args": ["background-ran"], "background": true}),
        ),
        (
            "shell",
            json!({"command": "sh", "args": ["-c", "$X status"]}),
        ),
        ("shell", json!({"command": "git", "args": ["statusx"]})),
    ];
    let count = calls.len();
    let app = app(
        Setup {
            agents: &[("scoped", "tools: Read, Bash(git status:*)")],
            ..Setup::default()
        },
        spawning(Some("scoped"), calling(calls)),
    )
    .await;
    run(&app, "yolo", "go").await;
    let (tools, system) = child_request(&app).await;
    assert!(tools.iter().any(|tool| tool == "shell"), "{tools:?}");
    // No other way to a command, and it knows its limit.
    for other in ["git", "background_task", "write_file"] {
        assert!(!tools.iter().any(|tool| tool == other), "{tools:?}");
    }
    assert!(system.contains("git status:*"), "{system}");
    let answered = child_results(&app).await;
    assert_eq!(answered.len(), count, "{answered:?}");
    assert!(
        answered[0].contains("\"exit_code\""),
        "the named command was refused: {}",
        answered[0]
    );
    for (index, answer) in answered.iter().enumerate().skip(1) {
        assert!(answer.contains("denied"), "call {index} ran: {answer}");
    }
    for ran in [
        "plain-ran",
        "chained-ran",
        "wrapped-ran",
        "redirected-ran",
        "background-ran",
    ] {
        assert!(!app.project.path().join(ran).exists(), "{ran}");
    }
    let listed = call(
        app.data.path(),
        "list_subagents",
        json!({"threadId": app.thread}),
    );
    assert_eq!(
        listed[0]["shell_scopes"],
        json!(["git status:*"]),
        "{listed}"
    );
    app.serving.abort();
}

/// With `BashOutput` and `KillShell` it can start, read and stop its own background program, and
/// the program ends with it. A scope still holds for what it starts.
#[tokio::test(flavor = "multi_thread")]
async fn a_subagent_uses_background_programs_and_they_end_with_it() {
    let child = |body: &Value| -> ResponseTemplate {
        let done = results(body);
        match done.len() {
            0 => tool_call(
                "b0",
                "shell",
                &json!({"command": "sh", "args": ["-c", "echo started-output; sleep 300"], "background": true}),
            ),
            1 => {
                let started: Value = serde_json::from_str(done[0]).unwrap_or(Value::Null);
                std::thread::sleep(Duration::from_millis(300));
                tool_call(
                    "b1",
                    "background_task",
                    &json!({"action": "log", "task_id": started["task_id"]}),
                )
            }
            2 => tool_call("b2", "background_task", &json!({"action": "list"})),
            _ => reply("Reported."),
        }
    };
    let app = app(
        Setup {
            agents: &[("runner", "tools: Bash, BashOutput, KillShell")],
            ..Setup::default()
        },
        spawning(Some("runner"), child),
    )
    .await;
    run(&app, "yolo", "go").await;
    let (tools, _) = child_request(&app).await;
    assert!(
        tools.iter().any(|tool| tool == "background_task"),
        "{tools:?}"
    );
    let answered = child_results(&app).await;
    assert!(answered[1].contains("started-output"), "{answered:?}");
    assert!(answered[2].contains("\"tasks\""), "{answered:?}");
    let data = app.data.path().to_owned();
    let thread = app.thread.clone();
    tokio::task::spawn_blocking(move || {
        let deadline = Instant::now() + Duration::from_secs(20);
        loop {
            let tasks = call(&data, "get_thread_tasks", json!({"threadId": thread}));
            if tasks["background_tasks"][0]["state"] == "stopped" {
                break;
            }
            assert!(Instant::now() < deadline, "still running: {tasks}");
            std::thread::sleep(Duration::from_millis(100));
        }
    })
    .await
    .unwrap();
    app.serving.abort();
}

#[tokio::test(flavor = "multi_thread")]
async fn a_plain_subagent_has_none_of_these_tools_unless_named() {
    let app = app(
        Setup::default(),
        spawning(None, |_: &Value| reply("Reported.")),
    )
    .await;
    run(&app, "yolo", "go").await;
    let (tools, _) = child_request(&app).await;
    for named_only in [
        "background_task",
        "todo_write",
        "ask_user",
        "spawn_subagents",
        "git",
    ] {
        assert!(!tools.iter().any(|tool| tool == named_only), "{tools:?}");
    }
    app.serving.abort();
}

#[tokio::test(flavor = "multi_thread")]
async fn a_todo_list_is_kept_and_shows_what_it_is_doing() {
    let todos = json!({"todos": [
        {"content": "Read the code", "status": "completed"},
        {"content": "Write the report", "status": "in_progress"}
    ]});
    let app = app(
        Setup {
            agents: &[("planner", "tools: Read, TodoWrite")],
            ..Setup::default()
        },
        spawning(Some("planner"), calling(vec![("todo_write", todos)])),
    )
    .await;
    run(&app, "custom", "go").await;
    let answered = child_results(&app).await;
    assert!(answered[0].contains("\"done\":1"), "{answered:?}");
    let listed = call(
        app.data.path(),
        "list_subagents",
        json!({"threadId": app.thread}),
    );
    assert_eq!(listed[0]["text"], "Write the report", "{listed}");
    app.serving.abort();
}

/// `AskUserQuestion` reaches the person through the chat, names who asks, and the answer goes to
/// the subagent that asked, which never stops to wait.
#[tokio::test(flavor = "multi_thread")]
async fn a_subagents_question_reaches_the_person_and_the_answer_reaches_it() {
    let child = |body: &Value| -> ResponseTemplate {
        if mentions(body, "Use the blue one") {
            return reply("Done with blue.");
        }
        if results(body).is_empty() {
            return tool_call(
                "q0",
                "ask_user",
                &json!({"question": "Which colour?", "blocking": true}),
            );
        }
        if results(body).len() == 1 {
            return tool_call(
                "q1",
                "ask_user",
                &json!({"question": "Which colour?", "recommended": "red", "why": "It is the house style"}),
            );
        }
        tool_call("w", "shell", &json!({"command": "sleep", "args": ["0.1"]}))
    };
    let app = app(
        Setup {
            agents: &[("asker", "tools: Bash, AskUserQuestion")],
            ..Setup::default()
        },
        spawning(Some("asker"), child),
    )
    .await;
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
            json!({"threadId": thread, "text": "go"}),
        );
        let deadline = Instant::now() + Duration::from_secs(30);
        let question = loop {
            assert!(Instant::now() < deadline, "the question never came");
            let open = call(&data, "list_questions", json!({"threadId": thread}));
            if let Some(found) = open.as_array().and_then(|all| all.first().cloned()) {
                break found;
            }
            std::thread::sleep(Duration::from_millis(50));
        };
        assert!(
            question["question"]
                .as_str()
                .unwrap()
                .starts_with("Helper: "),
            "{question}"
        );
        assert_eq!(question["blocking"], false, "{question}");
        call(
            &data,
            "answer_question",
            json!({"threadId": thread, "questionId": question["id"], "answer": "Use the blue one"}),
        );
        while call(&data, "get_thread", json!({"threadId": thread}))["running"] == true {
            assert!(Instant::now() < deadline, "the run never finished");
            std::thread::sleep(Duration::from_millis(50));
        }
    })
    .await
    .unwrap();
    let all = bodies(&app.llm).await;
    let child_saw = all
        .iter()
        .any(|body| is_subagent(body) && mentions(body, "Use the blue one"));
    assert!(child_saw, "the answer never reached the subagent");
    let first = all.iter().rfind(|body| is_subagent(body)).unwrap();
    assert!(
        results(first)[0].contains("can't stop"),
        "a blocking question from a subagent is refused: {:?}",
        results(first)
    );
    assert!(
        !all.iter()
            .any(|body| !is_subagent(body) && mentions(body, "Use the blue one")),
        "the answer went to the main agent as well"
    );
    let (_, system) = child_request(&app).await;
    assert!(system.contains("ask_user"), "{system}");
    app.serving.abort();
}

/// Patterns as catalog plugins write them, and every way around them a model could try. Each
/// form would create `escaped` if it ran. They are refused, not merely unsuccessful.
#[tokio::test(flavor = "multi_thread")]
async fn a_scoped_shell_holds_against_every_way_around_it() {
    let touch_py = "open('escaped','w')";
    let touch_js = "require('fs').writeFileSync('escaped','')";
    let shell = |args: Value| ("shell", args);
    let calls: Vec<(&'static str, Value)> = vec![
        // Interpreter options, bundled and long, beside a pattern with a loose script name.
        shell(json!({"command": "python3", "args": ["-c", touch_py]})),
        shell(json!({"command": "python3", "args": ["-Ic", touch_py]})),
        shell(json!({"command": "python3", "args": ["-Ic", touch_py, "tool.py"]})),
        shell(json!({"command": "python3", "args": ["-c", touch_py, "tool.py"]})),
        shell(json!({"command": "python3", "args": ["-", "tool.py"]})),
        shell(json!({"command": "python3", "args": ["-m", "http.server", "tool.py"]})),
        // Another script, with the named one further along.
        shell(json!({"command": "python3", "args": ["evil.py", "tool.py"]})),
        shell(json!({"command": "node", "args": ["-e", touch_js, "run.js"]})),
        shell(json!({"command": "node", "args": ["-pe", touch_js, "run.js"]})),
        shell(json!({"command": "node", "args": ["--eval=x", "run.js"]})),
        shell(json!({"command": "node", "args": ["--eval", touch_js, "run.js"]})),
        // A path, or a prefix, in front of it.
        shell(json!({"command": "/usr/bin/env", "args": ["python3", "-c", touch_py]})),
        shell(json!({"command": "env", "args": ["python3", "-c", touch_py]})),
        shell(json!({"command": "/usr/bin/python3", "args": ["-c", touch_py]})),
        shell(json!({"command": "sh", "args": ["-c", "python3 -c \"open('escaped','w')\""]})),
        shell(json!({"command": "sh", "args": ["-c", "touch escaped"]})),
        shell(json!({"command": "xargs", "args": ["touch", "escaped"]})),
        shell(
            json!({"command": "find", "args": [".", "-name", "x", "-exec", "touch", "escaped", ";"]}),
        ),
        shell(json!({"command": "find", "args": [".", "-name", "x", "-delete"]})),
        // Git's ways of running something else.
        shell(json!({"command": "git", "args": ["-c", "core.sshCommand=touch escaped", "status"]})),
        shell(json!({"command": "git", "args": ["-c", "alias.status=!touch escaped", "status"]})),
        shell(json!({"command": "git", "args": ["--exec-path=.", "status"]})),
        shell(json!({"command": "git", "args": ["--git-dir=x", "status"]})),
        shell(json!({"command": "git", "args": ["status", "--ext-diff"]})),
        shell(json!({"command": "git", "args": ["--config", "a=b", "status"]})),
        shell(json!({"command": "git", "args": ["-u", "status"]})),
        shell(json!({"command": "git", "args": ["status", "-u"]})),
        // Tar's bundles need no dash, and its options run programs.
        shell(json!({"command": "tar", "args": ["xI", "touch escaped", "-f", "x.tar"]})),
        shell(json!({"command": "tar", "args": ["-xI", "touch escaped", "-f", "x.tar"]})),
        shell(json!({"command": "tar", "args": ["tf", "x.tar", "--to-command=touch escaped"]})),
        // Not one simple command, though every part is named.
        shell(json!({"command": "sh", "args": ["-c", "git status && git status"]})),
        shell(json!({"command": "sh", "args": ["-c", "git status | cat"]})),
        // A setting is an option by another name, for make above all.
        shell(json!({"command": "make", "args": ["test", "SHELL=/tmp/evil"]})),
        shell(json!({"command": "make", "args": ["test", "CC=/tmp/evil"]})),
        // A wrapper, though the pattern names it.
        shell(json!({"command": "timeout", "args": ["5", "git", "status"]})),
        // Whole-program catch-alls aren't a limit, however a plugin writes them.
        shell(json!({"command": "npx", "args": ["cowsay"]})),
        shell(json!({"command": "npx", "args": ["-p", "evil", "tsc"]})),
        shell(json!({"command": "ruby", "args": ["-e", "File.write('escaped','')"]})),
        shell(json!({"command": "perl", "args": ["-e", "open(F,'>escaped')"]})),
        shell(json!({"command": "bash", "args": ["evil.sh"]})),
        shell(json!({"command": "awk", "args": ["BEGIN{system(\"touch escaped\")}"]})),
        shell(json!({"command": "make", "args": ["-f", "evil.mk"]})),
    ];
    let count = calls.len();
    let app = app(
        Setup {
            agents: &[(
                "scoped",
                "tools: Read, Bash(python3 *tool.py*), Bash(python3:*), Bash(node run.js:*), Bash(node:*), Bash(git status:*), Bash(git:*), Bash(find . -name *), Bash(find:*), Bash(npx tsc:*), Bash(npx:*), Bash(ruby:*), Bash(perl *), Bash(bash:*), Bash(awk *), Bash(make:*), Bash(env:*), Bash(xargs:*), Bash(tar *), Bash(tar tf *), Bash(timeout *), Bash(make test:*)",
            )],
            user_config: Some(r#"{"subagents": {"max_turns": 80}}"#),
            ..Setup::default()
        },
        spawning(Some("scoped"), calling(calls)),
    )
    .await;
    std::fs::write(app.project.path().join("evil.py"), "open('escaped','w')").unwrap();
    std::fs::write(app.project.path().join("evil.sh"), "touch escaped").unwrap();
    std::fs::write(
        app.project.path().join("evil.mk"),
        "all:\n\ttouch escaped\n",
    )
    .unwrap();
    run(&app, "yolo", "go").await;
    let answered = child_results(&app).await;
    assert_eq!(answered.len(), count, "{answered:?}");
    let ran: Vec<usize> = answered
        .iter()
        .enumerate()
        .filter(|(_, answer)| !answer.contains("denied"))
        .map(|(index, _)| index)
        .collect();
    assert!(ran.is_empty(), "these ran: {ran:?}: {answered:?}");
    assert!(!app.project.path().join("escaped").exists());
    app.serving.abort();
}

/// With no enforcing sandbox a subagent keeps its shell, as the design says, the scope still holds,
/// and its detail says the shell isn't confined.
#[tokio::test(flavor = "multi_thread")]
async fn with_no_sandbox_a_scope_still_holds_and_the_detail_says_so() {
    let calls = vec![
        ("shell", json!({"command": "git", "args": ["status"]})),
        ("shell", json!({"command": "touch", "args": ["escaped"]})),
    ];
    let app = app(
        Setup {
            agents: &[("scoped", "tools: Bash(git status:*)")],
            user_config: Some(r#"{"sandbox": {"enabled": false}}"#),
            ..Setup::default()
        },
        spawning(Some("scoped"), calling(calls)),
    )
    .await;
    run(&app, "yolo", "go").await;
    let (tools, _) = child_request(&app).await;
    assert!(tools.iter().any(|tool| tool == "shell"), "{tools:?}");
    let answered = child_results(&app).await;
    assert!(answered[0].contains("exit_code"), "{}", answered[0]);
    assert!(answered[1].contains("denied"), "{}", answered[1]);
    assert!(!app.project.path().join("escaped").exists());
    let listed = call(
        app.data.path(),
        "list_subagents",
        json!({"threadId": app.thread}),
    );
    assert_eq!(listed[0]["confined"], false, "{listed}");
    app.serving.abort();
}

/// What the scopes name still runs: a limit nobody can use isn't one.
#[tokio::test(flavor = "multi_thread")]
async fn a_scoped_shell_still_runs_what_it_names() {
    let shell = |args: Value| ("shell", args);
    let calls: Vec<(&'static str, Value)> = vec![
        shell(json!({"command": "python3", "args": ["tools/tool.py", "--flag", "-c"]})),
        shell(json!({"command": "git", "args": ["status", "--short"]})),
        shell(json!({"command": "find", "args": [".", "-name", "*.py"]})),
        shell(json!({"command": "sh", "args": ["-c", "git status --short 2>&1"]})),
    ];
    let count = calls.len();
    let app = app(
        Setup {
            agents: &[(
                "scoped",
                "tools: Bash(python3 *tool.py*), Bash(git status:*), Bash(find . -name *), Bash(ls)",
            )],
            ..Setup::default()
        },
        spawning(Some("scoped"), calling(calls)),
    )
    .await;
    std::fs::create_dir_all(app.project.path().join("tools")).unwrap();
    std::fs::write(
        app.project.path().join("tools/tool.py"),
        "print('script ran')",
    )
    .unwrap();
    run(&app, "yolo", "go").await;
    let answered = child_results(&app).await;
    assert_eq!(answered.len(), count, "{answered:?}");
    assert!(answered[0].contains("script ran"), "{}", answered[0]);
    for answer in &answered {
        assert!(!answer.contains("denied"), "{answer}");
    }
    app.serving.abort();
}
