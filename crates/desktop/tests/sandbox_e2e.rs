//! OS sandbox through the shared app server and the `themis` CLI, with a
//! scripted provider, isolated app data and synthetic credentials.
//!
//! Every run here uses this machine's real sandbox backend; where none is
//! usable (Windows, Linux without bubblewrap or Landlock) the tests that need
//! one are skipped with the reason.
#![cfg(unix)]

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use serde_json::{json, Value};
use themis_core::configuration::ApprovalMode;
use themis_desktop::server::Server;
use themis_desktop::sink::{ChannelSink, TestEvent};
use themis_desktop::state::AppState;
use themis_desktop::types::{ApprovalDecision, ApprovalRequest, ProviderKind, ThreadEvent};
use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, Request, ResponseTemplate};

#[allow(dead_code)]
mod common;

/// Folders outside the temporary folder (which the sandbox keeps writable).
struct Fixture {
    _dir: tempfile::TempDir,
    _repo: tempfile::TempDir,
    project: PathBuf,
    /// Listed in `sandbox.writable_paths`; holds the app data.
    writable: PathBuf,
    data: PathBuf,
    outside: PathBuf,
    home: PathBuf,
    state: AppState,
}

const SECRET: &str = "synthetic-secret-4711";

fn fixture() -> Fixture {
    let dir = tempfile::tempdir_in(env!("CARGO_TARGET_TMPDIR")).unwrap();
    let (repo, project) = common::init_repo();
    let base = dir.path().canonicalize().unwrap();
    let writable = base.join("writable");
    let data = writable.join("app-data");
    let outside = base.join("outside");
    let home = base.join("home");
    for folder in [&data, &outside, &home.join(".ssh")] {
        std::fs::create_dir_all(folder).unwrap();
    }
    std::fs::write(home.join(".ssh/id_test"), SECRET).unwrap();
    let state = AppState::new_for_test(data.join("settings.json"));
    state.set_sandbox_home_override(Some(home.clone()));
    Fixture {
        _dir: dir,
        _repo: repo,
        project: project.canonicalize().unwrap(),
        writable,
        data,
        outside,
        home,
        state,
    }
}

impl Fixture {
    fn save_user_config(&self, json: &str) {
        self.state
            .save_runtime_configuration(
                "user",
                Some(self.project.to_string_lossy().into_owned()),
                json,
            )
            .unwrap();
    }

    fn user_config(&self, extra: &str) -> String {
        format!(
            r#"{{"approval":{{"rules":[{{"tool":"shell","command":["sh"],"action":"allow"}},{{"tool":"*","risk":"read","action":"allow"}}]}},"sandbox":{{"writable_paths":[{}]{extra}}}}}"#,
            serde_json::to_string(&self.writable).unwrap()
        )
    }

    async fn sandbox_on(&self) -> bool {
        let view = self
            .state
            .get_effective_permissions(Some(self.project.to_string_lossy().into_owned()), None)
            .await
            .unwrap();
        let on = view["sandbox"]["state"] == "on";
        if !on {
            eprintln!(
                "skipping: no usable sandbox here ({})",
                view["sandbox"]["summary"]
            );
        }
        on
    }
}

/// Provider that issues one `sh -c SCRIPT` call per round, then answers.
async fn mount_scripts(scripts: Vec<String>) -> MockServer {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/chat/completions"))
        .respond_with(move |request: &Request| {
            let body: Value = serde_json::from_slice(&request.body).unwrap_or(Value::Null);
            let done = body["messages"]
                .as_array()
                .map(|m| m.iter().filter(|m| m["role"] == "tool").count())
                .unwrap_or(0);
            let payload = match scripts.get(done) {
                Some(script) => json!({"choices":[{"message":{"role":"assistant","content":null,
                    "tool_calls":[{"id":format!("call_{done}"),"type":"function","function":{
                        "name":"shell",
                        "arguments":json!({"command":"sh","args":["-c",script]}).to_string()}}]}}]}),
                None => json!({"choices":[{"message":{"role":"assistant","content":"finished"},
                    "finish_reason":"stop"}]}),
            };
            ResponseTemplate::new(200).set_body_json(payload)
        })
        .mount(&server)
        .await;
    server
}

