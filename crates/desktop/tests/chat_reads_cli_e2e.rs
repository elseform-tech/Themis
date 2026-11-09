//! `themis recall` and `themis memory search|show` through the real app server: the person's
//! CLI names any chat; an agent's credential reaches only its own chat, only the project that
//! chat works in, and only the read tools its run has; a real run (scripted provider) uses them
//! from its shell with no approval, and a run without the tools is refused. In-process app
//! server, isolated app data, synthetic credentials only.
#![cfg(unix)]

use std::path::{Path, PathBuf};
use std::process::Output;
use std::sync::Arc;
use std::time::Duration;

use serde_json::{json, Value};
use themis_core::configuration::ApprovalMode;
use themis_core::memory::{Memory, MemoryKind, MemoryStore, SaveMode};
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

const CODE: &str = "MARLIN-5521";
const OTHER_CODE: &str = "OSPREY-7730";

struct Fixture {
    _dir: tempfile::TempDir,
    _repos: Vec<tempfile::TempDir>,
    data: PathBuf,
    project: PathBuf,
    other_project: PathBuf,
    state: AppState,
    stop: Option<tokio::sync::oneshot::Sender<()>>,
    served: Option<tokio::task::JoinHandle<Result<(), String>>>,
}

async fn fixture() -> Fixture {
    let dir = tempfile::tempdir_in(env!("CARGO_TARGET_TMPDIR")).unwrap();
    let data = dir.path().canonicalize().unwrap().join("app-data");
    std::fs::create_dir_all(&data).unwrap();
    let (repo, project) = common::init_repo();
    let (other_repo, other_project) = common::init_repo();
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
        _repos: vec![repo, other_repo],
        data,
        project: project.canonicalize().unwrap(),
        other_project: other_project.canonicalize().unwrap(),
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

    async fn thread_in(&self, project: &Path) -> String {
        self.state
            .create_thread(
                project.to_string_lossy().into_owned(),
                ProviderKind::Go,
                Some("test-model".to_owned()),
            )
            .await
            .unwrap()
            .id
    }

    /// A credential for `run` of `thread`, bound to `project` with the given read tools.
    fn token(&self, thread: &str, run: &str, project: &Path, reads: &[&str]) -> String {
        let scope =
            themis_core::agent_cli::RunScope::new(project, None).with_reads(reads.iter().copied());
        let access = themis_core::agent_cli::issue(thread, run, &scope).expect("issued");
        access.env()[0].1.clone()
    }
}

/// A saved original in `thread`'s evidence directory, as compaction leaves it.
fn save_original(project: &Path, thread: &str, text: &str) {
    let evidence = project.join(".themis/context").join(thread);
    std::fs::create_dir_all(&evidence).unwrap();
    let line = json!({"role": "User", "content": text}).to_string();
    std::fs::write(evidence.join("snapshot-1.jsonl"), format!("{line}\n")).unwrap();
}

/// A project memory, as the memory models save it.
fn remember(data: &Path, project: &Path, name: &str, description: &str, body: &str) {
    let store = MemoryStore::new(
        &data.join("memory"),
        &themis_core::memory::project_folder(project),
    )
    .unwrap();
    let memory = Memory {
        name: name.into(),
        description: description.into(),
        kind: MemoryKind::Project,
        saved: "2026-11-09".into(),
        source: None,
        body: body.into(),
    };
    store.save(&memory, SaveMode::Create).unwrap();
}

