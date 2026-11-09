//! A person's message about integrations brings the matching built-in guide with its turn: the
//! request the model gets already holds it, with no load call. Real app server, the `themis` CLI
//! (`thread create`, `thread send`), a scripted model, isolated app data.
use serde_json::{json, Value};
use std::path::Path;
use themis_desktop::server::{Client, Server};
use themis_desktop::state::AppState;
use wiremock::matchers::method;
use wiremock::{Mock, MockServer, ResponseTemplate};

#[allow(dead_code)]
mod common;

fn cli(data: &Path, args: &[&str]) -> Value {
    let output = std::process::Command::new(env!("CARGO_BIN_EXE_themis"))
        .arg("--data-dir")
        .arg(data)
        .args(args)
        .env_remove("OPENCODE_KEY")
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "themis {args:?}: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    serde_json::from_slice(&output.stdout).unwrap_or(Value::Null)
}

/// The text of every message of the request, joined.
fn request_text(body: &Value) -> String {
    body["messages"]
        .as_array()
        .unwrap()
        .iter()
        .map(|message| message["content"].to_string())
        .collect()
}

#[tokio::test(flavor = "multi_thread")]
async fn a_message_about_integrations_brings_its_guide_without_a_load_call() {
    let data = tempfile::tempdir().unwrap();
    let project = tempfile::tempdir().unwrap();
    let state = AppState::new_for_test(data.path().join("settings.json"));
    let model = MockServer::builder().start().await;
    Mock::given(method("POST"))
        .respond_with(
            ResponseTemplate::new(200).set_body_json(json!({"choices": [{
            "message": {"role": "assistant", "content": "Noted."}, "finish_reason": "stop"}]})),
        )
        .mount(&model)
        .await;
    common::use_mock_llm(&state, &model).await;
    let server = Server::bind(state, data.path()).await.unwrap();
    let serving = tokio::spawn(server.run());
    let root = project.path().to_str().unwrap();
    let mut sent = 0;
    for (message, expected) in [
        (
            "add an MCP server called notes that runs python3 notes.py",
            &["MCP"][..],
        ),
        (
            "make a hook and a skill for release notes",
            &["Skills", "Hooks"],
        ),
        // Picked explicitly: attached once, not twice.
        ("[[skill:mcp]] fix my mcp server", &["MCP"]),
        ("fix the failing test, I'm hooked on green builds", &[]),
    ] {
        let thread = cli(data.path(), &["thread", "create", root, "test-model"]);
        let id = thread["id"].as_str().unwrap().to_owned();
        let directory = data.path().to_path_buf();
        let text = message.to_owned();
        tokio::task::spawn_blocking(move || cli(&directory, &["thread", "send", &id, &text]))
            .await
            .unwrap();
        let requests = model.received_requests().await.unwrap();
        let body: Value = serde_json::from_slice(&requests[sent].body).unwrap();
        sent = requests.len();
        let text = request_text(&body);
        for guide in ["MCP", "Plugins", "Skills", "Hooks"] {
            let heading = format!("## Skill: {guide}\\n");
            let count = text.matches(&heading).count();
            let wanted = usize::from(expected.contains(&guide));
            assert_eq!(count, wanted, "{message}: {guide} appears {count} times");
        }
        // Nothing was loaded by a tool call: the reply came in one request.
        assert!(!text.contains("\"Skill\""), "{message}");
    }
    let logs: String = std::fs::read_dir(data.path().join("logs"))
        .unwrap()
        .filter_map(Result::ok)
        .map(|entry| std::fs::read_to_string(entry.path()).unwrap_or_default())
        .collect();
    assert!(logs.contains("guides_attached"), "{logs}");
    assert!(logs.contains(r#""guides":["mcp"]"#), "{logs}");
    assert!(!logs.contains("notes.py"), "metadata only: {logs}");
    Client::new(data.path().into())
        .call("shutdown", json!({}))
        .await
        .unwrap();
    serving.await.unwrap().unwrap();
}