struct Outcome {
    approvals: Vec<ApprovalRequest>,
    outputs: Vec<String>,
    failure: Option<String>,
    /// The system message of the run's first model request.
    system: String,
}

/// Runs one message in a new chat and answers approvals with `decide`.
async fn run(
    fx: &Fixture,
    mode: ApprovalMode,
    scripts: Vec<String>,
    decide: impl Fn(&ApprovalRequest) -> ApprovalDecision,
) -> (String, Outcome) {
    let provider = mount_scripts(scripts).await;
    common::use_mock_llm(&fx.state, &provider).await;
    let thread = fx
        .state
        .create_thread(
            fx.project.to_string_lossy().into_owned(),
            ProviderKind::Go,
            Some("test-model".to_owned()),
        )
        .await
        .unwrap();
    fx.state
        .set_thread_approval_mode(thread.id.clone(), mode)
        .await
        .unwrap();
    let mut outcome = send(fx, &thread.id, decide).await;
    outcome.system = provider
        .received_requests()
        .await
        .unwrap_or_default()
        .first()
        .and_then(|request| serde_json::from_slice::<Value>(&request.body).ok())
        .and_then(|body| body["messages"][0]["content"].as_str().map(str::to_owned))
        .unwrap_or_default();
    (thread.id, outcome)
}

async fn send(
    fx: &Fixture,
    thread_id: &str,
    decide: impl Fn(&ApprovalRequest) -> ApprovalDecision,
) -> Outcome {
    let (sink, mut rx) = ChannelSink::channel();
    let mut outcome = Outcome {
        approvals: Vec::new(),
        outputs: Vec::new(),
        failure: None,
        system: String::new(),
    };
    if let Err(error) = fx
        .state
        .send_message(
            Arc::new(sink),
            thread_id.to_owned(),
            "run the commands".into(),
        )
        .await
    {
        outcome.failure = Some(error);
        return outcome;
    }
    let finished = tokio::time::timeout(Duration::from_secs(90), async {
        while let Some(event) = rx.recv().await {
            match event {
                TestEvent::Approval(request) => {
                    let decision = decide(&request);
                    outcome.approvals.push(request.clone());
                    fx.state
                        .approve_action(thread_id.to_owned(), request.approval_id.clone(), decision)
                        .await
                        .unwrap();
                }
                TestEvent::Thread(envelope) => match envelope.event {
                    ThreadEvent::ToolFinished { tool, output, .. } if tool == "shell" => {
                        outcome.outputs.push(output)
                    }
                    ThreadEvent::Finished { .. } => break,
                    ThreadEvent::Failed { error, .. } => {
                        outcome.failure = Some(error);
                        break;
                    }
                    _ => {}
                },
                _ => {}
            }
        }
    })
    .await;
    assert!(finished.is_ok(), "run did not finish");
    outcome
}

fn quoted(path: &Path) -> String {
    format!("'{}'", path.display())
}

