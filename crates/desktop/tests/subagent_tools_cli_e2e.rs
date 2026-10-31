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