fn text(output: &Output) -> String {
    format!(
        "{}{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    )
}

fn json_of(output: &Output) -> Value {
    assert!(output.status.success(), "{}", text(output));
    serde_json::from_slice(&output.stdout).unwrap_or_else(|_| panic!("{}", text(output)))
}

fn refused(output: &Output, method: &str) -> bool {
    !output.status.success() && text(output).contains(&format!("Themis refused `{method}`"))
}

fn unavailable(output: &Output, method: &str) -> bool {
    refused(output, method) && text(output).contains("does not have that tool")
}

#[tokio::test(flavor = "multi_thread")]
async fn recall_and_memory_reach_only_the_agents_own_chat_and_its_runs_tools() {
    let _turn = ONE_SERVER.lock().await;
    let fx = fixture().await;
    let own = fx.thread_in(&fx.project).await;
    let other = fx.thread_in(&fx.other_project).await;
    save_original(
        &fx.project,
        &own,
        &format!("The dock code for the north pier is {CODE}."),
    );
    save_original(
        &fx.other_project,
        &other,
        &format!("The dock code for the south pier is {OTHER_CODE}."),
    );
    remember(
        &fx.data,
        &fx.project,
        "release-steps",
        "How releases are cut here",
        "Tag, then publish the notes.",
    );
    remember(
        &fx.data,
        &fx.other_project,
        "other-release",
        "How releases are cut in the other project",
        "Never seen from the first project.",
    );

    // The person's own CLI names the chat.
    let found = json_of(&fx.cli(
        None,
        &["recall", "--thread", &own, "dock", "code", "north", "pier"],
    ));
    assert!(found["hits"].to_string().contains(CODE), "{found}");
    assert_eq!(found["untrusted"], true);
    let found = json_of(&fx.cli(None, &["recall", "--thread", &other, "dock", "code"]));
    assert!(found["hits"].to_string().contains(OTHER_CODE), "{found}");
    let missing = fx.cli(None, &["recall", "dock"]);
    assert!(
        text(&missing).contains("pass --thread ID"),
        "{}",
        text(&missing)
    );
    let searched = json_of(&fx.cli(None, &["memory", "search", "--thread", &own, "releases"]));
    let lines = searched["matches"].to_string();
    assert!(lines.contains("release-steps"), "{searched}");
    assert!(
        !lines.contains("other-release"),
        "another project's memory: {searched}"
    );
    let shown = json_of(&fx.cli(None, &["memory", "show", "--thread", &own, "release-steps"]));
    assert!(
        shown["text"]
            .as_str()
            .unwrap()
            .contains("publish the notes"),
        "{shown}"
    );

    // An agent's credential: its own chat with no `--thread`, the same answers.
    let agent = fx.token(&own, "run-1", &fx.project, &["recall", "search_memory"]);
    let found = json_of(&fx.cli(Some(&agent), &["recall", "dock", "code", "north", "pier"]));
    assert!(found["hits"].to_string().contains(CODE), "{found}");
    assert!(!found.to_string().contains(OTHER_CODE), "{found}");
    let around = found["hits"][0]["around"].as_str().unwrap().to_owned();
    let expanded = json_of(&fx.cli(Some(&agent), &["recall", "--around", &around]));
    assert!(expanded.to_string().contains(CODE), "{expanded}");
    let searched = json_of(&fx.cli(Some(&agent), &["memory", "search", "releases"]));
    assert!(
        searched["matches"].to_string().contains("release-steps"),
        "{searched}"
    );
    assert!(
        !searched.to_string().contains("other-release"),
        "{searched}"
    );
    let shown = fx.cli(Some(&agent), &["memory", "show", "other-release"]);
    assert!(text(&shown).contains("nothing named"), "{}", text(&shown));
    let own_named = json_of(&fx.cli(Some(&agent), &["recall", "--thread", &own, "dock"]));
    assert!(own_named["hits"].to_string().contains(CODE));

    // Another chat (and through it another project) is refused, however it is asked.
    let elsewhere = fx.cli(Some(&agent), &["recall", "--thread", &other, "dock"]);
    assert!(refused(&elsewhere, "recall"), "{}", text(&elsewhere));
    assert!(!text(&elsewhere).contains(OTHER_CODE));
    let raw = json!({"threadId": other, "query": "dock code"}).to_string();
    let elsewhere = fx.cli(Some(&agent), &["call", "recall", &raw]);
    assert!(refused(&elsewhere, "recall"), "{}", text(&elsewhere));
    let elsewhere = fx.cli(Some(&agent), &["memory", "search", "--thread", &other, "x"]);
    assert!(refused(&elsewhere, "search_memory"), "{}", text(&elsewhere));

    // A run without the tools (memory off or denied, a subagent's) gets neither.
    let without = fx.token(&own, "run-2", &fx.project, &[]);
    let output = fx.cli(Some(&without), &["recall", "dock"]);
    assert!(unavailable(&output, "recall"), "{}", text(&output));
    let output = fx.cli(Some(&without), &["memory", "search", "releases"]);
    assert!(unavailable(&output, "search_memory"), "{}", text(&output));
    let recall_only = fx.token(&own, "run-3", &fx.project, &["recall"]);
    assert!(fx
        .cli(Some(&recall_only), &["recall", "dock"])
        .status
        .success());
    let output = fx.cli(Some(&recall_only), &["memory", "show", "tasks"]);
    assert!(unavailable(&output, "search_memory"), "{}", text(&output));

    // Logged as metadata: the way in, never the words or the credential.
    let logs = std::fs::read_dir(fx.data.join("logs"))
        .unwrap()
        .filter_map(Result::ok)
        .map(|entry| std::fs::read_to_string(entry.path()).unwrap_or_default())
        .collect::<String>();
    assert!(logs.contains("\"via\":\"cli\""), "{logs}");
    assert!(logs.contains("tool_unavailable"), "{logs}");
    for secret in [CODE, "north pier", "releases", agent.as_str()] {
        assert!(!logs.contains(secret), "{secret} in logs");
    }
    fx.shut_down().await;
}

/// A provider whose chat run calls `shell` with each argv in turn, then answers; any other
/// request (the memory pass after the turn) gets a plain reply.
async fn mount_calls(calls: Vec<Vec<String>>) -> MockServer {
    let server = MockServer::builder().start().await;
    Mock::given(method("POST"))
        .and(path("/chat/completions"))
        .respond_with(move |request: &Request| {
            let body: Value = serde_json::from_slice(&request.body).unwrap_or(Value::Null);
            let chat_run = body["tools"]
                .as_array()
                .is_some_and(|tools| tools.iter().any(|t| t["function"]["name"] == "shell"));
            // Tool results since this run's request (earlier runs' stay in the history).
            let done = body["messages"]
                .as_array()
                .map(|m| {
                    m.iter()
                        .rev()
                        .take_while(|m| m["role"] != "user")
                        .filter(|m| m["role"] == "tool")
                        .count()
                })
                .unwrap_or(0);
            let payload = match calls.get(done).filter(|_| chat_run) {
                Some(argv) => json!({"choices":[{"message":{"role":"assistant","content":null,
                    "tool_calls":[{"id":format!("call_{done}"),"type":"function","function":{
                        "name":"shell",
                        "arguments":json!({"command":argv[0],"args":argv[1..]}).to_string()}}]}}]}),
                None => json!({"choices":[{"message":{"role":"assistant","content":"finished"},
                    "finish_reason":"stop"}]}),
            };
            ResponseTemplate::new(200).set_body_json(payload)
        })
        .mount(&server)
        .await;
    server
}

/// Sends one message; returns each shell output and how many approvals were asked (all denied).
async fn run(fx: &Fixture, thread: &str) -> (Vec<String>, usize) {
    let (sink, mut rx) = ChannelSink::channel();
    fx.state
        .send_message(Arc::new(sink), thread.to_owned(), "look it up".into())
        .await
        .unwrap();
    let (mut outputs, mut approvals) = (Vec::new(), 0);
    let finished = tokio::time::timeout(Duration::from_secs(90), async {
        while let Some(event) = rx.recv().await {
            match event {
                TestEvent::Approval(request) => {
                    approvals += 1;
                    fx.state
                        .approve_action(
                            thread.to_owned(),
                            request.approval_id.clone(),
                            ApprovalDecision::Deny,
                        )
                        .await
                        .unwrap()
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
    (outputs, approvals)
}

fn argv(words: &[&str]) -> Vec<String> {
    words.iter().map(|word| (*word).to_owned()).collect()
}

#[tokio::test(flavor = "multi_thread")]
async fn a_runs_shell_recalls_and_searches_memory_without_approval_only_when_it_has_the_tools() {
    if !common::git_available() {
        return;
    }
    let _turn = ONE_SERVER.lock().await;
    let fx = fixture().await;
    let thread = fx.thread_in(&fx.project).await;
    save_original(
        &fx.project,
        &thread,
        &format!("The dock code for the north pier is {CODE}."),
    );
    remember(
        &fx.data,
        &fx.project,
        "release-steps",
        "How releases are cut here",
        "Tag, then publish the notes.",
    );
    let calls = vec![
        argv(&["themis", "recall", "dock", "code", "north", "pier"]),
        argv(&["themis", "memory", "search", "releases"]),
    ];
    let provider = mount_calls(calls).await;
    common::use_mock_llm(&fx.state, &provider).await;
    fx.state
        .set_thread_approval_mode(thread.clone(), ApprovalMode::Custom)
        .await
        .unwrap();

    // Memory on, no rule denying either tool: both answer, and nothing asks (they are reads).
    let (outputs, approvals) = run(&fx, &thread).await;
    assert_eq!(outputs.len(), 2, "{outputs:#?}");
    assert_eq!(approvals, 0, "{outputs:#?}");
    assert!(outputs[0].contains(CODE), "{}", outputs[0]);
    assert!(outputs[1].contains("release-steps"), "{}", outputs[1]);

    // The next run has memory off and a rule denying `recall`: neither is reachable.
    fx.state
        .save_runtime_configuration(
            "user",
            Some(fx.project.to_string_lossy().into_owned()),
            r#"{"memory":{"enabled":false},"approval":{"rules":[{"tool":"recall","action":"deny"},{"tool":"*","risk":"read","action":"allow"}]}}"#,
        )
        .unwrap();
    let (outputs, _) = run(&fx, &thread).await;
    assert_eq!(outputs.len(), 2, "{outputs:#?}");
    assert!(
        outputs[0].contains("does not have that tool"),
        "{}",
        outputs[0]
    );
    assert!(!outputs[0].contains(CODE), "{}", outputs[0]);
    assert!(
        outputs[1].contains("does not have that tool"),
        "{}",
        outputs[1]
    );
    assert!(!outputs[1].contains("release-steps"), "{}", outputs[1]);
    fx.shut_down().await;
}
