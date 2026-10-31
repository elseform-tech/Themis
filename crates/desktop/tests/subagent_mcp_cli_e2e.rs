//! MCP tools for subagents, through the real shared server and the `themis` CLI with a scripted
//! model. What a profile names or the main agent gives decides which tools a subagent gets, from
//! the connections the person enabled; approvals, deny rules, hooks and stopping hold for them.
#![cfg(unix)]

use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use serde_json::{json, Value};
use themis_desktop::{server::Server, state::AppState, types::ProviderKind};
use wiremock::matchers::method;
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

/// The MCP tools among a request's tools, by the server's own names.
fn mcp_tools(body: &Value) -> Vec<String> {
    let mut names: Vec<String> = tool_names(body)
        .iter()
        .filter(|name| name.starts_with("mcp_"))
        .map(|name| name.rsplit('_').next().unwrap_or_default().to_owned())
        .collect();
    names.sort();
    names
}

fn wire_name(body: &Value, tail: &str) -> Option<String> {
    tool_names(body)
        .into_iter()
        .find(|name| name.starts_with("mcp_") && name.ends_with(tail))
}

/// An MCP server with the tools `lookup` and `other`; a call answers after `delay`.
async fn mcp_server(delay: Duration) -> MockServer {
    let mcp = MockServer::start().await;
    Mock::given(method("POST"))
        .respond_with(move |request: &Request| {
            let body: Value = serde_json::from_slice(&request.body).unwrap();
            let schema = json!({"type": "object", "properties": {}});
            let method = body["method"].as_str().unwrap_or_default();
            let result = match method {
                "initialize" => json!({"protocolVersion": "2025-03-26", "capabilities": {},
                    "serverInfo": {"name": "docs", "version": "1"}}),
                "tools/list" => json!({"tools": [
                    {"name": "lookup", "description": "x", "inputSchema": schema},
                    {"name": "other", "description": "x", "inputSchema": schema}
                ]}),
                "tools/call" => json!({"content": [{"type": "text", "text": "SERVED"}]}),
                _ => json!({}),
            };
            let response = ResponseTemplate::new(200)
                .set_body_json(json!({"jsonrpc": "2.0", "id": body["id"], "result": result}));
            if method == "tools/call" {
                response.set_delay(delay)
            } else {
                response
            }
        })
        .mount(&mcp)
        .await;
    mcp
}

async fn calls_to(mcp: &MockServer) -> usize {
    mcp.received_requests()
        .await
        .unwrap()
        .iter()
        .filter(|request| {
            request.body_json::<Value>().unwrap_or(Value::Null)["method"] == "tools/call"
        })
        .count()
}

struct App {
    llm: MockServer,
    mcp: MockServer,
    data: tempfile::TempDir,
    project: tempfile::TempDir,
    _plugins: tempfile::TempDir,
    thread: String,
    serving: tokio::task::JoinHandle<()>,
}

struct Setup<'a> {
    agents: &'a [(&'a str, &'a str)],
    hooks: Option<Value>,
    config: Option<&'a str>,
    enable_server: bool,
    delay: Duration,
}

impl Default for Setup<'_> {
    fn default() -> Self {
        Self {
            agents: &[],
            hooks: None,
            config: None,
            enable_server: true,
            delay: Duration::ZERO,
        }
    }
}

