//! Project secrets through the shared app server and the `themis` CLI: sensitive files
//! ask first, a project cannot remove the defaults, and what the model provider receives.
#![cfg(unix)]

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use serde_json::{json, Value};
use themis_core::configuration::ApprovalMode;
use themis_desktop::server::Server;
use themis_desktop::sink::{ChannelSink, TestEvent};
use themis_desktop::state::AppState;
use themis_desktop::types::{ApprovalDecision, ApprovalRequest, ProviderKind, ThreadEvent};
use wiremock::matchers::method;
use wiremock::{Mock, MockServer, Request, ResponseTemplate};

#[allow(dead_code)]
mod common;

const VALUE: &str = "value-only-in-the-dotenv-file";

struct Fixture {
    data: tempfile::TempDir,
    _repo: tempfile::TempDir,
    project: PathBuf,
    state: AppState,
}

fn fixture() -> Fixture {
    let (repo, project) = common::init_repo();
    std::fs::write(project.join(".env"), format!("API_TOKEN={VALUE}\n")).unwrap();
    std::fs::write(project.join("notes.txt"), "plain notes\n").unwrap();
    let data = tempfile::tempdir().unwrap();
    let state = AppState::new_for_test(data.path().join("settings.json"));
    Fixture {
        data,
        _repo: repo,
        project,
        state,
    }
}