#[tokio::test(flavor = "multi_thread")]
async fn custom_run_is_confined_and_escalation_reruns_outside_the_sandbox() {
    if !common::git_available() {
        return;
    }
    let fx = fixture();
    if !fx.sandbox_on().await {
        return;
    }
    fx.save_user_config(&fx.user_config(""));
    let once = fx.outside.join("once.txt");
    let granted = fx.outside.join("granted.txt");
    let runtime = fx.data.join("runtime.jsonc");
    let before = std::fs::read_to_string(&runtime).unwrap();
    let grants = fx.data.join("chat_grants/forged.json");
    let scripts = vec![
        "echo inside > inside.txt".to_owned(),
        format!("echo once > {}", quoted(&once)),
        format!("cat {}", quoted(&fx.home.join(".ssh/id_test"))),
        format!(
            "echo '{{\"sandbox\":{{\"enabled\":false}}}}' > {}",
            quoted(&runtime)
        ),
        format!(
            "mkdir -p {} && echo '{{}}' > {}",
            quoted(grants.parent().unwrap()),
            quoted(&grants)
        ),
        format!("echo granted > {}", quoted(&granted)),
        format!("echo granted > {}", quoted(&granted)),
    ];
    let answers = Arc::new(Mutex::new(vec![
        ApprovalDecision::Once,
        ApprovalDecision::Deny,
        ApprovalDecision::Deny,
        ApprovalDecision::Deny,
        ApprovalDecision::Chat,
    ]));
    let (thread, outcome) = run(&fx, ApprovalMode::Custom, scripts, move |_| {
        let mut answers = answers.lock().unwrap();
        if answers.is_empty() {
            ApprovalDecision::Deny
        } else {
            answers.remove(0)
        }
    })
    .await;
    assert!(outcome.failure.is_none(), "{:?}", outcome.failure);
    assert!(
        fx.project.join("inside.txt").exists(),
        "writes inside the project succeed"
    );
    // Every prompt was the escalation, with the blocked reason and the argv.
    assert_eq!(outcome.approvals.len(), 5, "{:#?}", outcome.approvals);
    for request in &outcome.approvals {
        assert_eq!(request.tool, "shell_unsandboxed");
        let command = request.command.as_ref().unwrap();
        assert_eq!(command.argv[0], "sh");
        assert!(command.sandbox_blocked.is_some(), "{request:?}");
    }
    assert!(outcome.approvals[1]
        .command
        .as_ref()
        .unwrap()
        .sandbox_blocked
        .as_ref()
        .unwrap()
        .contains("secret"));
    // Allow once reran it outside the sandbox.
    assert_eq!(std::fs::read_to_string(&once).unwrap().trim(), "once");
    // Denied escalations kept the sandbox's answer.
    assert!(outcome
        .outputs
        .iter()
        .all(|output| !output.contains(SECRET)));
    assert_eq!(std::fs::read_to_string(&runtime).unwrap(), before);
    assert!(!grants.exists());
    // "Allow in this chat" ran it and covered the identical later call.
    assert_eq!(std::fs::read_to_string(&granted).unwrap().trim(), "granted");
    let view = fx
        .state
        .get_effective_permissions(None, Some(thread.clone()))
        .await
        .unwrap();
    let stored = &view["chat"]["grants"];
    assert_eq!(stored[0]["tool"], "shell_unsandboxed", "{view:#}");
    assert!(view["chat"]["grants_path"]
        .as_str()
        .unwrap()
        .ends_with(&format!("{thread}.json")));
}

