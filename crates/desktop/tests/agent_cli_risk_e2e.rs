//! `themis` calls in an agent's shell at the approval step, through the real app server and a
//! scripted provider: a read runs without a prompt, a change asks once and "Allow in this chat"
//! covers the same command, switching on a hook asks every time and never gains a chat grant,
//! the agent's saves keep a new hook switched off, and a deny rule turns `themis` off in YOLO.
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
    // The person allowed switching it on once; the last list shows it on.
    let listed = command_json(&outputs[7]);
    assert_eq!(listed[0]["spec"]["hooks"][0]["enabled"], true, "{listed}");
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