fn plugin_folder(parent: &Path, setup: &Setup<'_>, url: &str) -> PathBuf {
    let root = parent.join("kit");
    std::fs::create_dir_all(root.join(".claude-plugin")).unwrap();
    std::fs::create_dir_all(root.join("agents")).unwrap();
    std::fs::write(
        root.join(".claude-plugin/plugin.json"),
        json!({"name": "kit", "description": "A test plugin"}).to_string(),
    )
    .unwrap();
    std::fs::write(
        root.join(".mcp.json"),
        json!({"mcpServers": {"docs": {"type": "http", "url": url}}}).to_string(),
    )
    .unwrap();
    for (name, frontmatter) in setup.agents {
        std::fs::write(
            root.join(format!("agents/{name}.md")),
            format!("---\nname: {name}\ndescription: A role.\n{frontmatter}---\nPlay the role.\n"),
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
    root
}

async fn app(
    setup: Setup<'_>,
    respond: impl Fn(&Value) -> ResponseTemplate + Send + Sync + 'static,
) -> (App, Value) {
    let llm = MockServer::start().await;
    Mock::given(method("POST"))
        .respond_with(move |request: &Request| respond(&request.body_json().unwrap_or(Value::Null)))
        .mount(&llm)
        .await;
    let mcp = mcp_server(setup.delay).await;
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
    let folder = plugin_folder(plugins.path(), &setup, &mcp.uri());
    let directory = data.path().to_owned();
    let enable = setup.enable_server;
    let plugin = tokio::task::spawn_blocking(move || {
        let plugin = themis(
            &directory,
            &[
                "plugin",
                "import",
                folder.to_str().unwrap(),
                "--allow-partial",
            ],
        );
        if enable {
            themis(&directory, &["mcp", "enable", "kit", "docs"]);
        }
        for hook in plugin["spec"]["hooks"].as_array().into_iter().flatten() {
            themis(
                &directory,
                &["hook", "enable", "kit", hook["name"].as_str().unwrap()],
            );
        }
        plugin
    })
    .await
    .unwrap();
    (
        App {
            llm,
            mcp,
            data,
            project,
            _plugins: plugins,
            thread,
            serving,
        },
        plugin,
    )
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

fn child<'a>(all: &'a [Value], brief: &str) -> &'a Value {
    all.iter()
        .find(|body| is_subagent(body) && mentions(body, brief))
        .unwrap_or_else(|| panic!("no child for {brief}"))
}

/// The main agent's spawn call with `subagents` (built once it knows its tools), then done.
fn spawning(
    subagents: impl Fn(&Value) -> Value + Send + Sync + 'static,
    child: impl Fn(&Value) -> ResponseTemplate + Send + Sync + 'static,
) -> impl Fn(&Value) -> ResponseTemplate + Send + Sync + 'static {
    move |body| {
        if is_subagent(body) {
            return child(body);
        }
        if tool_results(body) == 0 {
            return tool_call(
                "s1",
                "spawn_subagents",
                &json!({"say": "Starting.", "subagents": subagents(body)}),
            );
        }
        if mentions(body, NOTE) {
            reply("Done.")
        } else {
            reply("Waiting.")
        }
    }
}

/// A subagent that calls its `lookup` once, then reports.
fn calls_lookup(body: &Value) -> ResponseTemplate {
    match (tool_results(body), wire_name(body, "_lookup")) {
        (0, Some(name)) => tool_call("c1", &name, &json!({"status": "Looking it up"})),
        _ => reply("Reported."),
    }
}

fn diagnostics(data: &Path) -> String {
    let mut all = String::new();
    for entry in std::fs::read_dir(data.join("logs")).into_iter().flatten() {
        all.push_str(&std::fs::read_to_string(entry.unwrap().path()).unwrap_or_default());
    }
    all
}

#[tokio::test(flavor = "multi_thread")]
async fn what_a_profile_names_decides_its_mcp_tools() {
    let (app, plugin) = app(
        Setup {
            agents: &[
                ("named", "tools: Read, mcp__docs__lookup\n"),
                ("in-plugin", "tools: mcp__plugin_kit_docs__lookup\n"),
                ("server-wide", "tools: Read, mcp__docs__*\n"),
                ("all-listed", "tools: \"*\"\n"),
                ("no-list", ""),
                ("reads-only", "tools: Read\n"),
                ("denied", "disallowedTools: mcp__docs__other\n"),
            ],
            ..Setup::default()
        },
        spawning(
            |_| {
                json!([
                    {"brief": "brief-named", "profile": "named"},
                    {"brief": "brief-in-plugin", "profile": "in-plugin"},
                    {"brief": "brief-server-wide", "profile": "server-wide"},
                    {"brief": "brief-all-listed", "profile": "all-listed"},
                    {"brief": "brief-no-list", "profile": "no-list"},
                    {"brief": "brief-reads-only", "profile": "reads-only"}
                ])
            },
            |_| reply("Reported."),
        ),
    )
    .await;
    // Named MCP tools now map, so they don't make the plugin partial.
    let reasons = plugin["spec"]["unsupported"].to_string();
    assert!(!reasons.contains("mcp__"), "{reasons}");
    run(&app, "yolo", "go").await;
    let all = bodies(&app.llm).await;
    let both = ["lookup", "other"];
    assert_eq!(mcp_tools(child(&all, "brief-named")), ["lookup"]);
    assert_eq!(mcp_tools(child(&all, "brief-in-plugin")), ["lookup"]);
    assert_eq!(mcp_tools(child(&all, "brief-server-wide")), both);
    assert_eq!(mcp_tools(child(&all, "brief-all-listed")), both);
    assert_eq!(mcp_tools(child(&all, "brief-no-list")), both);
    assert!(mcp_tools(child(&all, "brief-reads-only")).is_empty());
    app.serving.abort();
}