/// YOLO has no walls but Themis' own settings: commands write outside the project and
/// read secret locations, never prompt, and still cannot write the app data or the
/// project's `.themis/` (Landlock cannot carve `.themis/` out; the file tools cover it
/// there). The run's system prompt carries the YOLO caution.
#[tokio::test(flavor = "multi_thread")]
async fn yolo_has_no_walls_but_themis_own_settings() {
    if !common::git_available() {
        return;
    }
    let fx = fixture();
    if !fx.sandbox_on().await {
        return;
    }
    fx.save_user_config(&fx.user_config(""));
    let target = fx.outside.join("yolo.txt");
    let config = fx.data.join("runtime.jsonc");
    let before = std::fs::read_to_string(&config).unwrap_or_default();
    std::fs::create_dir_all(fx.project.join(".themis")).unwrap();
    let local = fx.project.join(".themis/config.jsonc");
    let scripts = vec![
        "echo inside > yolo-inside.txt".to_owned(),
        format!("echo x > {}", quoted(&target)),
        format!("cat {}", quoted(&fx.home.join(".ssh/id_test"))),
        format!("echo '{{}}' > {}", quoted(&config)),
        format!("echo '{{}}' > {}", quoted(&local)),
    ];
    let (_, outcome) = run(&fx, ApprovalMode::Yolo, scripts, |_| ApprovalDecision::Once).await;
    assert!(outcome.failure.is_none(), "{:?}", outcome.failure);
    assert!(
        outcome.approvals.is_empty(),
        "YOLO never asks: {:?}",
        outcome.approvals
    );
    assert!(fx.project.join("yolo-inside.txt").exists());
    assert!(target.exists(), "writes outside the project work");
    assert_eq!(outcome.outputs.len(), 5);
    assert!(
        outcome.outputs[2].contains(SECRET),
        "secret locations are readable"
    );
    assert_eq!(
        std::fs::read_to_string(&config).unwrap_or_default(),
        before,
        "the app data stays read-only"
    );
    assert!(
        outcome.outputs[3].contains("Themis sandbox blocked this command"),
        "{}",
        outcome.outputs[3]
    );
    let landlock = fx
        .state
        .get_effective_permissions(Some(fx.project.to_string_lossy().into_owned()), None)
        .await
        .unwrap()["sandbox"]["backend"]
        == "Landlock";
    if !landlock {
        assert!(
            !local.exists(),
            "the project's .themis/ stays read-only: {}",
            outcome.outputs[4]
        );
    }
    assert!(
        outcome.system.contains("YOLO mode:"),
        "the caution is in the system prompt"
    );
}

/// A one-shot HTTP server on localhost; returns its port.
fn http_server() -> u16 {
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    std::thread::spawn(move || {
        for mut stream in listener.incoming().flatten().take(4) {
            use std::io::{Read, Write};
            let mut buffer = [0u8; 1024];
            let _ = stream.read(&mut buffer);
            let _ = stream.write_all(
                b"HTTP/1.1 200 OK\r\nContent-Length: 7\r\nConnection: close\r\n\r\nfetched",
            );
        }
    });
    port
}

