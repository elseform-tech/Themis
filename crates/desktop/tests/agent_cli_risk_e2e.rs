//! `themis` calls in an agent's shell at the approval step, through the real app server and a
//! scripted provider: a read runs without a prompt, a change asks once and "Allow in this chat"
//! covers the same command, switching on a hook asks every time and never gains a chat grant,
//! the agent's saves keep a new hook switched off, and a deny rule turns `themis` off in YOLO.
//! Whatever the shell allows, the server never lets an agent's credential switch a hook or a
//! command-based MCP server on, by `themis call` or a request on the socket, in any mode.
//! In-process app server, isolated app data, synthetic credentials only.
#![cfg(unix)]

use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use serde_json::{json, Value};
use themis_core::configuration::ApprovalMode;
use themis_desktop::server::Server;
use themis_desktop::sink::{ChannelSink, TestEvent};
use themis_desktop::state::AppState;
use themis_desktop::types::{ApprovalDecision, ProviderKind, ThreadEvent};
use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, Request, ResponseTemplate};

#[allow(dead_code)]
mod common;

/// One app server per process installs the run-credential issuer, so these tests take turns.
static ONE_SERVER: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

struct Fixture {
    _dir: tempfile::TempDir,
    _repo: tempfile::TempDir,
    data: PathBuf,
    project: PathBuf,
    state: AppState,
    stop: Option<tokio::sync::oneshot::Sender<()>>,
    served: Option<tokio::task::JoinHandle<Result<(), String>>>,
}

async fn fixture() -> Fixture {
    let dir = tempfile::tempdir_in(env!("CARGO_TARGET_TMPDIR")).unwrap();
    let data = dir.path().canonicalize().unwrap().join("app-data");
    std::fs::create_dir_all(&data).unwrap();
    let (repo, project) = common::init_repo();
    let state = AppState::new_for_test(data.join("settings.json"));
    let server = Server::bind(state.clone(), &data)
        .await
        .unwrap()
        .with_agent_cli(PathBuf::from(env!("CARGO_BIN_EXE_themis")));
    let (stop, stopped) = tokio::sync::oneshot::channel::<()>();
    let served = tokio::spawn(server.run_until(async {
        let _ = stopped.await;
    }));
    Fixture {
        _dir: dir,
        _repo: repo,
        data,
        project: project.canonicalize().unwrap(),
        state,
        stop: Some(stop),
        served: Some(served),
    }
}

impl Fixture {
    async fn shut_down(mut self) {
        let _ = self.stop.take().unwrap().send(());
        self.served.take().unwrap().await.unwrap().unwrap();
    }

    async fn thread(&self, mode: ApprovalMode) -> String {
        let id = self
            .state
            .create_thread(
                self.project.to_string_lossy().into_owned(),
                ProviderKind::Go,
                Some("test-model".to_owned()),
            )
            .await
            .unwrap()
            .id;
        self.state
            .set_thread_approval_mode(id.clone(), mode)
            .await
            .unwrap();
        id
    }
}

/// A provider that makes one `shell` call per round, then answers.
async fn mount_calls(calls: Vec<Vec<String>>) -> MockServer {
    let server = MockServer::builder().start().await;
    Mock::given(method("POST"))
        .and(path("/chat/completions"))
        .respond_with(move |request: &Request| {
            let body: Value = serde_json::from_slice(&request.body).unwrap_or(Value::Null);
            let done = body["messages"]
                .as_array()
                .map(|m| m.iter().filter(|m| m["role"] == "tool").count())
                .unwrap_or(0);
            let payload = match calls.get(done) {
                Some(argv) => json!({"choices":[{"message":{"role":"assistant","content":null,
                    "tool_calls":[{"id":format!("call_{done}"),"type":"function","function":{
                        "name":"shell",
                        "arguments":json!({"command":argv[0],"args":&argv[1..]}).to_string()}}]}}]}),
                None => json!({"choices":[{"message":{"role":"assistant","content":"finished"},
                    "finish_reason":"stop"}]}),
            };
            ResponseTemplate::new(200).set_body_json(payload)
        })
        .mount(&server)
        .await;
    server
}