#[tokio::test(flavor = "multi_thread")]
async fn a_deny_list_takes_mcp_tools_away() {
    let (app, _) = app(
        Setup {
            agents: &[("denied", "disallowedTools: mcp__docs__other\n")],
            ..Setup::default()
        },
        spawning(
            |_| json!([{"brief": "brief-denied", "profile": "denied"}]),
            |_| reply("Reported."),
        ),
    )
    .await;
    run(&app, "yolo", "go").await;
    let all = bodies(&app.llm).await;
    assert_eq!(mcp_tools(child(&all, "brief-denied")), ["lookup"]);
    app.serving.abort();
}

#[tokio::test(flavor = "multi_thread")]
async fn the_main_agent_gives_a_plain_subagent_its_mcp_tools() {
    let (app, _) = app(
        Setup::default(),
        spawning(
            |body| {
                let lookup = wire_name(body, "_lookup").expect("the main agent's tool");
                json!([
                    {"brief": "brief-none"},
                    {"brief": "brief-one", "mcp_tools": [lookup]},
                    {"brief": "brief-all", "mcp_tools": ["*"]},
                    {"brief": "brief-unknown", "mcp_tools": ["mcp_nothing_like_it"]}
                ])
            },
            |_| reply("Reported."),
        ),
    )
    .await;
    run(&app, "yolo", "go").await;
    let all = bodies(&app.llm).await;
    let main = all.iter().find(|body| !is_subagent(body)).unwrap();
    let spawn = main["tools"]
        .as_array()
        .unwrap()
        .iter()
        .find(|tool| tool["function"]["name"] == "spawn_subagents")
        .unwrap();
    assert!(
        spawn["function"]["parameters"]["properties"]["subagents"]["items"]["properties"]
            ["mcp_tools"]
            .is_object(),
        "{spawn}"
    );
    assert!(mcp_tools(child(&all, "brief-none")).is_empty());
    assert_eq!(mcp_tools(child(&all, "brief-one")), ["lookup"]);
    assert_eq!(mcp_tools(child(&all, "brief-all")), ["lookup", "other"]);
    // Left out and said so, on both sides, without failing the call.
    let unknown = child(&all, "brief-unknown");
    assert!(mcp_tools(unknown).is_empty());
    assert!(mentions(unknown, "mcp_nothing_like_it"));
    let result = all
        .iter()
        .filter(|body| !is_subagent(body))
        .flat_map(|body| messages(body).iter())
        .find(|message| message["role"] == "tool")
        .map(text)
        .unwrap_or_default()
        .to_owned();
    assert!(result.contains("mcp_nothing_like_it"), "{result}");
    assert!(result.contains("mcp_left_out"), "{result}");
    app.serving.abort();
}

#[tokio::test(flavor = "multi_thread")]
async fn a_connection_that_is_not_enabled_stays_out_and_the_subagent_is_told() {
    let (app, plugin) = app(
        Setup {
            agents: &[
                ("named", "tools: Read, mcp__docs__lookup\n"),
                ("no-list", ""),
            ],
            enable_server: false,
            ..Setup::default()
        },
        spawning(
            |_| {
                json!([
                    {"brief": "brief-named", "profile": "named"},
                    {"brief": "brief-no-list", "profile": "no-list"}
                ])
            },
            |_| reply("Reported."),
        ),
    )
    .await;
    assert_eq!(plugin["spec"]["mcp"]["docs"]["enabled"], false, "{plugin}");
    run(&app, "yolo", "go").await;
    let all = bodies(&app.llm).await;
    let named = child(&all, "brief-named");
    assert!(mcp_tools(named).is_empty());
    assert!(mentions(named, "mcp__docs__lookup"), "told what is missing");
    assert!(mcp_tools(child(&all, "brief-no-list")).is_empty());
    // Nothing a subagent has can add or enable a connection.
    for name in tool_names(named) {
        assert!(
            !name.contains("integration") && !name.contains("plugin"),
            "{name}"
        );
    }
    // Only the named one counts as left out, in the diagnostics too: metadata, by name.
    let logged = diagnostics(app.data.path());
    assert!(logged.contains("mcp_left_out"), "{logged}");
    assert!(logged.contains("not_connected"), "{logged}");
    assert!(logged.contains("mcp__docs__lookup"), "{logged}");
    // The agents run on, and nothing reached the server.
    assert!(all.iter().any(|body| mentions(body, "Reported.")));
    assert_eq!(calls_to(&app.mcp).await, 0);
    app.serving.abort();
}

