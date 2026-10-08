//! CLI calls the same live AppState as the desktop, including streamed approvals.

use std::path::Path;
use std::time::Duration;

use serde_json::{json, Value};
use themis_desktop::server::{Client, Server};
use themis_desktop::state::AppState;

mod common;

fn cli(data_dir: &Path, words: &[&str]) -> Value {
    let output = std::process::Command::new(env!("CARGO_BIN_EXE_themis"))
        .arg("--data-dir")
        .arg(data_dir)
        .args(words)
        .env_remove("OPENCODE_KEY")
        .output()
        .expect("run CLI");
    assert!(
        output.status.success(),
        "CLI {words:?} failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    serde_json::from_slice(&output.stdout).expect("CLI JSON output")
}

#[test]
fn headless_desktop_process_persists_cli_created_threads() {
    let data_dir = tempfile::tempdir().expect("data dir");
    let project = tempfile::tempdir().expect("project dir");
    let start = || {
        std::process::Command::new(env!("CARGO_BIN_EXE_themis-desktop"))
            .env_remove("OPENCODE_KEY")
            .arg("--themis-server")
            .arg(data_dir.path())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .spawn()
            .expect("start headless desktop server")
    };
    let ready = || {
        for _ in 0..50 {
            if std::process::Command::new(env!("CARGO_BIN_EXE_themis"))
                .arg("--data-dir")
                .arg(data_dir.path())
                .arg("status")
                .output()
                .is_ok_and(|output| output.status.success())
            {
                return;
            }
            std::thread::sleep(Duration::from_millis(100));
        }
        panic!("headless desktop server did not become ready");
    };

    let mut first = start();
    ready();
    let thread = cli(
        data_dir.path(),
        &["thread", "create", project.path().to_str().unwrap()],
    );
    let id = thread["id"].as_str().unwrap().to_owned();
    assert_eq!(cli(data_dir.path(), &["server", "stop"]), Value::Null);
    first.wait().expect("reap first server");
    assert!(!data_dir.path().join("server.json").exists());

    let mut second = start();
    ready();
    assert_eq!(cli(data_dir.path(), &["thread", "get", &id])["id"], id);
    assert_eq!(cli(data_dir.path(), &["server", "stop"]), Value::Null);
    second.wait().expect("reap second server");
}

#[tokio::test(flavor = "multi_thread")]
async fn server_cli_drives_thread_approval_and_persistence() {
    if !common::git_available() {
        eprintln!("skipping: git binary not found");
        return;
    }
    let (_repo_dir, root) = common::init_repo();
    let data_dir = tempfile::tempdir().expect("data dir");
    let state = AppState::new_for_test(data_dir.path().join("settings.json"));
    let provider = common::mount_scripted_llm().await;
    common::use_mock_llm(&state, &provider).await;
    let server = Server::bind(state.clone(), data_dir.path())
        .await
        .expect("bind server");
    let task = tokio::spawn(server.run());

    let project = root.to_str().expect("project path");
    assert_eq!(cli(data_dir.path(), &["status"]), json!("0.1.0"));
    let opened = cli(
        data_dir.path(),
        &[
            "call",
            "open_project",
            &json!({"path": project}).to_string(),
        ],
    );
    assert_eq!(
        opened["root"],
        root.canonicalize().unwrap().to_str().unwrap()
    );
    let thread = cli(
        data_dir.path(),
        &["thread", "create", project, "test-model"],
    );
    let id = thread["id"].as_str().expect("thread ID");
    assert_eq!(
        cli(data_dir.path(), &["thread", "list", project])
            .as_array()
            .unwrap()
            .len(),
        1
    );
    assert_eq!(cli(data_dir.path(), &["thread", "get", id])["id"], id);

    let mut events = Client::new(data_dir.path().to_path_buf())
        .subscribe()
        .await
        .expect("subscribe");
    let handle = cli(
        data_dir.path(),
        &["thread", "send", id, "write hello.txt", "--detach"],
    );
    let run_id = handle["run_id"].as_str().expect("run ID");
    let mut approved = false;
    let terminal = tokio::time::timeout(Duration::from_secs(60), async {
        loop {
            let event = Client::next_event(&mut events).await.expect("event");
            if event["name"] == "approval-request" {
                assert_eq!(event["payload"]["thread_id"], id);
                let approval_id = event["payload"]["approval_id"].as_str().unwrap();
                let request =
                    json!({"threadId": id, "approvalId": approval_id, "decision": "once"});
                assert_eq!(
                    cli(
                        data_dir.path(),
                        &["call", "approve_action", &request.to_string()]
                    ),
                    Value::Null
                );
                approved = true;
            }
            if event["name"] == "thread-event" && event["payload"]["run_id"] == run_id {
                match event["payload"]["event"]["kind"]
                    .as_str()
                    .unwrap_or_default()
                {
                    "finished" | "failed" => break event["payload"]["event"].clone(),
                    _ => {}
                }
            }
        }
    })
    .await
    .expect("terminal event");
    assert!(
        approved,
        "tool approval was not delivered through the server"
    );
    assert_eq!(terminal["kind"], "finished", "{terminal}");
    assert_eq!(
        std::fs::read_to_string(root.join("hello.txt")).unwrap(),
        "hi from agent\n"
    );
    assert!(!cli(data_dir.path(), &["thread", "history", id])
        .as_array()
        .unwrap()
        .is_empty());
    assert!(!cli(
        data_dir.path(),
        &["call", "list_diff", &json!({"threadId": id}).to_string()]
    )
    .is_null());

    task.abort();
}

#[test]
fn help_and_version_work_without_starting_a_server() {
    let dir = tempfile::tempdir().unwrap();
    for option in ["--help", "--version"] {
        let output = std::process::Command::new(env!("CARGO_BIN_EXE_themis"))
            .args(["--data-dir", dir.path().to_str().unwrap(), option])
            .env_remove("OPENCODE_KEY")
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "{option}: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        assert!(!dir.path().join("sessions.sqlite3").exists());
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn streamed_cli_followup_uses_persisted_context_and_skills() {
    use wiremock::matchers::{method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};
    for (model, endpoint, response, message_field) in [
        (
            "test-model",
            "/chat/completions",
            common::final_text_body(),
            "messages",
        ),
        (
            "gpt-6-luna",
            "/responses",
            json!({"output":[{"type":"message","content":[{"type":"output_text","text":"Done."}]}]}),
            "input",
        ),
    ] {
        let project = tempfile::tempdir().unwrap();
        let data = tempfile::tempdir().unwrap();
        let state = AppState::new_for_test(data.path().join("settings.json"));
        let provider = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path(endpoint))
            .respond_with(ResponseTemplate::new(200).set_body_json(response))
            .mount(&provider)
            .await;
        common::use_mock_llm(&state, &provider).await;
        let server = Server::bind(state, data.path()).await.unwrap();
        let task = tokio::spawn(server.run());
        let thread = cli(
            data.path(),
            &["thread", "create", project.path().to_str().unwrap(), model],
        );
        let id = thread["id"].as_str().unwrap();
        let skill = cli(data.path(), &["call", "create_skill", &json!({"input": {
        "name": "Writer", "description": "test", "instructions": "Always be concise.",
        "allowed_tools": ["read_file"], "scripts": [{"name": "greet.sh", "content": "echo hi"}]
    }}).to_string()]);
        cli(
            data.path(),
            &[
                "call",
                "set_thread_skills",
                &json!({"threadId": id, "skillIds": [skill["id"]]}).to_string(),
            ],
        );
        for prompt in ["Remember my first request", "What was my first request?"] {
            let output = std::process::Command::new(env!("CARGO_BIN_EXE_themis"))
                .arg("--data-dir")
                .arg(data.path())
                .args(["thread", "send", id, prompt, "--json"])
                .env_remove("OPENCODE_KEY")
                .output()
                .unwrap();
            assert!(
                output.status.success(),
                "{}",
                String::from_utf8_lossy(&output.stderr)
            );
            let events: Vec<Value> = String::from_utf8(output.stdout)
                .unwrap()
                .lines()
                .map(|line| serde_json::from_str(line).unwrap())
                .collect();
            assert!(events
                .iter()
                .any(|event| event["payload"]["event"]["kind"] == "finished"));
        }
        let requests = provider.received_requests().await.unwrap();
        let body: Value = serde_json::from_slice(&requests.last().unwrap().body).unwrap();
        let messages = body[message_field].to_string();
        if endpoint == "/responses" {
            let reader = body["tools"]
                .as_array()
                .unwrap()
                .iter()
                .find(|tool| tool["name"] == "read_file")
                .unwrap();
            assert_eq!(reader["strict"], false);
            assert_eq!(reader["parameters"]["required"], json!(["file_path"]));
        }
        assert!(messages.contains("Remember my first request"));
        assert!(messages.contains("Always be concise."));
        assert!(project
            .path()
            .join(".themis/skills")
            .join(skill["id"].as_str().unwrap())
            .join("greet.sh")
            .exists());
        assert!(
            cli(data.path(), &["thread", "history", id])
                .as_array()
                .unwrap()
                .len()
                >= 4
        );
        task.abort();
    }
}

#[test]
fn doctor_reports_corruption_without_initializing_or_repairing_stores() {
    let dir = tempfile::tempdir().unwrap();
    let settings = dir.path().join("settings.json");
    std::fs::write(&settings, "{broken secret-like-content").unwrap();
    let output = std::process::Command::new(env!("CARGO_BIN_EXE_themis"))
        .arg("--data-dir")
        .arg(dir.path())
        .args(["doctor", "--json", "--deep"])
        .env_remove("OPENCODE_KEY")
        .output()
        .unwrap();
    assert!(!output.status.success());
    let report: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert!(report["checks"]
        .as_array()
        .unwrap()
        .iter()
        .any(|check| check["name"] == "settings.json" && check["status"] == "fail"));
    assert!(!String::from_utf8_lossy(&output.stdout).contains("secret-like-content"));
    assert_eq!(
        std::fs::read_to_string(&settings).unwrap(),
        "{broken secret-like-content"
    );
    assert!(!dir.path().join("sessions.sqlite3").exists());
    assert!(!dir.path().join("server.lock").exists());
}

#[cfg(unix)]
#[test]
fn packaged_executable_installs_cli_and_starts_shared_server() {
    let dir = tempfile::tempdir().unwrap();
    let project = tempfile::tempdir().unwrap();
    let destination = dir.path().join("bin");
    let output = std::process::Command::new(env!("CARGO_BIN_EXE_themis-desktop"))
        .args(["--themis-cli", "cli", "install"])
        .arg(&destination)
        .env_remove("OPENCODE_KEY")
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let link = destination.join("themis");
    let invoke = |words: &[&str]| {
        std::process::Command::new(&link)
            .arg("--data-dir")
            .arg(dir.path())
            .args(words)
            .env_remove("OPENCODE_KEY")
            .output()
            .unwrap()
    };
    assert!(invoke(&["--help"]).status.success());
    assert!(invoke(&["cli", "install", destination.to_str().unwrap()])
        .status
        .success());
    assert!(!dir.path().join("sessions.sqlite3").exists());
    let created = invoke(&["thread", "create", project.path().to_str().unwrap()]);
    assert!(
        created.status.success(),
        "{}",
        String::from_utf8_lossy(&created.stderr)
    );
    let thread: Value = serde_json::from_slice(&created.stdout).unwrap();
    assert!(thread["id"].is_string());
    assert!(invoke(&["server", "stop"]).status.success());
    let conflict = dir.path().join("conflict");
    std::fs::create_dir(&conflict).unwrap();
    std::fs::write(conflict.join("themis"), "preserve existing command").unwrap();
    assert!(!invoke(&["cli", "install", conflict.to_str().unwrap()])
        .status
        .success());
    assert_eq!(
        std::fs::read_to_string(conflict.join("themis")).unwrap(),
        "preserve existing command"
    );
}

#[cfg(unix)]
#[tokio::test(flavor = "multi_thread")]
async fn terminal_send_approves_tool_and_unattended_send_denies_it() {
    let (_project_dir, project) = common::init_repo();
    let dir = tempfile::tempdir().unwrap();
    let state = AppState::new_for_test(dir.path().join("settings.json"));
    let provider = common::mount_scripted_llm().await;
    common::use_mock_llm(&state, &provider).await;
    let server = Server::bind(state, dir.path()).await.unwrap();
    let task = tokio::spawn(server.run());
    let thread = cli(
        dir.path(),
        &[
            "thread",
            "create",
            project.as_path().to_str().unwrap(),
            "test-model",
        ],
    );
    let id = thread["id"].as_str().unwrap();
    let denied = std::process::Command::new(env!("CARGO_BIN_EXE_themis"))
        .arg("--data-dir")
        .arg(dir.path())
        .args(["thread", "send", id, "write hello.txt"])
        .stdin(std::process::Stdio::null())
        .env_remove("OPENCODE_KEY")
        .output()
        .unwrap();
    assert!(
        denied.status.success(),
        "{}",
        String::from_utf8_lossy(&denied.stderr)
    );
    assert!(String::from_utf8_lossy(&denied.stderr).contains("Tool approval denied"));
    assert!(!project.as_path().join("hello.txt").exists());
    let script = r#"
import os, pty, select, sys, time
binary, directory, thread = sys.argv[1:]
pid, fd = pty.fork()
if pid == 0:
    env = dict(os.environ)
    env.pop('OPENCODE_KEY', None)
    os.execve(binary, [binary, '--data-dir', directory, 'thread', 'send', thread, 'write hello.txt'], env)
output = b''
deadline = time.monotonic() + 15
approved = False
try:
    while True:
        assert time.monotonic() < deadline, 'terminal send timed out'
        exited, status = os.waitpid(pid, os.WNOHANG)
        if exited: break
        if select.select([fd], [], [], .1)[0]:
            try: output += os.read(fd, 8192)
            except OSError: pass
        if b'Allow once?' in output and not approved:
            os.write(fd, b'y\n')
            approved = True
    assert approved and os.waitstatus_to_exitcode(status) == 0, output
finally:
    try: os.kill(pid, 9)
    except ProcessLookupError: pass
    try: os.waitpid(pid, 0)
    except ChildProcessError: pass
    os.close(fd)
"#;
    let output = std::process::Command::new("python3")
        .arg("-c")
        .arg(script)
        .arg(env!("CARGO_BIN_EXE_themis"))
        .arg(dir.path())
        .arg(id)
        .env_remove("OPENCODE_KEY")
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(
        std::fs::read_to_string(project.as_path().join("hello.txt")).unwrap(),
        "hi from agent\n"
    );
    task.abort();
}

#[tokio::test(flavor = "multi_thread")]
async fn cli_preserves_incomplete_handoff_and_exits_nonzero() {
    let project = tempfile::tempdir().unwrap();
    let dir = tempfile::tempdir().unwrap();
    let state = AppState::new_for_test(dir.path().join("settings.json"));
    state
        .update_settings(themis_desktop::types::SettingsPatch {
            max_total_turns: Some(1),
            ..Default::default()
        })
        .await
        .unwrap();
    use std::sync::atomic::{AtomicUsize, Ordering};
    use wiremock::matchers::{method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};
    let provider = MockServer::start().await;
    let calls = AtomicUsize::new(0);
    Mock::given(method("POST"))
        .and(path("/chat/completions"))
        .respond_with(move |_: &wiremock::Request| {
            ResponseTemplate::new(200).set_body_json(if calls.fetch_add(1, Ordering::SeqCst) == 0 {
                common::tool_call_body()
            } else {
                common::final_text_body()
            })
        })
        .mount(&provider)
        .await;
    common::use_mock_llm(&state, &provider).await;
    let server = Server::bind(state, dir.path()).await.unwrap();
    let task = tokio::spawn(server.run());
    let thread = cli(
        dir.path(),
        &[
            "thread",
            "create",
            project.path().to_str().unwrap(),
            "test-model",
        ],
    );
    let output = std::process::Command::new(env!("CARGO_BIN_EXE_themis"))
        .arg("--data-dir")
        .arg(dir.path())
        .args([
            "thread",
            "send",
            thread["id"].as_str().unwrap(),
            "write hello.txt",
        ])
        .env_remove("OPENCODE_KEY")
        .output()
        .unwrap();
    assert!(!output.status.success());
    let history = cli(
        dir.path(),
        &["thread", "history", thread["id"].as_str().unwrap()],
    );
    let handoff = history
        .as_array()
        .unwrap()
        .iter()
        .find(|item| item["envelope"]["event"]["kind"] == "incomplete")
        .expect("recorded incomplete handoff");
    let result = handoff["envelope"]["event"]["result"].as_str().unwrap();
    assert!(!result.is_empty());
    assert!(String::from_utf8_lossy(&output.stdout).contains(result));
    assert!(String::from_utf8_lossy(&output.stderr).contains("unfinished work"));
    task.abort();
}

#[tokio::test(flavor = "multi_thread")]
async fn appearance_preferences_roundtrip_through_server_and_cli() {
    let data_dir = tempfile::tempdir().expect("data dir");
    let settings_path = data_dir.path().join("settings.json");
    let state = AppState::new_for_test(settings_path.clone());
    let server = Server::bind(state, data_dir.path()).await.expect("bind");
    let task = tokio::spawn(server.run());
    let args = json!({"patch": {"theme_palette": "ocean", "font_family": "mono", "text_size": 22}})
        .to_string();
    let saved = cli(data_dir.path(), &["call", "update_settings", &args]);
    assert_eq!(saved["theme_palette"], "ocean");
    assert_eq!(saved["font_family"], "mono");
    assert_eq!(saved["text_size"], 22);
    let loaded = themis_desktop::settings::SettingsStore::load(settings_path)
        .get()
        .await;
    assert_eq!(loaded.theme_palette, "ocean");
    assert_eq!(loaded.font_family, "mono");
    assert_eq!(loaded.text_size, 22);
    let invalid = Client::new(data_dir.path().to_path_buf())
        .call(
            "update_settings",
            json!({"patch": {"font_family": "unknown"}}),
        )
        .await;
    assert!(invalid.is_err());
    assert_eq!(
        cli(data_dir.path(), &["call", "get_settings", "{}"])["font_family"],
        "mono"
    );
    task.abort();
}

#[tokio::test(flavor = "multi_thread")]
async fn fixed_workspace_and_dark_presets_survive_settings_changes_and_restart() {
    let data = tempfile::tempdir().unwrap();
    let settings_path = data.path().join("settings.json");
    std::fs::write(
        &settings_path,
        serde_json::to_vec(&json!({
            "theme": "light", "default_provider": "go", "default_model": "test",
            "max_turns": 20
        }))
        .unwrap(),
    )
    .unwrap();
    let state = AppState::new_for_test(settings_path.clone());
    let (first, second) = tokio::join!(state.get_default_project(), state.get_default_project());
    assert_eq!(first.unwrap().root, second.unwrap().root);
    let server = Server::bind(state, data.path()).await.unwrap();
    let task = tokio::spawn(server.run());
    let project = cli(data.path(), &["call", "get_default_project", "{}"]);
    assert_eq!(project["name"], "Themis");
    assert_eq!(project["is_default"], true);
    assert_eq!(project["is_git"], true);
    let root = project["root"].as_str().unwrap();
    std::fs::write(Path::new(root).join("keep.txt"), "keep my work").unwrap();
    let args = json!({"patch": {
        "theme": "system", "theme_palette": "github-dimmed",
        "font_family": "mono", "text_size": 18
    }})
    .to_string();
    let saved = cli(data.path(), &["call", "update_settings", &args]);
    assert_eq!(saved["theme"], "dark");
    assert_eq!(saved["theme_palette"], "github-dimmed");
    assert_eq!(
        cli(data.path(), &["call", "get_default_project", "{}"]),
        project
    );
    let thread = cli(data.path(), &["thread", "create", root]);
    assert!(thread["id"].is_string());
    task.abort();
    let reopened = AppState::new_for_test(settings_path);
    assert_eq!(
        reopened.get_settings().await.theme,
        themis_desktop::types::ThemeMode::Dark
    );
    assert_eq!(reopened.get_settings().await.font_family, "mono");
    assert_eq!(
        std::fs::read_to_string(Path::new(root).join("keep.txt")).unwrap(),
        "keep my work"
    );
    let server = Server::bind(reopened, data.path()).await.unwrap();
    let task = tokio::spawn(server.run());
    assert_eq!(
        cli(data.path(), &["call", "get_default_project", "{}"]),
        project
    );
    task.abort();
}

#[tokio::test(flavor = "multi_thread")]
async fn projects_use_one_fixed_root_and_rename_preserves_threads_after_restart() {
    let data = tempfile::tempdir().unwrap();
    let path = data.path().join("settings.json");
    let state = AppState::new_for_test(path.clone());
    let fixed = state.get_settings().await.projects_directory;
    let server = Server::bind(state, data.path()).await.unwrap();
    let task = tokio::spawn(server.run());
    let client = Client::new(data.path().to_path_buf());
    assert!(client
        .call(
            "update_settings",
            json!({"patch":{"projects_directory":data.path().join("escape")}})
        )
        .await
        .unwrap_err()
        .contains("fixed"));
    assert!(client
        .call(
            "create_project",
            json!({"name":"Escaped","directory":data.path().join("escape")})
        )
        .await
        .unwrap_err()
        .contains("fixed"));
    let project = cli(
        data.path(),
        &["call", "create_project", "{\"name\":\"Research\"}"],
    );
    let root = project["root"].as_str().unwrap();
    assert_eq!(
        Path::new(root).parent().unwrap(),
        Path::new(&fixed).canonicalize().unwrap()
    );
    let thread = cli(data.path(), &["thread", "create", root]);
    std::fs::write(Path::new(root).join("keep.txt"), "preserve").unwrap();
    let renamed = cli(
        data.path(),
        &[
            "call",
            "rename_project",
            &json!({"path":root,"name":"Research notes"}).to_string(),
        ],
    );
    assert_eq!(renamed["name"], "Research notes");
    assert_eq!(renamed["root"], root);
    let default = cli(data.path(), &["call", "get_default_project", "{}"]);
    let renamed_default = client
        .call(
            "rename_project",
            json!({"path":default["root"],"name":"My workspace"}),
        )
        .await
        .unwrap();
    assert_eq!(renamed_default["root"], default["root"]);
    assert_eq!(renamed_default["name"], "My workspace");
    task.abort();
    let reopened = AppState::new_for_test(path);
    let restored_default = reopened.get_default_project().await.unwrap();
    assert_eq!(restored_default.name, "My workspace");
    assert_eq!(restored_default.root, default["root"].as_str().unwrap());
    assert_eq!(
        reopened.open_project(root.to_owned()).await.unwrap().name,
        "Research notes"
    );
    assert_eq!(
        reopened
            .get_thread(thread["id"].as_str().unwrap())
            .await
            .unwrap()
            .id,
        thread["id"]
    );
    assert_eq!(
        std::fs::read_to_string(Path::new(root).join("keep.txt")).unwrap(),
        "preserve"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn cli_stop_releases_stalled_compaction_and_restores_chat() {
    use wiremock::{matchers::method, Mock, MockServer, ResponseTemplate};
    let data = tempfile::tempdir().unwrap();
    let project = tempfile::tempdir().unwrap();
    let state = AppState::new_for_test(data.path().join("settings.json"));
    state
        .update_settings(themis_desktop::types::SettingsPatch {
            context_token_budget: Some(2000),
            ..Default::default()
        })
        .await
        .unwrap();
    let provider = MockServer::start().await;
    Mock::given(method("POST"))
        .respond_with(ResponseTemplate::new(200).set_delay(Duration::from_secs(30)).set_body_json(json!({"choices":[{"message":{"role":"assistant","content":"Late checkpoint"},"finish_reason":"stop"}]})))
        .mount(&provider).await;
    state
        .set_secret("go".into(), "test-key".into())
        .await
        .unwrap();
    state.set_go_base_url_override(Some(provider.uri()));
    let server = Server::bind(state.clone(), data.path()).await.unwrap();
    let serving = tokio::spawn(server.run());
    let thread = cli(
        data.path(),
        &[
            "thread",
            "create",
            project.path().to_str().unwrap(),
            "test-model",
        ],
    );
    let id = thread["id"].as_str().unwrap();
    let mut events = Client::new(data.path().into()).subscribe().await.unwrap();
    cli(
        data.path(),
        &[
            "thread",
            "send",
            id,
            &"Original task ".repeat(3000),
            "--detach",
        ],
    );
    tokio::time::timeout(Duration::from_secs(3), async {
        loop {
            let event = Client::next_event(&mut events).await.unwrap();
            if event["name"] == "thread-event"
                && event["payload"]["event"]["kind"] == "context_compacting"
            {
                break;
            }
        }
        while provider.received_requests().await.unwrap().is_empty() {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .expect("Compaction must reach the stalled mock provider");
    cli(data.path(), &["thread", "stop", id]);
    let terminal = tokio::time::timeout(Duration::from_secs(1), async {
        loop {
            let event = Client::next_event(&mut events).await.unwrap();
            if event["name"] == "thread-event" && event["payload"]["event"]["kind"] == "failed" {
                break event;
            }
        }
    })
    .await
    .expect("CLI Stop must release stalled compaction promptly");
    assert!(terminal["payload"]["event"]["error"]
        .as_str()
        .unwrap()
        .contains("Stopped by you"));
    let threads = Client::new(data.path().into())
        .call("list_threads", json!({"projectRoot":project.path()}))
        .await
        .unwrap();
    assert_eq!(
        threads
            .as_array()
            .unwrap()
            .iter()
            .find(|thread| thread["id"] == id)
            .unwrap()["running"],
        false
    );
    Client::new(data.path().into())
        .call("shutdown", json!({}))
        .await
        .unwrap();
    serving.await.unwrap().unwrap();
}
