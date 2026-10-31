//! `PromptSubmit` hooks (imported from Claude's `UserPromptSubmit`) through the real server
//! and the `themis` CLI with a scripted model: their context reaches the model labelled,
//! after the person's message, and a hook that blocks stops the turn before any request.

use std::path::Path;
use std::time::{Duration, Instant};

use serde_json::{json, Value};
use themis_desktop::{server::Server, state::AppState, types::ProviderKind};
use wiremock::matchers::method;
use wiremock::{Mock, MockServer, ResponseTemplate};

const LABEL: &str = "A plugin hook added the context below";

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

fn until(what: &str, mut done: impl FnMut() -> bool) {
    let deadline = Instant::now() + Duration::from_secs(60);
    while !done() {
        assert!(Instant::now() < deadline, "{what}");
        std::thread::sleep(Duration::from_millis(100));
    }
}

/// A chat in YOLO whose project has a plugin with one `PromptSubmit` hook running `command`.
async fn chat_with_prompt_hook(
    command: &str,
) -> (
    MockServer,
    tempfile::TempDir,
    tempfile::TempDir,
    String,
    tokio::task::JoinHandle<()>,
) {
    let llm = MockServer::builder().start().await;
    Mock::given(method("POST"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "choices": [{"message": {"role": "assistant", "content": "Done."}, "finish_reason": "stop"}]
        })))
        .mount(&llm)
        .await;
    let data = tempfile::tempdir().unwrap();
    let project = tempfile::tempdir().unwrap();
    std::process::Command::new("git")
        .args(["init", "-q"])
        .current_dir(project.path())
        .status()
        .expect("git");
    let state = AppState::new_for_test(data.path().join("settings.json"));
    state
        .set_secret("go".into(), "synthetic-test-key".into())
        .await
        .unwrap();
    state.set_go_base_url_override(Some(llm.uri()));
    state
        .plugin_action(json!({"action": "save", "scope": "local", "projectRoot": project.path(), "spec": {
            "name": "tickets",
            "hooks": [{"name": "ticket-context", "event": "PromptSubmit", "command": command, "enabled": true}],
            "files": {}
        }}))
        .await
        .unwrap();
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
    (llm, data, project, thread, serving)
}

fn run(data: &Path, thread: &str, text: &str) {
    call(
        data,
        "set_thread_approval_mode",
        json!({"threadId": thread, "mode": "yolo"}),
    );
    call(
        data,
        "send_message",
        json!({"threadId": thread, "text": text}),
    );
    until("the run never finished", || {
        call(data, "get_thread", json!({"threadId": thread}))["running"] == false
    });
}

#[tokio::test(flavor = "multi_thread")]
async fn prompt_hook_context_reaches_the_model_after_the_message() {
    let (llm, data, _project, thread, serving) =
        chat_with_prompt_hook("echo Ticket ABC-1 is open").await;
    let dir = data.path().to_owned();
    tokio::task::spawn_blocking(move || run(&dir, &thread, "fix the login bug"))
        .await
        .unwrap();
    let requests = llm.received_requests().await.unwrap();
    let body: Value = serde_json::from_slice(&requests[0].body).unwrap();
    let messages = body["messages"].as_array().unwrap();
    let text = |m: &Value| m["content"].as_str().unwrap_or_default().to_owned();
    let request = messages
        .iter()
        .position(|m| text(m) == "fix the login bug")
        .unwrap();
    let context = text(&messages[request + 1]);
    assert_eq!(messages[request + 1]["role"], "user");
    assert!(context.starts_with(LABEL), "{context}");
    assert!(
        context.contains("<<<hook-context\nTicket ABC-1 is open\n>>>"),
        "{context}"
    );
    serving.abort();
}

#[tokio::test(flavor = "multi_thread")]
async fn a_prompt_hook_that_blocks_stops_the_turn_before_any_request() {
    // cmd keeps single quotes in the output and treats double quotes as plain text.
    let command = if cfg!(windows) {
        r#"echo {"decision":"block","reason":"no tickets on Fridays"}"#
    } else {
        r#"echo '{"decision":"block","reason":"no tickets on Fridays"}'"#
    };
    let (llm, data, _project, thread, serving) = chat_with_prompt_hook(command).await;
    let dir = data.path().to_owned();
    let history = tokio::task::spawn_blocking(move || {
        run(&dir, &thread, "fix the login bug");
        call(&dir, "get_thread_history", json!({"threadId": thread}))
    })
    .await
    .unwrap();
    assert!(
        llm.received_requests().await.unwrap().is_empty(),
        "no model request"
    );
    let items = history
        .as_array()
        .cloned()
        .or_else(|| history["items"].as_array().cloned())
        .unwrap();
    let failed = items
        .iter()
        .filter_map(|item| item["envelope"]["event"].as_object())
        .find(|event| event["kind"] == "failed")
        .expect("the turn failed");
    assert_eq!(
        failed["error"],
        "Blocked by hook ticket-context: no tickets on Fridays"
    );
    serving.abort();
}