#[tokio::test(flavor = "multi_thread")]
async fn allow_mcp_off_gives_no_subagent_any_mcp_tool() {
    let (app, _) = app(
        Setup {
            agents: &[
                ("named", "tools: Read, mcp__docs__lookup\n"),
                ("no-list", ""),
            ],
            config: Some(r#"{"subagents": {"allow_mcp": false}}"#),
            ..Setup::default()
        },
        spawning(
            |_| {
                json!([
                    {"brief": "brief-named", "profile": "named"},
                    {"brief": "brief-no-list", "profile": "no-list"},
                    {"brief": "brief-asked", "mcp_tools": ["*"]}
                ])
            },
            calls_lookup,
        ),
    )
    .await;
    run(&app, "yolo", "go").await;
    let all = bodies(&app.llm).await;
    for brief in ["brief-named", "brief-no-list", "brief-asked"] {
        assert!(mcp_tools(child(&all, brief)).is_empty(), "{brief}");
    }
    let main = all.iter().find(|body| !is_subagent(body)).unwrap();
    assert!(
        !mcp_tools(main).is_empty(),
        "the main agent keeps its tools"
    );
    assert!(
        !main.to_string().contains("mcp_tools"),
        "the field isn't offered"
    );
    assert_eq!(calls_to(&app.mcp).await, 0);
    let logged = diagnostics(app.data.path());
    assert!(logged.contains("mcp_left_out"), "{logged}");
    assert!(logged.contains("setting"), "{logged}");
    app.serving.abort();
}

#[tokio::test(flavor = "multi_thread")]
async fn in_custom_a_subagents_mcp_call_asks_under_its_name_and_its_grant_stays_its_own() {
    let (app, _) = app(
        Setup {
            agents: &[("role", "")],
            ..Setup::default()
        },
        |body: &Value| {
            if is_subagent(body) {
                return calls_lookup(body);
            }
            let results = tool_results(body);
            if results == 0 {
                return tool_call(
                    "s1",
                    "spawn_subagents",
                    &json!({"say": "Starting.", "subagents": [
                        {"name": "Finder", "brief": "brief", "profile": "role"}
                    ]}),
                );
            }
            // The main agent then calls the same tool itself.
            if mentions(body, NOTE) && results == 1 {
                let name = wire_name(body, "_lookup").expect("the main agent's tool");
                return tool_call("m1", &name, &json!({}));
            }
            if mentions(body, NOTE) {
                reply("Done.")
            } else {
                reply("Waiting.")
            }
        },
    )
    .await;
    let (data, thread) = (app.data.path().to_owned(), app.thread.clone());
    let pending_first = tokio::task::spawn_blocking({
        let (data, thread) = (data.clone(), thread.clone());
        move || {
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
            let mut pending = Value::Null;
            until("the subagent never asked", || {
                pending = call(&data, "get_pending_approvals", json!({}));
                pending.as_array().is_some_and(|list| !list.is_empty())
            });
            pending
        }
    })
    .await
    .unwrap();
    assert_eq!(
        pending_first[0]["subagent"]["name"], "Finder",
        "{pending_first}"
    );
    assert_eq!(calls_to(&app.mcp).await, 0, "it waits for the answer");
    let approval = pending_first[0]["approval_id"].clone();
    let main_pending = tokio::task::spawn_blocking({
        let (data, thread) = (data.clone(), thread.clone());
        move || {
            call(
                &data,
                "approve_action",
                json!({"threadId": thread, "approvalId": approval, "decision": "chat"}),
            );
            // Its "in this chat" covers it alone: the main agent asks for the same tool.
            let mut pending = Value::Null;
            until("the main agent never asked", || {
                pending = call(&data, "get_pending_approvals", json!({}));
                pending
                    .as_array()
                    .is_some_and(|list| list.iter().any(|item| item["subagent"].is_null()))
            });
            pending
        }
    })
    .await
    .unwrap();
    assert_eq!(calls_to(&app.mcp).await, 1, "{main_pending}");
    let grants = data.join("chat_grants").join(format!("{thread}.json"));
    assert!(
        !std::fs::read_to_string(&grants)
            .unwrap_or_default()
            .contains("mcp"),
        "a subagent's grant reached the chat's file"
    );
    let approval = main_pending[0]["approval_id"].clone();
    tokio::task::spawn_blocking(move || {
        call(
            &data,
            "approve_action",
            json!({"threadId": thread, "approvalId": approval, "decision": "deny"}),
        );
        until("the run never finished", || {
            call(&data, "get_thread", json!({"threadId": thread}))["running"] != true
        });
    })
    .await
    .unwrap();
    app.serving.abort();
}

#[tokio::test(flavor = "multi_thread")]
async fn a_deny_rule_beats_a_subagents_mcp_call_even_in_yolo() {
    let (app, _) = app(
        Setup {
            agents: &[("role", "")],
            ..Setup::default()
        },
        |body: &Value| {
            if is_subagent(body) {
                return calls_lookup(body);
            }
            if mentions(body, "learn the tool") {
                return reply("Known.");
            }
            match tool_results(body) {
                0 => tool_call(
                    "s1",
                    "spawn_subagents",
                    &json!({"say": "Starting.", "subagents": [{"brief": "brief", "profile": "role"}]}),
                ),
                _ if mentions(body, NOTE) => reply("Done."),
                _ => reply("Waiting."),
            }
        },
    )
    .await;
    // The rule names the exact tool, so the server still starts.
    run(&app, "yolo", "learn the tool").await;
    let lookup = wire_name(&bodies(&app.llm).await[0], "_lookup").expect("the tool");
    let config = json!({"approval": {"rules": [{"tool": lookup, "action": "deny"}]}});
    std::fs::create_dir_all(app.project.path().join(".themis")).unwrap();
    std::fs::write(
        app.project.path().join(".themis/config.jsonc"),
        config.to_string(),
    )
    .unwrap();
    // A new chat, so the turn above doesn't colour what the model sees.
    let data = app.data.path().to_owned();
    let project = app.project.path().to_string_lossy().into_owned();
    let thread = tokio::task::spawn_blocking(move || {
        call(
            &data,
            "create_thread",
            json!({"projectRoot": project, "provider": "go", "model": "test-model"}),
        )
    })
    .await
    .unwrap();
    let thread = thread["id"].as_str().expect("a chat").to_owned();
    let (data, chat) = (app.data.path().to_owned(), thread);
    tokio::task::spawn_blocking(move || {
        call(
            &data,
            "set_thread_approval_mode",
            json!({"threadId": chat, "mode": "yolo"}),
        );
        call(
            &data,
            "send_message",
            json!({"threadId": chat, "text": "go"}),
        );
        until("the run never finished", || {
            call(&data, "get_thread", json!({"threadId": chat}))["running"] != true
        });
    })
    .await
    .unwrap();
    assert_eq!(calls_to(&app.mcp).await, 0, "the deny rule let it through");
    let all = bodies(&app.llm).await;
    let child = all.iter().rfind(|body| is_subagent(body)).unwrap();
    assert!(
        messages(child)
            .iter()
            .any(|m| m["role"] == "tool" && text(m).contains("denied")),
        "{child}"
    );
    app.serving.abort();
}

#[tokio::test(flavor = "multi_thread")]
async fn hooks_fire_on_a_subagents_mcp_call_and_name_the_agent() {
    let (app, _) = app(
        Setup {
            agents: &[("role", "")],
            hooks: Some(json!({
                "PreToolUse": [{"matcher": "mcp__docs__.*", "hooks": [
                    {"type": "command", "command": "cat > before.json"}
                ]}],
                "PostToolUse": [{"matcher": "mcp__docs__.*", "hooks": [
                    {"type": "command", "command": "cat > after.json"}
                ]}]
            })),
            ..Setup::default()
        },
        spawning(
            |_| json!([{"name": "Finder", "brief": "brief", "profile": "role"}]),
            calls_lookup,
        ),
    )
    .await;
    run(&app, "yolo", "go").await;
    for file in ["before.json", "after.json"] {
        let seen: Value = serde_json::from_slice(
            &std::fs::read(app.project.path().join(file)).unwrap_or_else(|_| panic!("{file}")),
        )
        .unwrap();
        assert_eq!(seen["agent"]["name"], "Finder", "{seen}");
        assert_eq!(seen["tool_name"], "mcp__docs__lookup", "{seen}");
    }
    assert_eq!(calls_to(&app.mcp).await, 1);
    app.serving.abort();
}

#[tokio::test(flavor = "multi_thread")]
async fn stopping_a_subagent_ends_its_mcp_call() {
    let (app, _) = app(
        Setup {
            agents: &[("role", "")],
            delay: Duration::from_secs(25),
            ..Setup::default()
        },
        spawning(
            |_| json!([{"name": "Finder", "brief": "brief", "profile": "role"}]),
            calls_lookup,
        ),
    )
    .await;
    let (data, thread) = (app.data.path().to_owned(), app.thread.clone());
    let mcp_calls = {
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
            until("the subagent never started", || {
                call(&data, "list_subagents", json!({"threadId": thread}))[0]["tool_calls"]
                    .as_u64()
                    .is_some_and(|calls| calls > 0)
            });
        })
    };
    mcp_calls.await.unwrap();
    let started = Instant::now();
    tokio::task::spawn_blocking(move || {
        let id = call(&data, "list_subagents", json!({"threadId": thread}))[0]["id"]
            .as_str()
            .unwrap()
            .to_owned();
        call(
            &data,
            "stop_subagent",
            json!({"threadId": thread, "subagentId": id}),
        );
        until("the subagent never ended", || {
            call(&data, "list_subagents", json!({"threadId": thread}))[0]["status"] == "stopped"
        });
    })
    .await
    .unwrap();
    assert!(
        started.elapsed() < Duration::from_secs(10),
        "it waited for the server: {:?}",
        started.elapsed()
    );
    app.serving.abort();
}

