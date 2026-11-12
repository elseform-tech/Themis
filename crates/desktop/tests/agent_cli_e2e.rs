//! The `themis` CLI from an agent's shell: run credentials, the server's guard on what they may
//! call, and a real run (scripted provider) using `themis` on its `PATH`. In-process app server,
//! isolated app data, synthetic credentials only.
#![cfg(unix)]

use std::path::{Path, PathBuf};
use std::process::Output;
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

    fn port(&self) -> u16 {
        let text = std::fs::read_to_string(self.data.join("server.json")).unwrap();
        serde_json::from_str::<Value>(&text).unwrap()["port"]
            .as_u64()
            .unwrap() as u16
    }

    fn person_token(&self) -> String {
        let text = std::fs::read_to_string(self.data.join("server.json")).unwrap();
        serde_json::from_str::<Value>(&text).unwrap()["token"]
            .as_str()
            .unwrap()
            .to_owned()
    }

    /// `themis` as the person (discovery file) or with an agent's credential.
    fn cli(&self, agent: Option<&str>, words: &[&str]) -> Output {
        let mut command = std::process::Command::new(env!("CARGO_BIN_EXE_themis"));
        command
            .arg("--data-dir")
            .arg(&self.data)
            .args(words)
            .env_remove("OPENCODE_KEY")
            .env_remove(themis_core::agent_cli::TOKEN_ENV)
            .env_remove(themis_core::agent_cli::PORT_ENV);
        if let Some(token) = agent {
            command
                .env(themis_core::agent_cli::TOKEN_ENV, token)
                .env(themis_core::agent_cli::PORT_ENV, self.port().to_string());
        }
        command.output().expect("run CLI")
    }

    async fn thread(&self) -> String {
        self.state
            .create_thread(
                self.project.to_string_lossy().into_owned(),
                ProviderKind::Go,
                Some("test-model".to_owned()),
            )
            .await
            .unwrap()
            .id
    }
}

