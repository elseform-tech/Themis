//! Background shell tasks through the real shared server, `themis call` and a
//! scripted provider: the agent starts and inspects a task, the person stops
//! it, chat deletion and server shutdown take it down, a restart reports it
//! as lost, and Stop kills a running blocking command.
#![cfg(unix)]

use std::path::Path;
use std::sync::Arc;
use std::time::{Duration, Instant};

use serde_json::{json, Value};
use themis_desktop::server::Server;
use themis_desktop::sink::{ChannelSink, EventSink, TestEvent};
use themis_desktop::state::AppState;
use themis_desktop::types::{ApprovalDecision, ProviderKind, ThreadEvent};
use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, Request, ResponseTemplate};

#[allow(dead_code)]
mod common;

/// One scripted step: a tool name plus arguments, which may depend on the
/// text of earlier tool results (to read the task id the agent was given).
type Step = Box<dyn Fn(&str) -> (&'static str, Value) + Send + Sync>;

fn step(name: &'static str, args: Value) -> Step {
    Box::new(move |_| (name, args.clone()))
}

fn task_id_in(text: &str) -> String {
    let start = text.find("bg_").expect("a task id in earlier tool output");
    text[start..start + 9].to_owned()
}

async fn mount_script(steps: Vec<Step>) -> MockServer {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/chat/completions"))
        .respond_with(move |request: &Request| {
            let body: Value = serde_json::from_slice(&request.body).unwrap_or(Value::Null);
            let tool_text: Vec<String> = body["messages"]
                .as_array()
                .map(|messages| {
                    messages
                        .iter()
                        .filter(|message| message["role"] == "tool")
                        .map(|message| message["content"].to_string())
                        .collect()
                })
                .unwrap_or_default();
            let payload = match steps.get(tool_text.len()) {
                Some(step) => {
                    let (name, args) = step(&tool_text.join("\n"));
                    json!({"choices":[{"message":{"role":"assistant","content":null,
                        "tool_calls":[{"id":format!("call_{}", tool_text.len()),"type":"function",
                        "function":{"name":name,"arguments":args.to_string()}}]}}]})
                }
                None => json!({"choices":[{"message":{"role":"assistant","content":"finished"},
                    "finish_reason":"stop"}]}),
            };
            ResponseTemplate::new(200).set_body_json(payload)
        })
        .mount(&server)
        .await;
    server
}

fn cli_call(data_dir: &Path, method: &str, args: Value) -> Value {
    let output = std::process::Command::new(env!("CARGO_BIN_EXE_themis"))
        .arg("--data-dir")
        .arg(data_dir)
        .env_remove("OPENCODE_KEY")
        .args(["call", method, &args.to_string()])
        .output()
        .expect("run CLI");
    assert!(
        output.status.success(),
        "CLI {method} failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    serde_json::from_slice(&output.stdout).unwrap_or(Value::Null)
}

fn alive(pid: u64) -> bool {
    std::process::Command::new("kill")
        .args(["-0", &pid.to_string()])
        .stderr(std::process::Stdio::null())
        .status()
        .is_ok_and(|status| status.success())
}

async fn eventually(what: &str, mut condition: impl FnMut() -> bool) {
    let deadline = Instant::now() + Duration::from_secs(20);
    while !condition() {
        assert!(Instant::now() < deadline, "timed out waiting for {what}");
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
}

struct Fixture {
    data: tempfile::TempDir,
    _repo: tempfile::TempDir,
    state: AppState,
    sink: Arc<ChannelSink>,
    rx: tokio::sync::mpsc::UnboundedReceiver<TestEvent>,
    thread: String,
    announced: usize,
    serving: Option<tokio::task::JoinHandle<Result<(), String>>>,
    _provider: MockServer,
}

async fn fixture(steps: Vec<Step>) -> Fixture {
    assert!(common::git_available(), "git is required");
    let data = tempfile::tempdir().unwrap();
    let (repo, project) = common::init_repo();
    let state = AppState::new_for_test(data.path().join("settings.json"));
    let provider = mount_script(steps).await;
    common::use_mock_llm(&state, &provider).await;
    let thread = state
        .create_thread(
            project.to_string_lossy().into(),
            ProviderKind::Go,
            Some("test-model".to_owned()),
        )
        .await
        .unwrap();
    let (sink, rx) = ChannelSink::channel();
    let sink = Arc::new(sink);
    // `themis call` talks to this in-process server; without it the CLI would
    // auto-start a detached `serve` process that outlives the test.
    let serving = Server::bind(state.clone(), data.path()).await.unwrap();
    let serving = tokio::spawn(serving.run());
    // Server::bind registered its own sink; route events to ours instead.
    state.set_scheduler_sink(Arc::clone(&sink) as Arc<dyn EventSink>);
    Fixture {
        data,
        _repo: repo,
        state,
        sink,
        rx,
        thread: thread.id,
        announced: 0,
        serving: Some(serving),
        _provider: provider,
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        if let Some(serving) = self.serving.take() {
            serving.abort();
        }
        if std::thread::panicking() {
            return;
        }
        // No detached `themis ... serve` may be left on this data directory.
        let leftover = std::process::Command::new("pgrep")
            .arg("-f")
            .arg(self.data.path().to_string_lossy().as_ref())
            .output()
            .map(|output| output.status.success())
            .unwrap_or(false);
        assert!(!leftover, "a server process was left running");
    }
}

impl Fixture {
    /// Sends a message and approves every prompt until the run ends. Returns
    /// whether the run finished (true) or failed (false).
    async fn run(&mut self, text: &str) -> bool {
        self.state
            .send_message(
                Arc::clone(&self.sink) as Arc<dyn EventSink>,
                self.thread.clone(),
                text.into(),
            )
            .await
            .unwrap();
        loop {
            let event = tokio::time::timeout(Duration::from_secs(60), self.rx.recv())
                .await
                .expect("run did not finish")
                .expect("sink closed");
            match event {
                TestEvent::Approval(request) => self
                    .state
                    .approve_action(
                        self.thread.clone(),
                        request.approval_id,
                        ApprovalDecision::Once,
                    )
                    .await
                    .unwrap(),
                TestEvent::Thread(envelope) => match envelope.event {
                    ThreadEvent::BackgroundTasksChanged => self.announced += 1,
                    ThreadEvent::Finished { .. } => return true,
                    ThreadEvent::Failed { .. } => return false,
                    _ => {}
                },
                _ => {}
            }
        }
    }

    /// Task-change announcements seen since the last call.
    fn changed_events(&mut self) -> usize {
        while let Ok(event) = self.rx.try_recv() {
            if matches!(event, TestEvent::Thread(envelope)
                if matches!(envelope.event, ThreadEvent::BackgroundTasksChanged) && envelope.run_id.is_empty())
            {
                self.announced += 1;
            }
        }
        std::mem::take(&mut self.announced)
    }

    fn background(&self) -> Vec<Value> {
        let feed = cli_call(
            self.data.path(),
            "get_thread_tasks",
            json!({"threadId": self.thread}),
        );
        feed["background_tasks"]
            .as_array()
            .cloned()
            .unwrap_or_default()
    }
}

fn start_step(script: &str) -> Step {
    step(
        "shell",
        json!({"command": "sh", "args": ["-c", script], "background": true}),
    )
}

#[tokio::test(flavor = "multi_thread")]
async fn agent_starts_inspects_and_the_person_stops_a_background_task() {
    let mut fx = fixture(vec![
        start_step("echo serving-on-8765; exec sleep 3140"),
        Box::new(|earlier| {
            (
                "background_task",
                json!({"action": "status", "task_id": task_id_in(earlier)}),
            )
        }),
    ])
    .await;
    assert!(fx.run("start a server").await);
    let tasks = fx.background();
    assert_eq!(tasks.len(), 1, "{tasks:?}");
    let task = &tasks[0];
    assert_eq!(task["state"], "running");
    let id = task["id"].as_str().unwrap().to_owned();
    let pid = task["pid"].as_u64().unwrap();
    assert!(alive(pid));
    eventually("output captured", || {
        fx.background()[0]["last_lines"] == json!(["serving-on-8765"])
    })
    .await;
    assert!(fx.changed_events() >= 1, "start must announce the change");

    let log = cli_call(
        fx.data.path(),
        "get_background_task_log",
        json!({"threadId": fx.thread, "taskId": id, "offset": 0, "maxBytes": 5}),
    );
    assert_eq!(log["text"], "servi");
    let log = cli_call(
        fx.data.path(),
        "get_background_task_log",
        json!({"threadId": fx.thread, "taskId": id}),
    );
    assert_eq!(log["text"], "serving-on-8765\n");

    // The run ended; the task outlives it.
    assert!(alive(pid));
    let stopped = cli_call(
        fx.data.path(),
        "stop_background_task",
        json!({"threadId": fx.thread, "taskId": id}),
    );
    assert_eq!(stopped["state"], "stopped");
    eventually("process gone", || !alive(pid)).await;
    assert!(fx.changed_events() >= 1, "stop must announce the change");
    assert_eq!(fx.background()[0]["state"], "stopped");

    let wrong = std::process::Command::new(env!("CARGO_BIN_EXE_themis"))
        .arg("--data-dir")
        .arg(fx.data.path())
        .env_remove("OPENCODE_KEY")
        .args([
            "call",
            "stop_background_task",
            &json!({"threadId": "nope", "taskId": id}).to_string(),
        ])
        .output()
        .unwrap();
    assert!(!wrong.status.success());

    cli_call(fx.data.path(), "shutdown", Value::Null);
    let _ = fx.serving.take().unwrap().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn deleting_the_chat_and_shutting_down_the_server_kill_their_tasks() {
    let mut fx = fixture(vec![start_step("exec sleep 3141")]).await;
    assert!(fx.run("start").await);
    let pid = fx.background()[0]["pid"].as_u64().unwrap();
    assert!(alive(pid));
    // Background tasks never block deleting the chat.
    fx.state.discard_thread(fx.thread.clone()).await.unwrap();
    eventually("task killed with the chat", || !alive(pid)).await;
    assert!(fx.state.get_thread_tasks(&fx.thread).await.is_err());

    // Server shutdown stops tasks of the remaining chats.
    let mut second = fixture(vec![start_step("exec sleep 3142")]).await;
    assert!(second.run("start").await);
    let pid = second.background()[0]["pid"].as_u64().unwrap();
    assert!(alive(pid));
    cli_call(second.data.path(), "shutdown", Value::Null);
    let _ = second.serving.take().unwrap().await;
    eventually("task killed with the server", || !alive(pid)).await;
    let reborn = AppState::new_for_test(second.data.path().join("settings.json"));
    let tasks = reborn
        .get_thread_tasks(&second.thread)
        .await
        .unwrap()
        .background_tasks;
    assert_eq!(tasks.len(), 1);
    assert_eq!(
        serde_json::to_value(&tasks[0]).unwrap()["state"],
        "stopped",
        "a graceful shutdown records the stop"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn a_restart_after_a_crash_reports_the_task_as_lost_never_running() {
    let mut fx = fixture(vec![start_step("exec sleep 3143")]).await;
    assert!(fx.run("start").await);
    let first = fx.background();
    let pid = first[0]["pid"].as_u64().unwrap();
    // A second state on the same data directory models a server that died
    // without cleaning up while the first one's task still runs.
    let reborn = AppState::new_for_test(fx.data.path().join("settings.json"));
    let tasks = reborn
        .get_thread_tasks(&fx.thread)
        .await
        .unwrap()
        .background_tasks;
    assert_eq!(tasks.len(), 1);
    let row = serde_json::to_value(&tasks[0]).unwrap();
    assert_eq!(row["state"], "lost");
    assert!(row["ended_at"].is_string());
    fx.state.shutdown_background_tasks().await;
    eventually("cleanup", || !alive(pid)).await;
}

#[tokio::test(flavor = "multi_thread")]
async fn stopping_a_run_kills_the_blocking_command_it_was_waiting_for() {
    let mut fx = fixture(vec![step(
        "shell",
        json!({"command": "sleep", "args": ["3144"]}),
    )])
    .await;
    let state = fx.state.clone();
    let thread = fx.thread.clone();
    let stopper = tokio::spawn(async move {
        // Wait until the command is really running, then press Stop.
        let deadline = Instant::now() + Duration::from_secs(30);
        loop {
            let running = std::process::Command::new("pgrep")
                .args(["-f", "sleep 3144"])
                .output()
                .map(|output| output.status.success())
                .unwrap_or(false);
            if running || Instant::now() > deadline {
                break;
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
        state.stop_thread(thread).await.unwrap();
    });
    let began = Instant::now();
    let finished = fx.run("wait a long time").await;
    stopper.await.unwrap();
    assert!(!finished, "the stopped run reports a failure");
    assert!(began.elapsed() < Duration::from_secs(40));
    eventually("sleep killed", || {
        !std::process::Command::new("pgrep")
            .args(["-f", "sleep 3144"])
            .output()
            .map(|output| output.status.success())
            .unwrap_or(false)
    })
    .await;
}