#[tokio::test(flavor = "multi_thread")]
async fn mcp_tools_are_no_file_permission_and_the_subagent_is_told_what_they_are() {
    let refused = std::sync::Arc::new(std::sync::Mutex::new(String::new()));
    let seen = std::sync::Arc::clone(&refused);
    let (app, _) = app(
        Setup {
            agents: &[("only-mcp", "tools: mcp__docs__lookup\n")],
            ..Setup::default()
        },
        move |body: &Value| {
            if is_subagent(body) {
                return reply("Reported.");
            }
            match tool_results(body) {
                0 => tool_call(
                    "s1",
                    "spawn_subagents",
                    &json!({"say": "Asking.", "subagents": [
                        {"brief": "brief-writes", "profile": "only-mcp", "write_paths": ["notes/"]}
                    ]}),
                ),
                1 if !mentions(body, NOTE) && !mentions(body, "Waiting") => {
                    *seen.lock().unwrap() = messages(body)
                        .iter()
                        .rfind(|m| m["role"] == "tool")
                        .map(text)
                        .unwrap_or_default()
                        .to_owned();
                    tool_call(
                        "s2",
                        "spawn_subagents",
                        &json!({"say": "Without paths.", "subagents": [
                            {"name": "Reader", "brief": "brief-reads", "profile": "only-mcp"}
                        ]}),
                    )
                }
                _ if mentions(body, NOTE) => reply("Done."),
                _ => reply("Waiting."),
            }
        },
    )
    .await;
    run(&app, "yolo", "go").await;
    // MCP tools don't count as write tools: the same refusal as a list with none.
    let refusal = refused.lock().unwrap().clone();
    assert!(refusal.contains("no write tools"), "{refusal}");
    let all = bodies(&app.llm).await;
    let reader = child(&all, "brief-reads");
    assert_eq!(mcp_tools(reader), ["lookup"]);
    let names = tool_names(reader);
    assert!(!names.iter().any(|name| name == "write_file"), "{names:?}");
    let system = text(&messages(reader)[0]);
    assert!(
        system.contains(themis_core::prompts::SUBAGENT_READ_ONLY_NOTE),
        "{system}"
    );
    assert!(system.contains("outside the project"), "{system}");
    // The listing and the saved event say which connections it can use; paths stay empty.
    let data = app.data.path().to_owned();
    let thread = app.thread.clone();
    let listed = tokio::task::spawn_blocking(move || {
        call(&data, "list_subagents", json!({"threadId": thread}))
    })
    .await
    .unwrap();
    assert_eq!(listed[0]["connections"], json!(["docs"]), "{listed}");
    assert_eq!(listed[0]["write_paths"], json!([]), "{listed}");
    let saved = std::sync::Arc::new(app.data.path().to_owned());
    let thread = app.thread.clone();
    let history = tokio::task::spawn_blocking(move || {
        call(&saved, "get_thread_history", json!({"threadId": thread}))
    })
    .await
    .unwrap();
    assert!(history.to_string().contains(r#""connections":["docs"]"#));
    app.serving.abort();
}