#[tokio::test(flavor = "multi_thread")]
async fn network_is_allowed_by_default_and_blocked_by_configuration() {
    if !common::git_available()
        || themis_core::sandbox::find_program("curl", std::env::var_os("PATH").as_deref()).is_none()
    {
        return;
    }
    let fx = fixture();
    if !fx.sandbox_on().await {
        return;
    }
    let status = themis_core::sandbox::availability(themis_core::sandbox::Backend::Landlock);
    let port = http_server();
    let fetch = format!("curl -sS -m 5 http://127.0.0.1:{port}/");
    fx.save_user_config(&fx.user_config(""));
    // Custom (the rules allow `sh`): YOLO's open plan ignores `network`.
    let (_, allowed) = run(&fx, ApprovalMode::Custom, vec![fetch.clone()], |_| {
        ApprovalDecision::Deny
    })
    .await;
    assert!(
        !allowed.system.contains("YOLO mode:"),
        "only YOLO runs carry the caution"
    );
    assert!(
        allowed.outputs[0].contains("fetched"),
        "{:?}",
        allowed.outputs
    );
    fx.save_user_config(&fx.user_config(r#","network":"block""#));
    let view = fx
        .state
        .get_effective_permissions(Some(fx.project.to_string_lossy().into_owned()), None)
        .await
        .unwrap();
    if view["sandbox"]["network_enforced"] != true {
        eprintln!(
            "skipping the block half: {} ({})",
            view["sandbox"]["network_note"], status.detail
        );
        return;
    }
    let (_, blocked) = run(&fx, ApprovalMode::Custom, vec![fetch], |_| {
        ApprovalDecision::Deny
    })
    .await;
    assert!(
        !blocked.outputs[0].contains("fetched"),
        "{:?}",
        blocked.outputs
    );
    assert!(
        blocked.outputs[0].contains("network access is blocked"),
        "{:?}",
        blocked.outputs
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn project_configuration_cannot_disable_the_sandbox() {
    if !common::git_available() {
        return;
    }
    let fx = fixture();
    fx.save_user_config(&fx.user_config(""));
    std::fs::create_dir_all(fx.project.join(".themis")).unwrap();
    std::fs::write(
        fx.project.join(".themis/config.jsonc"),
        "{\n  // trying to escape\n  \"sandbox\": {\n    \"enabled\": false\n  }\n}\n",
    )
    .unwrap();
    let (_, outcome) = run(
        &fx,
        ApprovalMode::Yolo,
        vec!["echo hi > ran.txt".into()],
        |_| ApprovalDecision::Deny,
    )
    .await;
    let failure = outcome.failure.expect("the run is refused");
    assert!(failure.contains("cannot disable the sandbox"), "{failure}");
    assert!(!fx.project.join("ran.txt").exists());
    let view = fx
        .state
        .get_effective_permissions(Some(fx.project.to_string_lossy().into_owned()), None)
        .await
        .unwrap();
    let project_file = &view["files"][1];
    assert_eq!(project_file["status"], "invalid", "{view:#}");
    assert_eq!(project_file["error"]["line"], 4, "{view:#}");
    assert!(view["validation_error"]
        .as_str()
        .unwrap()
        .contains("cannot disable the sandbox"));
}

#[tokio::test(flavor = "multi_thread")]
async fn a_named_backend_that_is_unavailable_fails_closed_and_auto_does_not() {
    if !common::git_available() {
        return;
    }
    let fx = fixture();
    let backend = if cfg!(target_os = "macos") {
        "seatbelt"
    } else {
        "bubblewrap"
    };
    fx.state.set_sandbox_backends_unavailable(true);
    fx.save_user_config(&fx.user_config(&format!(r#","backend":"{backend}""#)));
    let (_, refused) = run(
        &fx,
        ApprovalMode::Yolo,
        vec!["echo hi > refused.txt".into()],
        |_| ApprovalDecision::Deny,
    )
    .await;
    assert!(refused.failure.is_none(), "{:?}", refused.failure);
    assert!(!fx.project.join("refused.txt").exists());
    assert!(
        refused.outputs[0].contains(backend),
        "{:?}",
        refused.outputs
    );
    let view = fx
        .state
        .get_effective_permissions(Some(fx.project.to_string_lossy().into_owned()), None)
        .await
        .unwrap();
    assert_eq!(view["sandbox"]["state"], "blocked", "{view:#}");
    fx.save_user_config(&fx.user_config(""));
    let (_, auto) = run(
        &fx,
        ApprovalMode::Yolo,
        vec!["echo hi > auto.txt".into()],
        |_| ApprovalDecision::Deny,
    )
    .await;
    assert!(auto.failure.is_none(), "{:?}", auto.failure);
    assert!(fx.project.join("auto.txt").exists(), "auto never blocks");
    let view = fx
        .state
        .get_effective_permissions(Some(fx.project.to_string_lossy().into_owned()), None)
        .await
        .unwrap();
    assert_eq!(view["sandbox"]["state"], "unavailable");
    assert!(view["sandbox"]["summary"]
        .as_str()
        .unwrap()
        .contains("Not available on this system"));
}

fn cli(data_dir: &Path, args: &[&str]) -> (bool, String, String) {
    let output = std::process::Command::new(env!("CARGO_BIN_EXE_themis"))
        .arg("--data-dir")
        .arg(data_dir)
        .args(args)
        .env_remove("OPENCODE_KEY")
        .stdin(std::process::Stdio::null())
        .output()
        .unwrap();
    (
        output.status.success(),
        String::from_utf8_lossy(&output.stdout).into_owned(),
        String::from_utf8_lossy(&output.stderr).into_owned(),
    )
}

#[tokio::test(flavor = "multi_thread")]
async fn cli_permissions_view_matches_the_shared_server() {
    if !common::git_available() {
        return;
    }
    let fx = fixture();
    fx.save_user_config(&fx.user_config(""));
    std::fs::create_dir_all(fx.project.join(".themis")).unwrap();
    std::fs::write(
        fx.project.join(".themis/config.jsonc"),
        r#"{"approval":{"rules":[{"tool":"shell","command":["git","push"],"action":"deny"}]},"sandbox":{"network":"block"}}"#,
    )
    .unwrap();
    let thread = fx
        .state
        .create_thread(
            fx.project.to_string_lossy().into_owned(),
            ProviderKind::Go,
            Some("test-model".into()),
        )
        .await
        .unwrap();
    let expected = fx
        .state
        .get_effective_permissions(
            Some(fx.project.to_string_lossy().into_owned()),
            Some(thread.id.clone()),
        )
        .await
        .unwrap();
    let server = Server::bind(fx.state.clone(), &fx.data).await.unwrap();
    let task = tokio::spawn(server.run());
    let root = fx.project.to_string_lossy().into_owned();
    let (ok, stdout, stderr) = tokio::task::spawn_blocking({
        let data = fx.data.clone();
        let root = root.clone();
        let thread = thread.id.clone();
        move || {
            cli(
                &data,
                &["permissions", &root, "--thread", &thread, "--json"],
            )
        }
    })
    .await
    .unwrap();
    assert!(ok, "{stderr}");
    let printed: Value = serde_json::from_str(&stdout).unwrap();
    assert_eq!(printed, expected);
    assert_eq!(printed["approval"]["project_rule_count"], 1);
    assert_eq!(printed["approval"]["rules"][0]["source"], "project");
    assert_eq!(printed["sandbox"]["network"], "block");
    assert_eq!(printed["sandbox"]["sources"]["network"], "project");
    let (ok, text, stderr) = tokio::task::spawn_blocking({
        let data = fx.data.clone();
        let thread = thread.id.clone();
        move || cli(&data, &["permissions", &root, "--thread", &thread])
    })
    .await
    .unwrap();
    assert!(ok, "{stderr}");
    assert!(text.contains("+1 deny rules from this project"), "{text}");
    assert!(text.contains("[Project file]"), "{text}");
    assert!(text.contains("Sandbox: "), "{text}");
    assert!(text.contains("Network: block"), "{text}");
    assert!(text.contains("~/.ssh"), "{text}");
    assert!(text.contains("Allowed in this chat"), "{text}");
    assert!(
        text.contains(&fx.data.join("runtime.jsonc").display().to_string()),
        "{text}"
    );
    task.abort();
}

#[tokio::test(flavor = "multi_thread")]
async fn cli_ensure_runtime_config_writes_the_starter_once() {
    let fx = fixture();
    let path = fx.data.join("runtime.jsonc");
    assert!(!path.exists());
    let server = Server::bind(fx.state.clone(), &fx.data).await.unwrap();
    let task = tokio::spawn(server.run());
    let call = |data: PathBuf| move || cli(&data, &["call", "ensure_runtime_config", "{}"]);
    let (ok, stdout, stderr) = tokio::task::spawn_blocking(call(fx.data.clone()))
        .await
        .unwrap();
    assert!(ok, "{stderr}");
    let printed: Value = serde_json::from_str(&stdout).unwrap();
    assert_eq!(printed, json!(path.display().to_string()));
    let starter = std::fs::read_to_string(&path).unwrap();
    assert!(starter.contains("// \"sandbox\": {"), "{starter}");
    let view = fx
        .state
        .get_effective_permissions(None, None)
        .await
        .unwrap();
    assert_eq!(view["files"][0]["status"], "loaded");
    std::fs::write(&path, r#"{"sandbox":{"network":"block"}}"#).unwrap();
    let (ok, _, stderr) = tokio::task::spawn_blocking(call(fx.data.clone()))
        .await
        .unwrap();
    assert!(ok, "{stderr}");
    assert_eq!(
        std::fs::read_to_string(&path).unwrap(),
        r#"{"sandbox":{"network":"block"}}"#
    );
    task.abort();
}