fn text(output: &Output) -> String {
    format!(
        "{}{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    )
}

fn refused(output: &Output, method: &str) -> bool {
    !output.status.success() && text(output).contains(&format!("Themis refused `{method}`"))
}

/// Each refused area, with a call an agent's credential must not make.
fn refused_calls(thread: &str, other: &str, project: &str) -> Vec<(&'static str, Value)> {
    vec![
        (
            "approve_action",
            json!({"threadId": thread, "approvalId": "a1", "decision": "once"}),
        ),
        (
            "answer_question",
            json!({"threadId": thread, "questionId": "q1", "answer": "yes"}),
        ),
        (
            "set_thread_approval_mode",
            json!({"threadId": thread, "mode": "yolo"}),
        ),
        (
            "save_runtime_configuration",
            json!({"scope": "user", "content": "{}"}),
        ),
        ("ensure_runtime_config", json!({})),
        ("open_runtime_config", json!({})),
        ("update_settings", json!({"patch": {}})),
        ("update_browser_settings", json!({})),
        ("set_provider", json!({"provider": "go"})),
        ("clear_secret", json!({"provider": "go"})),
        ("list_keys", json!({"projectRoot": project})),
        ("check_go_key", json!({})),
        ("terminal_open", json!({"root": project})),
        ("terminal_input", json!({"root": project, "data": "ls\n"})),
        ("create_project", json!({"name": "new"})),
        ("delete_project", json!({"projectRoot": project})),
        ("send_message", json!({"threadId": other, "text": "hi"})),
        ("stop_thread", json!({"threadId": other})),
        ("discard_thread", json!({"threadId": other})),
        ("switch_branch", json!({"threadId": thread, "branch": "x"})),
        ("accept_file", json!({"threadId": thread, "path": "a"})),
        ("dismiss_review_item", json!({"id": "r1"})),
        (
            "browser_navigate",
            json!({"threadId": thread, "url": "https://example.com"}),
        ),
        ("clear_browsing_data", json!({})),
        ("shutdown", json!({})),
        ("delete_memory", json!({"name": "m"})),
        ("subscribe", json!({})),
        ("no_such_method", json!({})),
        // An allowed reading method, but for another chat.
        ("get_thread_tasks", json!({"threadId": other})),
    ]
}

#[tokio::test(flavor = "multi_thread")]
async fn an_agent_credential_is_refused_the_persons_methods_which_still_work_without_it() {
    let _turn = ONE_SERVER.lock().await;
    let fx = fixture().await;
    let thread = fx.thread().await;
    let other = fx.thread().await;
    let project = fx.project.to_string_lossy().into_owned();
    let scope = themis_core::agent_cli::RunScope {
        project_root: Some(fx.project.clone()),
        network_blocked: false,
        reads: Vec::new(),
        vision: None,
    };
    let access = themis_core::agent_cli::issue(&thread, "run-under-test", &scope).expect("issued");
    let token = access.env()[0].1.clone();

    // Approvals and the permission mode first: refused for the agent, working for the person.
    let mode = json!({"threadId": thread, "mode": "yolo"}).to_string();
    let as_agent = fx.cli(Some(&token), &["call", "set_thread_approval_mode", &mode]);
    assert!(
        refused(&as_agent, "set_thread_approval_mode"),
        "{}",
        text(&as_agent)
    );
    let as_person = fx.cli(None, &["call", "set_thread_approval_mode", &mode]);
    assert!(as_person.status.success(), "{}", text(&as_person));
    let approve = json!({"threadId": thread, "approvalId": "a1", "decision": "once"}).to_string();
    let as_agent = fx.cli(Some(&token), &["call", "approve_action", &approve]);
    assert!(refused(&as_agent, "approve_action"), "{}", text(&as_agent));
    let as_person = fx.cli(None, &["call", "approve_action", &approve]);
    assert!(
        !text(&as_person).contains("Themis refused"),
        "{}",
        text(&as_person)
    );

    for (method, args) in refused_calls(&thread, &other, &project) {
        let output = fx.cli(Some(&token), &["call", method, &args.to_string()]);
        assert!(refused(&output, method), "{method}: {}", text(&output));
    }
    // The CLI's own shortcuts reach the same guard.
    let stop = fx.cli(Some(&token), &["server", "stop"]);
    assert!(refused(&stop, "shutdown"), "{}", text(&stop));
    let quit = fx.cli(Some(&token), &["app", "quit"]);
    assert!(text(&quit).contains("cannot quit"), "{}", text(&quit));
    let sign_in = json!({"action": "mcp_sign_in", "projectRoot": project, "url": "https://x"});
    let output = fx.cli(
        Some(&token),
        &["call", "plugin_action", &sign_in.to_string()],
    );
    assert!(
        refused(&output, "plugin_action mcp_sign_in"),
        "{}",
        text(&output)
    );
    // The person's calls are unchanged: the person's own `get_thread_tasks` for any chat.
    let tasks = json!({"threadId": other}).to_string();
    let output = fx.cli(None, &["call", "get_thread_tasks", &tasks]);
    assert!(output.status.success(), "{}", text(&output));

    // What the agent may do still works with its credential.
    let list = fx.cli(Some(&token), &["plugin", "list"]);
    assert!(list.status.success(), "{}", text(&list));
    let own = json!({"threadId": thread}).to_string();
    let output = fx.cli(Some(&token), &["call", "get_thread_tasks", &own]);
    assert!(output.status.success(), "{}", text(&output));
    let status = fx.cli(Some(&token), &["status"]);
    assert!(status.status.success(), "{}", text(&status));

    // A credential nobody issued is no one's.
    let forged = fx.cli(Some("forged-token"), &["plugin", "list"]);
    assert!(text(&forged).contains("unauthorized"), "{}", text(&forged));

    // Refusals are logged as metadata: the method, never the credential or the arguments.
    let logs = std::fs::read_dir(fx.data.join("logs"))
        .unwrap()
        .filter_map(Result::ok)
        .map(|entry| std::fs::read_to_string(entry.path()).unwrap_or_default())
        .collect::<String>();
    assert!(logs.contains("agent_call_refused"), "{logs}");
    assert!(logs.contains("\"method\":\"approve_action\""), "{logs}");
    assert!(!logs.contains(&token));
    assert!(!logs.contains("synthetic"));
    fx.shut_down().await;
}

/// A provider that runs one `sh -c SCRIPT` call per round, then answers.
async fn mount_scripts(scripts: Vec<String>) -> MockServer {
    let server = MockServer::builder().start().await;
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

/// Sends one message and returns each shell output; any approval (a sandbox escape) is denied.
async fn run(fx: &Fixture, thread: &str) -> Vec<String> {
    let (sink, mut rx) = ChannelSink::channel();
    fx.state
        .send_message(Arc::new(sink), thread.to_owned(), "run the commands".into())
        .await
        .unwrap();
    let mut outputs = Vec::new();
    let finished = tokio::time::timeout(Duration::from_secs(90), async {
        while let Some(event) = rx.recv().await {
            match event {
                TestEvent::Approval(request) => fx
                    .state
                    .approve_action(
                        thread.to_owned(),
                        request.approval_id.clone(),
                        ApprovalDecision::Deny,
                    )
                    .await
                    .unwrap(),
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
    outputs
}

fn quoted(path: &Path) -> String {
    format!("'{}'", path.display())
}

#[tokio::test(flavor = "multi_thread")]
async fn a_runs_shell_uses_the_apps_themis_and_its_credential_ends_with_the_run() {
    if !common::git_available() {
        return;
    }
    let _turn = ONE_SERVER.lock().await;
    let fx = fixture().await;
    fx.state
        .save_runtime_configuration(
            "user",
            Some(fx.project.to_string_lossy().into_owned()),
            r#"{"approval":{"rules":[{"tool":"shell","command":["sh"],"action":"allow"},{"tool":"*","risk":"read","action":"allow"}]}}"#,
        )
        .unwrap();
    let saved = tempfile::NamedTempFile::new().unwrap();
    let scripts = vec![
        "command -v themis".to_owned(),
        "themis plugin list".to_owned(),
        format!(
            "echo \"token:$THEMIS_AGENT_TOKEN\"; printf %s \"$THEMIS_AGENT_TOKEN\" > {}",
            quoted(saved.path())
        ),
        r#"themis call approve_action '{"threadId":"t","approvalId":"a","decision":"once"}'"#
            .to_owned(),
        format!("cat {}", quoted(&fx.data.join("server.json"))),
    ];
    let provider = mount_scripts(scripts).await;
    common::use_mock_llm(&fx.state, &provider).await;
    let thread = fx.thread().await;
    fx.state
        .set_thread_approval_mode(thread.clone(), ApprovalMode::Custom)
        .await
        .unwrap();
    let outputs = run(&fx, &thread).await;
    assert_eq!(outputs.len(), 5, "{outputs:#?}");

    // The app's own `themis` comes first, from the app-data folder agents cannot write.
    let link = fx.data.join("agent-cli/themis");
    assert!(
        outputs[0].contains(&link.display().to_string()),
        "{}",
        outputs[0]
    );
    assert_eq!(
        std::fs::read_link(&link).unwrap(),
        PathBuf::from(env!("CARGO_BIN_EXE_themis"))
            .canonicalize()
            .unwrap()
    );
    // Reading integrations works through the live server.
    let listed: Value = serde_json::from_str(&outputs[1]).unwrap();
    assert_eq!(listed["exit_code"], 0, "{}", outputs[1]);
    assert_eq!(listed["output"], "[]\n", "{}", outputs[1]);
    // The credential never reaches the model's tool output (or the transcript after it).
    let token = std::fs::read_to_string(saved.path()).unwrap();
    assert!(token.len() >= 64, "the shell got a credential");
    assert!(outputs[2].contains("token:[redacted]"), "{}", outputs[2]);
    assert!(outputs.iter().all(|output| !output.contains(&token)));
    // Approving itself is refused.
    assert!(
        outputs[3].contains("Themis refused `approve_action`"),
        "{}",
        outputs[3]
    );
    // The person's credential is out of reach where the sandbox runs.
    let person = fx.person_token();
    let sandbox = fx
        .state
        .get_effective_permissions(Some(fx.project.to_string_lossy().into_owned()), None)
        .await
        .unwrap();
    if sandbox["sandbox"]["state"] == "on" {
        assert!(!outputs[4].contains(&person), "{}", outputs[4]);
        // The read fails however the platform words it: Seatbelt says "Operation not
        // permitted", bubblewrap and Landlock "Permission denied".
        let read: Value = serde_json::from_str(&outputs[4]).unwrap();
        assert_ne!(read["exit_code"], 0, "{}", outputs[4]);
        assert!(
            ["Operation not permitted", "Permission denied"]
                .iter()
                .any(|refusal| outputs[4].contains(refusal)),
            "{}",
            outputs[4]
        );
    } else {
        eprintln!("sandbox not usable here: server.json read wall unverified");
    }

    // After the run, its credential is no one's.
    let after = fx.cli(Some(&token), &["plugin", "list"]);
    assert!(!after.status.success());
    assert!(text(&after).contains("unauthorized"), "{}", text(&after));
    fx.shut_down().await;
}