/// A model that makes the scripted calls one per round, then finishes.
async fn scripted(calls: Vec<(&'static str, Value)>) -> MockServer {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .respond_with(move |request: &Request| {
            let body: Value = serde_json::from_slice(&request.body).unwrap_or(Value::Null);
            let done = body["messages"]
                .as_array()
                .map_or(0, |m| m.iter().filter(|m| m["role"] == "tool").count());
            let payload = match calls.get(done) {
                Some((tool, args)) => {
                    json!({"choices":[{"message":{"role":"assistant","content":null,
                    "tool_calls":[{"id":format!("call_{done}"),"type":"function","function":{
                        "name":tool,"arguments":args.to_string()}}]}}]})
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

struct Outcome {
    approvals: Vec<ApprovalRequest>,
    /// Every request body the provider received, as text.
    wire: String,
    bodies: Vec<Value>,
    thread: String,
}

impl Outcome {
    fn sensitive(&self, index: usize) -> Vec<String> {
        serde_json::to_value(&self.approvals[index]).unwrap()["sensitive_files"]
            .as_array()
            .map(|files| {
                files
                    .iter()
                    .map(|file| file.as_str().unwrap().to_owned())
                    .collect()
            })
            .unwrap_or_default()
    }
}

async fn run(
    fx: &Fixture,
    mode: ApprovalMode,
    task: &str,
    calls: Vec<(&'static str, Value)>,
    decide: impl Fn(&ApprovalRequest) -> ApprovalDecision,
) -> Outcome {
    let provider = scripted(calls).await;
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
    let approvals = send(fx, &thread.id, task, &decide).await;
    finish(&provider, approvals, thread.id).await
}

async fn send(
    fx: &Fixture,
    thread: &str,
    task: &str,
    decide: &impl Fn(&ApprovalRequest) -> ApprovalDecision,
) -> Vec<ApprovalRequest> {
    let (sink, mut rx) = ChannelSink::channel();
    fx.state
        .send_message(Arc::new(sink), thread.to_owned(), task.into())
        .await
        .unwrap();
    let mut approvals = Vec::new();
    tokio::time::timeout(Duration::from_secs(90), async {
        while let Some(event) = rx.recv().await {
            match event {
                TestEvent::Approval(request) => {
                    let decision = decide(&request);
                    approvals.push(request.clone());
                    fx.state
                        .approve_action(thread.to_owned(), request.approval_id.clone(), decision)
                        .await
                        .unwrap();
                }
                TestEvent::Thread(envelope)
                    if matches!(
                        envelope.event,
                        ThreadEvent::Finished { .. } | ThreadEvent::Failed { .. }
                    ) =>
                {
                    break
                }
                _ => {}
            }
        }
    })
    .await
    .expect("run finished");
    approvals
}

async fn finish(provider: &MockServer, approvals: Vec<ApprovalRequest>, thread: String) -> Outcome {
    let requests = provider.received_requests().await.unwrap_or_default();
    let bodies: Vec<Value> = requests
        .iter()
        .filter_map(|request| serde_json::from_slice(&request.body).ok())
        .collect();
    Outcome {
        approvals,
        wire: bodies
            .iter()
            .map(Value::to_string)
            .collect::<Vec<_>>()
            .join("\n"),
        bodies,
        thread,
    }
}

fn read(path: &str) -> (&'static str, Value) {
    ("read_file", json!({"file_path": path}))
}

fn shell(command: &str, args: &[&str]) -> (&'static str, Value) {
    ("shell", json!({"command": command, "args": args}))
}

fn deny(_: &ApprovalRequest) -> ApprovalDecision {
    ApprovalDecision::Deny
}

fn once(_: &ApprovalRequest) -> ApprovalDecision {
    ApprovalDecision::Once
}

fn chat(_: &ApprovalRequest) -> ApprovalDecision {
    ApprovalDecision::Chat
}

fn user_config(fx: &Fixture, content: &str) {
    fx.state
        .save_runtime_configuration("user", None, content)
        .unwrap();
}

fn project_config(fx: &Fixture, content: &str) {
    std::fs::create_dir_all(fx.project.join(".themis")).unwrap();
    std::fs::write(fx.project.join(".themis/config.jsonc"), content).unwrap();
}

#[tokio::test(flavor = "multi_thread")]
async fn read_file_of_dotenv_asks_and_a_denial_keeps_the_value_from_the_provider() {
    let fx = fixture();
    let run = run(&fx, ApprovalMode::Custom, "go", vec![read(".env")], deny).await;
    assert_eq!(run.approvals.len(), 1, "one prompt");
    assert_eq!(run.sensitive(0), [".env"]);
    assert_eq!(run.approvals[0].tool, "read_file");
    assert!(!run.wire.contains(VALUE), "the value reached the provider");
    assert!(run.wire.contains("approval denied"));
}

#[tokio::test(flavor = "multi_thread")]
async fn allow_once_reads_it_once_and_the_next_read_asks_again() {
    let fx = fixture();
    let run = run(
        &fx,
        ApprovalMode::Custom,
        "go",
        vec![read(".env"), read(".env")],
        once,
    )
    .await;
    assert_eq!(run.approvals.len(), 2);
    assert!(run.wire.contains(VALUE));
}

#[tokio::test(flavor = "multi_thread")]
async fn allow_in_this_chat_covers_that_file_and_direction_only() {
    let fx = fixture();
    std::fs::write(fx.project.join(".env.local"), "LOCAL=1\n").unwrap();
    let run = run(
        &fx,
        ApprovalMode::Custom,
        "go",
        vec![
            read(".env"),
            read(".env"),
            read(".env.local"),
            (
                "write_file",
                json!({"file_path": ".env", "content": "X=1\n"}),
            ),
            (
                "write_file",
                json!({"file_path": "plain.txt", "content": "x"}),
            ),
        ],
        chat,
    )
    .await;
    let asked: Vec<Vec<String>> = (0..run.approvals.len()).map(|i| run.sensitive(i)).collect();
    // The second read is covered; the other file and the write each ask once, and
    // a grant for writing `.env` does not become a grant for every write.
    assert_eq!(
        asked,
        [
            vec![".env".to_owned()],
            vec![".env.local".to_owned()],
            vec![".env".to_owned()],
            vec![]
        ]
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn a_chat_grant_for_writes_does_not_cover_a_sensitive_write() {
    let fx = fixture();
    let run = run(
        &fx,
        ApprovalMode::Custom,
        "go",
        vec![
            (
                "write_file",
                json!({"file_path": "plain.txt", "content": "x"}),
            ),
            (
                "write_file",
                json!({"file_path": ".env", "content": "X=1\n"}),
            ),
            (
                "write_file",
                json!({"file_path": "plain2.txt", "content": "x"}),
            ),
        ],
        |request| {
            if request.summary.contains("plain.txt") {
                ApprovalDecision::Chat
            } else {
                ApprovalDecision::Deny
            }
        },
    )
    .await;
    assert_eq!(run.approvals.len(), 2, "plain2.txt is covered by the grant");
    assert_eq!(run.sensitive(1), [".env"]);
    let env = std::fs::read_to_string(fx.project.join(".env")).unwrap();
    assert!(env.contains(VALUE), "the denied write must not happen");
    assert!(fx.project.join("plain2.txt").exists());
}

#[tokio::test(flavor = "multi_thread")]
async fn search_file_lists_names_without_contents_and_does_not_ask() {
    let fx = fixture();
    let run = run(
        &fx,
        ApprovalMode::Custom,
        "go",
        vec![
            (
                "search_file",
                json!({"directory": ".", "pattern": "API_TOKEN"}),
            ),
            ("search_file", json!({"directory": ".", "pattern": ".env"})),
        ],
        deny,
    )
    .await;
    assert!(run.approvals.is_empty(), "names are not contents");
    assert!(!run.wire.contains(VALUE));
}

#[tokio::test(flavor = "multi_thread")]
async fn shell_reads_ask_and_exact_names_survive_spelling_and_links() {
    let fx = fixture();
    std::os::unix::fs::symlink(".env", fx.project.join("innocent.txt")).unwrap();
    std::fs::create_dir(fx.project.join("sub")).unwrap();
    std::fs::write(fx.project.join(".ENV.prod"), "UPPER=1\n").unwrap();
    // An allow rule for the programs: only the sensitive check can make these ask.
    user_config(
        &fx,
        r#"{"approval":{"rules":[
          {"tool":"shell","command":["cat"],"action":"allow"},
          {"tool":"shell","command":["sh"],"action":"allow"},
          {"tool":"shell","command":["head"],"action":"allow"},
          {"tool":"*","risk":"read","action":"allow"}
        ]}}"#,
    );
    let run = run(
        &fx,
        ApprovalMode::Custom,
        "go",
        vec![
            shell("cat", &[".env"]),
            shell("sh", &["-c", "cat .env | head -1"]),
            shell("head", &["./sub/../.env"]),
            shell("cat", &["innocent.txt"]),
            shell("sh", &["-c", "cat .en*"]),
            shell("cat", &[".ENV.prod"]),
            read("./sub/../.env"),
            read("innocent.txt"),
            shell("cat", &["notes.txt"]),
        ],
        deny,
    )
    .await;
    assert_eq!(
        run.approvals.len(),
        8,
        "everything but notes.txt asks: {:?}",
        run.approvals.iter().map(|a| &a.summary).collect::<Vec<_>>()
    );
    assert!(!run.wire.contains(VALUE));
    assert!(run.wire.contains("plain notes"));
}

#[tokio::test(flavor = "multi_thread")]
async fn examples_are_exempt_and_never_ask() {
    let fx = fixture();
    std::fs::write(fx.project.join(".env.example"), "API_TOKEN=\n").unwrap();
    std::fs::write(fx.project.join(".env.sample"), "A=\n").unwrap();
    std::fs::write(fx.project.join(".env.template"), "B=\n").unwrap();
    let run = run(
        &fx,
        ApprovalMode::Custom,
        "go",
        vec![
            read(".env.example"),
            read(".env.sample"),
            read(".env.template"),
        ],
        deny,
    )
    .await;
    assert!(run.approvals.is_empty());
    assert!(run.wire.contains("API_TOKEN="));
}

#[tokio::test(flavor = "multi_thread")]
async fn yolo_stays_open_but_a_deny_rule_holds() {
    let fx = fixture();
    let open = run(&fx, ApprovalMode::Yolo, "go", vec![read(".env")], deny).await;
    assert!(open.approvals.is_empty());
    assert!(open.wire.contains(VALUE));

    user_config(
        &fx,
        r#"{"approval":{"rules":[{"tool":"sensitive_file","action":"deny"}]}}"#,
    );
    let denied = run(
        &fx,
        ApprovalMode::Yolo,
        "go",
        vec![read(".env"), shell("cat", &[".env"]), read("notes.txt")],
        deny,
    )
    .await;
    assert!(denied.approvals.is_empty());
    assert!(!denied.wire.contains(VALUE));
    assert!(denied.wire.contains("plain notes"));
}

#[tokio::test(flavor = "multi_thread")]
async fn a_deny_rule_beats_an_earlier_chat_grant() {
    let fx = fixture();
    let first = run(&fx, ApprovalMode::Custom, "go", vec![read(".env")], chat).await;
    assert_eq!(first.approvals.len(), 1);
    user_config(
        &fx,
        r#"{"approval":{"rules":[{"tool":"sensitive_file","action":"deny"}]}}"#,
    );
    let provider = scripted(vec![read(".env")]).await;
    common::use_mock_llm(&fx.state, &provider).await;
    let approvals = send(&fx, &first.thread, "again", &deny).await;
    let second = finish(&provider, approvals, first.thread.clone()).await;
    assert!(second.approvals.is_empty());
    assert!(second.wire.contains("approval denied"));
}

#[tokio::test(flavor = "multi_thread")]
async fn a_chat_grant_keeps_later_runs_from_asking() {
    let fx = fixture();
    let first = run(&fx, ApprovalMode::Custom, "go", vec![read(".env")], chat).await;
    let provider = scripted(vec![read(".env")]).await;
    common::use_mock_llm(&fx.state, &provider).await;
    let approvals = send(&fx, &first.thread, "again", &deny).await;
    assert!(approvals.is_empty(), "the grant lasts for the chat");
}

#[tokio::test(flavor = "multi_thread")]
async fn a_user_list_replaces_the_defaults_and_an_empty_one_turns_it_off() {
    let fx = fixture();
    std::fs::write(fx.project.join("vault.txt"), "VAULT\n").unwrap();
    user_config(&fx, r#"{"approval":{"sensitive_files":["vault.*"]}}"#);
    let replaced = run(
        &fx,
        ApprovalMode::Custom,
        "go",
        vec![read(".env"), read("vault.txt")],
        deny,
    )
    .await;
    assert_eq!(replaced.approvals.len(), 1);
    assert_eq!(replaced.sensitive(0), ["vault.txt"]);

    user_config(&fx, r#"{"approval":{"sensitive_files":[]}}"#);
    let off = run(&fx, ApprovalMode::Custom, "go", vec![read(".env")], deny).await;
    assert!(off.approvals.is_empty());
    assert!(off.wire.contains(VALUE));
}

#[tokio::test(flavor = "multi_thread")]
async fn a_project_can_add_but_cannot_remove() {
    let fx = fixture();
    std::fs::write(fx.project.join("data.vault"), "VAULT\n").unwrap();
    project_config(&fx, r#"{"approval":{"sensitive_files":["*.vault"]}}"#);
    let added = run(
        &fx,
        ApprovalMode::Custom,
        "go",
        vec![read(".env"), read("data.vault")],
        deny,
    )
    .await;
    assert_eq!(added.approvals.len(), 2, "defaults stay, the project adds");

    project_config(&fx, r#"{"approval":{"sensitive_files":[]}}"#);
    let empty = run(&fx, ApprovalMode::Custom, "go", vec![read(".env")], deny).await;
    assert_eq!(
        empty.approvals.len(),
        1,
        "an empty project list removes nothing"
    );

    project_config(&fx, r#"{"approval":{"sensitive_files":["!.env"]}}"#);
    let thread = fx
        .state
        .create_thread(
            fx.project.to_string_lossy().into_owned(),
            ProviderKind::Go,
            Some("test-model".to_owned()),
        )
        .await;
    // An exemption from a project is a configuration error: the run is refused.
    let (sink, _rx) = ChannelSink::channel();
    let refused = match thread {
        Ok(thread) => fx
            .state
            .send_message(Arc::new(sink), thread.id, "go".into())
            .await
            .err(),
        Err(error) => Some(error),
    };
    assert!(
        refused.is_some_and(|error| error.contains("sensitive_files")),
        "a project must not exempt files"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn git_add_of_everything_asks_only_when_a_secret_would_be_staged() {
    let fx = fixture();
    // `.env` is untracked and not ignored.
    let staged = run(
        &fx,
        ApprovalMode::Custom,
        "go",
        vec![("git", json!({"args": ["add", "."]}))],
        deny,
    )
    .await;
    assert_eq!(staged.approvals.len(), 1);
    assert_eq!(staged.sensitive(0), [".env"]);

    // Ignored: `git add .` cannot stage it, so nothing is flagged.
    std::fs::write(fx.project.join(".gitignore"), ".env\n").unwrap();
    let ignored = run(
        &fx,
        ApprovalMode::Custom,
        "go",
        vec![("git", json!({"args": ["add", "."]}))],
        deny,
    )
    .await;
    assert_eq!(ignored.approvals.len(), 1, "git add still asks as a write");
    assert!(ignored.sensitive(0).is_empty());
}

#[tokio::test(flavor = "multi_thread")]
async fn a_grant_for_git_add_does_not_stage_a_secret_silently() {
    let fx = fixture();
    std::fs::remove_file(fx.project.join(".env")).unwrap();
    std::fs::write(fx.project.join("a.txt"), "a").unwrap();
    let run = run(
        &fx,
        ApprovalMode::Custom,
        "go",
        vec![
            ("git", json!({"args": ["add", "a.txt"]})),
            (
                "shell",
                json!({"command": "touch", "args": ["placeholder"]}),
            ),
            (
                "write_file",
                json!({"file_path": ".env", "content": "X=1\n"}),
            ),
            ("git", json!({"args": ["add", "."]})),
        ],
        |request| {
            if request.tool == "git" && request.summary.contains("a.txt") {
                ApprovalDecision::Chat
            } else if request.tool == "git" {
                ApprovalDecision::Deny
            } else {
                ApprovalDecision::Once
            }
        },
    )
    .await;
    let last = run.approvals.last().unwrap();
    assert_eq!(last.tool, "git");
    assert_eq!(run.sensitive(run.approvals.len() - 1), [".env"]);
    let status = std::process::Command::new("git")
        .args(["status", "--porcelain"])
        .current_dir(&fx.project)
        .output()
        .unwrap();
    let status = String::from_utf8_lossy(&status.stdout);
    assert!(
        status.contains("?? .env"),
        "`.env` stayed unstaged: {status}"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn git_commit_asks_when_a_secret_is_staged_and_the_agent_is_told() {
    let fx = fixture();
    common::git(&fx.project, &["add", "-f", ".env"]);
    let refused = run(
        &fx,
        ApprovalMode::Custom,
        "go",
        vec![("git", json!({"args": ["commit", "-m", "x"]}))],
        deny,
    )
    .await;
    assert_eq!(refused.sensitive(0), [".env"]);
    assert!(refused.wire.contains(".env"));

    // In YOLO nothing asks, but the result tells the agent what it did.
    let told = run(
        &fx,
        ApprovalMode::Yolo,
        "go",
        vec![("git", json!({"args": ["commit", "-m", "x"]}))],
        deny,
    )
    .await;
    assert!(told.approvals.is_empty());
    assert!(
        told.wire.contains(
            &themis_core::prompts::TOOL_SENSITIVE_COMMIT
                .replace("{{files}}", ".env")
                .replace('"', "\\\"")
        ),
        "the tool result carries the note"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn the_run_warns_about_an_unignored_or_tracked_secret_file() {
    let fx = fixture();
    let unignored = run(&fx, ApprovalMode::Custom, "go", vec![], deny).await;
    let system = unignored.bodies[0]["messages"][0]["content"]
        .as_str()
        .unwrap()
        .to_owned();
    assert!(
        system.contains(
            &themis_core::prompts::DESKTOP_SECRETS_UNIGNORED.replace("{{files}}", ".env")
        ),
        "{system}"
    );

    std::fs::write(fx.project.join(".gitignore"), ".env\n").unwrap();
    let ignored = run(&fx, ApprovalMode::Custom, "go", vec![], deny).await;
    let system = ignored.bodies[0]["messages"][0]["content"]
        .as_str()
        .unwrap();
    assert!(!system.contains("not ignored by Git"), "{system}");

    common::git(&fx.project, &["add", "-f", ".env"]);
    common::git(&fx.project, &["commit", "-qm", "oops"]);
    let tracked = run(&fx, ApprovalMode::Custom, "go", vec![], deny).await;
    let system = tracked.bodies[0]["messages"][0]["content"]
        .as_str()
        .unwrap();
    assert!(
        system
            .contains(&themis_core::prompts::DESKTOP_SECRETS_TRACKED.replace("{{files}}", ".env")),
        "{system}"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn a_pasted_key_adds_a_note_that_never_holds_the_value() {
    let fx = fixture();
    let key = ["ghp_", "abcdefghij0123456789", "ABCDEFGHIJ012345"].concat();
    let pasted = run(
        &fx,
        ApprovalMode::Custom,
        &format!("use this {key} please"),
        vec![],
        deny,
    )
    .await;
    let messages = pasted.bodies[0]["messages"].as_array().unwrap();
    let note = messages
        .iter()
        .find(|m| m["content"] == themis_core::prompts::RUNTIME_PASTED_SECRET)
        .expect("the note is in the request");
    assert_eq!(note["role"], "user");
    assert!(!note.to_string().contains(&key));
    assert_eq!(
        pasted.wire.matches(key.as_str()).count(),
        1,
        "the value appears only in the person's own message"
    );

    let plain = run(
        &fx,
        ApprovalMode::Custom,
        "commit 3f2a9c1d7e6b5a4f3c2d1e0f9a8b7c6d5e4f3a2b looks wrong",
        vec![],
        deny,
    )
    .await;
    assert!(!plain
        .wire
        .contains(themis_core::prompts::RUNTIME_PASTED_SECRET));
}

#[tokio::test(flavor = "multi_thread")]
async fn asking_logs_the_kind_and_never_the_path() {
    let fx = fixture();
    run(&fx, ApprovalMode::Custom, "go", vec![read(".env")], deny).await;
    let records = fx.state.query_diagnostic_logs(Default::default()).unwrap();
    let asked = records
        .iter()
        .find(|record| record.event == "sensitive_file_asked")
        .expect("a metadata event");
    let text = serde_json::to_string(&asked).unwrap();
    assert!(text.contains("\"kind\":\"env\""), "{text}");
    assert!(!text.contains(".env"), "{text}");
    assert!(!text.contains(VALUE), "{text}");
    let all = serde_json::to_string(&records).unwrap();
    assert!(!all.contains(VALUE));
}

#[tokio::test(flavor = "multi_thread")]
async fn a_subagents_request_names_it_and_its_grant_stays_its_own() {
    let fx = fixture();
    let provider = MockServer::start().await;
    Mock::given(method("POST"))
        .respond_with(move |request: &Request| {
            let body: Value = serde_json::from_slice(&request.body).unwrap_or(Value::Null);
            let messages = body["messages"].as_array().cloned().unwrap_or_default();
            let subagent = messages
                .first()
                .and_then(|m| m["content"].as_str())
                .is_some_and(|text| text.starts_with("You are a subagent"));
            let tools = messages.iter().filter(|m| m["role"] == "tool").count();
            let call = |name: &str, args: Value| {
                json!({"choices":[{"message":{"role":"assistant","content":null,
                    "tool_calls":[{"id":format!("c{tools}"),"type":"function","function":{
                        "name":name,"arguments":args.to_string()}}]}}]})
            };
            let text = |text: &str| {
                json!({"choices":[{"message":{"role":"assistant","content":text},
                    "finish_reason":"stop"}]})
            };
            let payload = if subagent {
                match tools {
                    0 => call("read_file", json!({"file_path": ".env"})),
                    1 => call("read_file", json!({"file_path": ".env"})),
                    _ => text("done reading"),
                }
            } else if tools == 0 {
                call(
                    "spawn_subagents",
                    json!({"subagents":[{"name":"Reader","brief":"Read the env file."}]}),
                )
            } else if messages.iter().any(|m| {
                m["content"]
                    .as_str()
                    .is_some_and(|t| t.contains("done reading"))
            }) {
                text("finished")
            } else {
                text("waiting")
            };
            ResponseTemplate::new(200).set_body_json(payload)
        })
        .mount(&provider)
        .await;
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
    let approvals = send(&fx, &thread.id, "use a subagent", &chat).await;
    // Wait for the subagent's own request, then its result arriving back at the main agent.
    tokio::time::sleep(Duration::from_millis(300)).await;
    let first = approvals
        .iter()
        .find(|request| request.subagent.is_some())
        .expect("the subagent asked");
    assert_eq!(first.subagent.as_ref().unwrap().name, "Reader");
    assert_eq!(
        serde_json::to_value(first).unwrap()["sensitive_files"],
        json!([".env"])
    );
    assert_eq!(
        approvals.iter().filter(|a| a.subagent.is_some()).count(),
        1,
        "its grant covers its second read"
    );
    // The subagent's grant is not the chat's: the file on disk is untouched either way.
    assert!(!grant_file_mentions(fx.data.path(), &thread.id, ".env"));
}

fn grant_file_mentions(data: &Path, thread: &str, needle: &str) -> bool {
    std::fs::read_to_string(data.join("chat_grants").join(format!("{thread}.json")))
        .is_ok_and(|text| text.contains(needle))
}

fn cli_call(data_dir: &Path, method: &str, args: Value) -> Value {
    use std::io::Write;
    let mut child = std::process::Command::new(env!("CARGO_BIN_EXE_themis"))
        .arg("--data-dir")
        .arg(data_dir)
        .env_remove("OPENCODE_KEY")
        .args(["call", method, "-"])
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .expect("run CLI");
    child
        .stdin
        .take()
        .unwrap()
        .write_all(args.to_string().as_bytes())
        .unwrap();
    let output = child.wait_with_output().unwrap();
    assert!(
        output.status.success(),
        "CLI {method} failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    serde_json::from_slice(&output.stdout).expect("CLI JSON output")
}

/// Unattended, through the real CLI: an allow rule runs the plain command, and the same
/// program on `.env` is refused, since nobody is there to answer.
#[tokio::test(flavor = "multi_thread")]
async fn the_cli_refuses_sensitive_reads_it_cannot_ask_about() {
    let fx = fixture();
    let provider = scripted(vec![
        read(".env"),
        shell("cat", &[".env"]),
        shell("cat", &["notes.txt"]),
    ])
    .await;
    common::use_mock_llm(&fx.state, &provider).await;
    let Fixture {
        data,
        _repo,
        project,
        state,
    } = fx;
    let server = Server::bind(state, data.path()).await.unwrap();
    let serving = tokio::spawn(server.run());
    let root = project.to_str().unwrap();
    cli_call(
        data.path(),
        "save_runtime_configuration",
        json!({"scope":"user","projectRoot":root,"json":
            r#"{"approval":{"rules":[
                {"tool":"shell","command":["cat"],"action":"allow"},
                {"tool":"*","risk":"read","action":"allow"}
            ]}}"#}),
    );
    let thread = cli_call(
        data.path(),
        "create_thread",
        json!({"projectRoot":root,"provider":"go","model":"test-model"}),
    );
    let id = thread["id"].as_str().unwrap().to_owned();
    let data_path = data.path().to_owned();
    let sent = tokio::task::spawn_blocking(move || {
        std::process::Command::new(env!("CARGO_BIN_EXE_themis"))
            .arg("--data-dir")
            .arg(&data_path)
            .args(["thread", "send", &id, "read them"])
            .stdin(std::process::Stdio::null())
            .env_remove("OPENCODE_KEY")
            .output()
            .unwrap()
    })
    .await
    .unwrap();
    assert!(
        sent.status.success(),
        "{}",
        String::from_utf8_lossy(&sent.stderr)
    );
    let requests = provider.received_requests().await.unwrap_or_default();
    let wire: String = requests
        .iter()
        .map(|request| String::from_utf8_lossy(&request.body).into_owned())
        .collect();
    assert!(!wire.contains(VALUE), "the value reached the provider");
    assert!(wire.contains("plain notes"), "the plain command ran");
    serving.abort();
}