fn words(text: &str) -> Vec<String> {
    text.split(' ').map(str::to_owned).collect()
}

/// Sends one message, answers the approval dialogs in order with `answers` (deny once they run
/// out), and returns the dialogs (as JSON) and each shell output.
async fn run(
    fx: &Fixture,
    thread: &str,
    answers: &[ApprovalDecision],
) -> (Vec<Value>, Vec<String>) {
    let (sink, mut rx) = ChannelSink::channel();
    fx.state
        .send_message(
            Arc::new(sink),
            thread.to_owned(),
            "manage the plugin".into(),
        )
        .await
        .unwrap();
    let mut asked = Vec::new();
    let mut outputs = Vec::new();
    let finished = tokio::time::timeout(Duration::from_secs(120), async {
        while let Some(event) = rx.recv().await {
            match event {
                TestEvent::Approval(request) => {
                    let answer = answers
                        .get(asked.len())
                        .copied()
                        .unwrap_or(ApprovalDecision::Deny);
                    asked.push(serde_json::to_value(&request).unwrap());
                    fx.state
                        .approve_action(thread.to_owned(), request.approval_id.clone(), answer)
                        .await
                        .unwrap();
                }
                TestEvent::Thread(envelope) => match envelope.event {
                    ThreadEvent::ToolFinished { tool, output, .. } if tool == "shell" => {
                        outputs.push(output)
                    }
                    ThreadEvent::Finished { .. } => break,
                    ThreadEvent::Failed { error, .. } => panic!("run failed: {error}"),
                    _ => {}
                },
                _ => {}
            }
        }
    })
    .await;
    assert!(finished.is_ok(), "run did not finish");
    (asked, outputs)
}

fn argv_of(dialog: &Value) -> Vec<String> {
    serde_json::from_value(dialog["command"]["argv"].clone()).unwrap_or_default()
}

/// The store's answer inside a shell result (`{"output": "<json>\n", ...}`).
fn command_json(output: &str) -> Value {
    let result: Value = serde_json::from_str(output).unwrap();
    assert_eq!(result["exit_code"], 0, "{output}");
    serde_json::from_str(result["output"].as_str().unwrap().trim()).unwrap()
}

#[tokio::test(flavor = "multi_thread")]
async fn custom_lets_reads_run_and_asks_every_time_before_switching_on_a_hook() {
    if !common::git_available() {
        return;
    }
    let _turn = ONE_SERVER.lock().await;
    let fx = fixture().await;
    // The definition asks for the hook switched on; the agent's save keeps it off.
    let hook = fx.project.join("hook.json");
    std::fs::write(
        &hook,
        json!({"event":"BeforeTool","command":"echo guarded","enabled":true}).to_string(),
    )
    .unwrap();
    let hook = hook.display().to_string();
    let calls = vec![
        words("themis plugin list"),
        words("themis plugin create demo"),
        words("themis plugin create demo"),
        words(&format!("themis hook add demo guard {hook}")),
        words("themis hook enable demo guard"),
        words("themis hook enable demo guard"),
        vec![
            "sh".to_owned(),
            "-c".to_owned(),
            "themis hook enable demo guard".to_owned(),
        ],
        words("themis plugin list"),
    ];
    let provider = mount_calls(calls).await;
    common::use_mock_llm(&fx.state, &provider).await;
    let thread = fx.thread(ApprovalMode::Custom).await;
    use ApprovalDecision::{Chat, Deny, Once};
    let (asked, outputs) = run(&fx, &thread, &[Chat, Once, Chat, Deny, Deny]).await;
    assert_eq!(outputs.len(), 8, "{outputs:#?}");
    let asked_for: Vec<Vec<String>> = asked.iter().map(argv_of).collect();
    assert_eq!(
        asked_for,
        vec![
            words("themis plugin create demo"),
            words(&format!("themis hook add demo guard {hook}")),
            words("themis hook enable demo guard"),
            words("themis hook enable demo guard"),
            vec![
                "sh".to_owned(),
                "-c".to_owned(),
                "themis hook enable demo guard".to_owned()
            ],
        ],
        "the read and the change already allowed in this chat ran without a prompt"
    );
    // A change is a write the chat may allow; switching on a hook is asked every time.
    assert_eq!(asked[0]["risk"], "write", "{}", asked[0]);
    assert_eq!(asked[0]["grantable"], true);
    for dialog in &asked[2..] {
        assert_eq!(dialog["grantable"], false, "{dialog}");
    }
    assert_eq!(asked[2]["risk"], "execute");

    assert_eq!(command_json(&outputs[0]), json!([]));
    let added = command_json(&outputs[3]);
    assert_eq!(added["spec"]["hooks"][0]["name"], "guard");
    assert_eq!(
        added["spec"]["hooks"][0]["enabled"], false,
        "the agent's save keeps a new hook off"
    );
    // Even with the command allowed, the server refuses an agent switching a hook on.
    let refused: Value = serde_json::from_str(&outputs[4]).unwrap();
    assert_ne!(refused["exit_code"], 0, "{}", outputs[4]);
    assert!(
        refused["output"]
            .as_str()
            .unwrap()
            .contains(themis_core::prompts::AGENT_SWITCH_ON_REFUSED),
        "{}",
        outputs[4]
    );
    let listed = command_json(&outputs[7]);
    assert_eq!(listed[0]["spec"]["hooks"][0]["enabled"], false, "{listed}");
    for denied in &outputs[5..7] {
        assert!(denied.contains("denied"), "{denied}");
    }
    fx.shut_down().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn a_deny_rule_turns_themis_off_even_in_yolo() {
    if !common::git_available() {
        return;
    }
    let _turn = ONE_SERVER.lock().await;
    let fx = fixture().await;
    fx.state
        .save_runtime_configuration(
            "user",
            Some(fx.project.to_string_lossy().into_owned()),
            r#"{"approval":{"rules":[{"tool":"shell","command":["themis"],"action":"deny"}]}}"#,
        )
        .unwrap();
    let calls = vec![
        words("themis plugin list"),
        vec![
            "sh".to_owned(),
            "-c".to_owned(),
            "echo before && themis status".to_owned(),
        ],
        words("echo still-runs"),
    ];
    let provider = mount_calls(calls).await;
    common::use_mock_llm(&fx.state, &provider).await;
    let thread = fx.thread(ApprovalMode::Yolo).await;
    let (asked, outputs) = run(&fx, &thread, &[]).await;
    assert!(asked.is_empty(), "YOLO asks nobody: {asked:#?}");
    assert_eq!(outputs.len(), 3, "{outputs:#?}");
    for denied in &outputs[..2] {
        assert!(denied.starts_with("approval denied"), "{denied}");
        assert!(!denied.contains("exit_code"), "it never ran: {denied}");
    }
    assert!(outputs[2].contains("still-runs"), "{}", outputs[2]);
    fx.shut_down().await;
}

/// `themis` with `--data-dir`, as the person (no credential) or with an agent's credential.
fn cli(fx: &Fixture, agent: Option<&str>, words: &[&str]) -> std::process::Output {
    let mut command = std::process::Command::new(env!("CARGO_BIN_EXE_themis"));
    command
        .arg("--data-dir")
        .arg(&fx.data)
        .args(words)
        .env_remove("OPENCODE_KEY")
        .env_remove(themis_core::agent_cli::TOKEN_ENV)
        .env_remove(themis_core::agent_cli::PORT_ENV);
    if let Some(token) = agent {
        command
            .env(themis_core::agent_cli::TOKEN_ENV, token)
            .env(themis_core::agent_cli::PORT_ENV, port(fx).to_string());
    }
    command.output().expect("run CLI")
}

fn port(fx: &Fixture) -> u16 {
    let text = std::fs::read_to_string(fx.data.join("server.json")).unwrap();
    serde_json::from_str::<Value>(&text).unwrap()["port"]
        .as_u64()
        .unwrap() as u16
}

/// One request straight on the server's socket, as a script holding the credential could send.
fn socket_call(fx: &Fixture, token: &str, method: &str, args: Value) -> Value {
    use std::io::{Read, Write};
    let mut stream = std::net::TcpStream::connect(("127.0.0.1", port(fx))).unwrap();
    let request =
        serde_json::to_vec(&json!({"token": token, "method": method, "args": args})).unwrap();
    stream
        .write_all(&(request.len() as u32).to_be_bytes())
        .unwrap();
    stream.write_all(&request).unwrap();
    let mut length = [0u8; 4];
    stream.read_exact(&mut length).unwrap();
    let mut reply = vec![0u8; u32::from_be_bytes(length) as usize];
    stream.read_exact(&mut reply).unwrap();
    serde_json::from_slice(&reply).unwrap()
}

fn switches(fx: &Fixture) -> Value {
    let listed = cli(fx, None, &["plugin", "list"]);
    assert!(listed.status.success());
    let plugins: Value = serde_json::from_slice(&listed.stdout).unwrap();
    let spec = &plugins[0]["spec"];
    json!({
        "hook": spec["hooks"][0]["enabled"],
        "local": spec["mcp"]["local"]["enabled"],
        "remote": spec["mcp"]["remote"]["enabled"],
    })
}

#[tokio::test(flavor = "multi_thread")]
async fn an_agent_credential_never_switches_a_hook_or_command_server_on_in_any_mode() {
    let _turn = ONE_SERVER.lock().await;
    let fx = fixture().await;
    fx.state
        .plugin_action(json!({"action":"save","scope":"global","spec":{"name":"demo",
            "hooks":[{"name":"guard","event":"BeforeTool","command":"echo guarded","enabled":false}],
            "mcp":{"local":{"command":"sh","args":["-c","exit 0"],"enabled":false},
                   "remote":{"url":"https://example.com/mcp","enabled":false}}}}))
        .await
        .unwrap();
    let switch_on = |kind: &str, id: &str| {
        json!({"action":"set_component_enabled","scope":"global","name":"demo",
            "kind":kind,"id":id,"enabled":true})
    };
    let refused = themis_core::prompts::AGENT_SWITCH_ON_REFUSED;
    let mut tokens = Vec::new();
    for mode in [ApprovalMode::Yolo, ApprovalMode::Auto, ApprovalMode::Custom] {
        let thread = fx.thread(mode).await;
        let token = agent_token(&fx, &thread, false);
        // `themis call`, as a shell command whatever its approval said.
        for (kind, id) in [("hook", "guard"), ("mcp", "local")] {
            let args = switch_on(kind, id).to_string();
            let output = cli(&fx, Some(&token), &["call", "plugin_action", &args]);
            let text = String::from_utf8_lossy(&output.stderr);
            assert!(!output.status.success(), "{mode:?} {kind}");
            assert!(text.contains(refused), "{mode:?} {kind}: {text}");
        }
        // The CLI's own shortcut and a request straight on the socket.
        let output = cli(&fx, Some(&token), &["hook", "enable", "demo", "guard"]);
        assert!(
            String::from_utf8_lossy(&output.stderr).contains(refused),
            "{mode:?}"
        );
        let reply = socket_call(&fx, &token, "plugin_action", switch_on("hook", "guard"));
        assert_eq!(reply["ok"], false, "{mode:?}: {reply}");
        assert!(
            reply["error"].as_str().unwrap().contains(refused),
            "{reply}"
        );
        assert_eq!(
            switches(&fx),
            json!({"hook": false, "local": false, "remote": false}),
            "{mode:?}: nothing was switched on"
        );
        tokens.push(token);
    }
    // A connection by URL keeps the consent rule: the agent may switch it on, and it still waits
    // for the person's approval to start.
    let args = switch_on("mcp", "remote").to_string();
    let output = cli(&fx, Some(&tokens[0]), &["call", "plugin_action", &args]);
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    // The person's own switch-on works.
    let output = cli(&fx, None, &["hook", "enable", "demo", "guard"]);
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(
        switches(&fx),
        json!({"hook": true, "local": false, "remote": true})
    );
    // A plugin switched off with a hook on inside: the agent may not switch the plugin on.
    let off = cli(&fx, None, &["plugin", "disable", "demo"]);
    assert!(off.status.success());
    let output = cli(&fx, Some(&tokens[2]), &["plugin", "enable", "demo"]);
    assert!(String::from_utf8_lossy(&output.stderr).contains(refused));
    // Refusals are logged as metadata: never the credential or the definitions.
    let logs = std::fs::read_dir(fx.data.join("logs"))
        .unwrap()
        .filter_map(Result::ok)
        .map(|entry| std::fs::read_to_string(entry.path()).unwrap_or_default())
        .collect::<String>();
    assert!(logs.contains("agent_call_refused"), "{logs}");
    assert!(logs.contains("\"reason\":\"switch_on\""), "{logs}");
    assert!(tokens.iter().all(|token| !logs.contains(token.as_str())));
    assert!(!logs.contains("echo guarded") && !logs.contains("exit 0"));
    fx.shut_down().await;
}

/// `--help` anywhere prints usage and does nothing else, with or without an agent's credential:
/// `themis hook enable demo probe --help` leaves the hook as it was.
#[tokio::test(flavor = "multi_thread")]
async fn help_prints_usage_and_changes_nothing() {
    let _turn = ONE_SERVER.lock().await;
    let fx = fixture().await;
    fx.state
        .plugin_action(
            json!({"action": "save", "scope": "global", "spec": {"name": "demo",
            "hooks": [{"name": "probe", "event": "BeforeTool", "command": "true",
                "enabled": false}]}}),
        )
        .await
        .unwrap();
    let thread = fx.thread(ApprovalMode::Custom).await;
    let token = agent_token(&fx, &thread, false);
    for agent in [None, Some(token.as_str())] {
        for words in [
            &["hook", "--help"][..],
            &["hook", "enable", "demo", "probe", "--help"],
            &["plugin", "-h"],
        ] {
            let output = cli(&fx, agent, words);
            let text = String::from_utf8_lossy(&output.stdout);
            assert!(output.status.success(), "{words:?}: {output:?}");
            assert!(text.contains("mcp/hook list PLUGIN"), "{words:?}: {text}");
        }
    }
    let general = cli(&fx, None, &["-h"]);
    assert!(String::from_utf8_lossy(&general.stdout).contains("usage: themis"));
    assert_eq!(switches(&fx)["hook"], false, "help switched nothing on");
    fx.shut_down().await;
}

/// A run credential bound to the fixture's project, with or without a network block.
fn agent_token(fx: &Fixture, thread: &str, network_blocked: bool) -> String {
    let scope = themis_core::agent_cli::RunScope {
        project_root: Some(fx.project.clone()),
        network_blocked,
    };
    themis_core::agent_cli::issue(thread, "run-under-test", &scope)
        .unwrap()
        .env()[0]
        .1
        .clone()
}

fn person_token(fx: &Fixture) -> String {
    let text = std::fs::read_to_string(fx.data.join("server.json")).unwrap();
    serde_json::from_str::<Value>(&text).unwrap()["token"]
        .as_str()
        .unwrap()
        .to_owned()
}

fn logs(fx: &Fixture) -> String {
    std::fs::read_dir(fx.data.join("logs"))
        .unwrap()
        .filter_map(Result::ok)
        .map(|entry| std::fs::read_to_string(entry.path()).unwrap_or_default())
        .collect()
}

fn refused_with(reply: &Value, text: &str) -> bool {
    reply["ok"] == false && reply["error"].as_str().is_some_and(|e| e.contains(text))
}

/// A script holding the run credential talks to the server directly (no `themis` command for the
/// shell's approval step to see). The server alone decides: a hook or MCP test, which would run
/// its command outside the sandbox, an unknown action and a missing one are refused in every mode.
#[tokio::test(flavor = "multi_thread")]
async fn an_agent_credential_never_runs_a_hook_or_server_test_outside_the_sandbox() {
    let _turn = ONE_SERVER.lock().await;
    let fx = fixture().await;
    let marker = fx.data.join("hook-ran");
    let server_marker = fx.data.join("server-ran");
    let project = fx.project.to_string_lossy().into_owned();
    let hook = json!({"name": "probe", "event": "BeforeTool",
        "command": format!("touch '{}'", marker.display())});
    let test_hook = json!({"action": "test_hook", "projectRoot": project, "hook": hook});
    let test_mcp = json!({"action": "test_mcp", "projectRoot": project,
        "server": {"command": "sh",
            "args": ["-c", format!("touch '{}'", server_marker.display())]}});
    let mut saved = hook.clone();
    saved["enabled"] = json!(false);
    fx.state
        .plugin_action(json!({"action": "save", "scope": "global",
            "spec": {"name": "demo", "hooks": [saved]}}))
        .await
        .unwrap();
    let runs_code = themis_core::prompts::AGENT_CLI_RUNS_CODE_REFUSED;
    for mode in [ApprovalMode::Custom, ApprovalMode::Auto, ApprovalMode::Yolo] {
        let thread = fx.thread(mode).await;
        let token = agent_token(&fx, &thread, false);
        let reply = socket_call(&fx, &token, "plugin_action", test_hook.clone());
        assert!(!marker.exists(), "{mode:?}: the hook ran: {reply}");
        assert!(refused_with(&reply, runs_code), "{mode:?}: {reply}");
        let reply = socket_call(&fx, &token, "plugin_action", test_mcp.clone());
        assert!(!server_marker.exists(), "{mode:?}: the server ran: {reply}");
        assert!(refused_with(&reply, runs_code), "{mode:?}: {reply}");
        let unknown = json!({"action": "no_such_action"});
        let reply = socket_call(&fx, &token, "plugin_action", unknown);
        assert!(
            refused_with(&reply, "Themis refused `plugin_action no_such_action`"),
            "{mode:?}: {reply}"
        );
        // No action at all is not a `list`.
        let reply = socket_call(&fx, &token, "plugin_action", json!({"scope": "global"}));
        assert!(refused_with(&reply, "Themis refused"), "{mode:?}: {reply}");
        // `themis hook test` sends the same request.
        let output = cli(&fx, Some(&token), &["hook", "test", "demo", "probe"]);
        let text = String::from_utf8_lossy(&output.stderr);
        assert!(text.contains(runs_code), "{mode:?}: {text}");
        tokio::time::sleep(Duration::from_millis(200)).await;
        assert!(!marker.exists(), "{mode:?}: the hook ran");
        assert!(!server_marker.exists(), "{mode:?}: the server ran");
    }
    // The person's own credential still tests a hook.
    let reply = socket_call(&fx, &person_token(&fx), "plugin_action", test_hook);
    assert_eq!(reply["ok"], true, "{reply}");
    assert!(marker.exists(), "the person's test runs the hook");
    let logs = logs(&fx);
    assert!(logs.contains("\"reason\":\"runs_code\""), "{logs}");
    assert!(logs.contains("\"reason\":\"action_not_allowed\""), "{logs}");
    assert!(!logs.contains("touch '"), "never the definition: {logs}");
    fx.shut_down().await;
}

/// The credential is bound to its run's project and network: another project's plugins are out
/// of reach, and a run whose sandbox blocks the network cannot have the server fetch for it.
/// Store actions in its own project and globally still work.
#[tokio::test(flavor = "multi_thread")]
async fn an_agent_credential_stays_in_its_project_and_behind_its_network_block() {
    let _turn = ONE_SERVER.lock().await;
    let fx = fixture().await;
    let (_other_repo, other) = common::init_repo();
    let project = fx.project.to_string_lossy().into_owned();
    let thread = fx.thread(ApprovalMode::Yolo).await;
    let token = agent_token(&fx, &thread, false);
    let save = |root: &str, name: &str| {
        json!({"action": "save", "scope": "local", "projectRoot": root,
            "spec": {"name": name}})
    };
    let elsewhere = save(&other.to_string_lossy(), "elsewhere");
    let reply = socket_call(&fx, &token, "plugin_action", elsewhere);
    let other_project = themis_core::prompts::AGENT_CLI_OTHER_PROJECT_REFUSED;
    assert!(refused_with(&reply, other_project), "{reply}");
    assert!(!other.join(".themis").exists(), "nothing was written there");
    // Its own project, globally, and the plain store actions.
    for args in [
        save(&project, "here"),
        json!({"action": "save", "scope": "global", "spec": {"name": "everywhere"}}),
        json!({"action": "list", "projectRoot": project}),
        json!({"action": "inventory"}),
        json!({"action": "disable", "scope": "global", "name": "everywhere"}),
        json!({"action": "enable", "scope": "global", "name": "everywhere"}),
        json!({"action": "marketplaces"}),
        json!({"action": "sources"}),
        json!({"action": "updates"}),
        json!({"action": "mcp_status", "projectRoot": project}),
        json!({"action": "delete", "scope": "global", "name": "everywhere"}),
    ] {
        let reply = socket_call(&fx, &token, "plugin_action", args.clone());
        assert_eq!(reply["ok"], true, "{args}: {reply}");
    }
    assert!(fx.project.join(".themis/plugins").is_dir());
    // A fetching action: allowed without a network block, refused behind one.
    let source = fx.data.join("local-source");
    std::fs::create_dir_all(source.join(".claude-plugin")).unwrap();
    std::fs::write(
        source.join(".claude-plugin/marketplace.json"),
        r#"{"name":"local-source","plugins":[]}"#,
    )
    .unwrap();
    let preview = json!({"action": "preview_source", "source": source});
    let reply = socket_call(&fx, &token, "plugin_action", preview.clone());
    assert_eq!(reply["ok"], true, "{reply}");
    let blocked_thread = fx.thread(ApprovalMode::Yolo).await;
    let blocked = agent_token(&fx, &blocked_thread, true);
    let network_blocked = themis_core::prompts::AGENT_CLI_NETWORK_BLOCKED;
    for args in [
        preview,
        json!({"action": "install", "scope": "global", "name": "x", "marketplace": "y"}),
        json!({"action": "import_repository", "scope": "global",
            "url": "https://example.com/x.git"}),
        json!({"action": "catalog", "name": "claude-plugins-official"}),
    ] {
        let reply = socket_call(&fx, &blocked, "plugin_action", args.clone());
        assert!(refused_with(&reply, network_blocked), "{args}: {reply}");
    }
    let reply = socket_call(&fx, &blocked, "plugin_action", json!({"action": "list"}));
    assert_eq!(reply["ok"], true, "{reply}");
    let logs = logs(&fx);
    assert!(logs.contains("\"reason\":\"other_project\""), "{logs}");
    assert!(logs.contains("\"reason\":\"network_blocked\""), "{logs}");
    fx.shut_down().await;
}
