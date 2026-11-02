//! `/init` through the real server and CLI: the composer lists it, its instruction reaches the
//! model, and the run it starts writes the project's AGENTS.md.

use std::path::Path;
use std::time::{Duration, Instant};

use serde_json::{json, Value};
use themis_desktop::{server::Server, state::AppState, types::ProviderKind};
use wiremock::matchers::method;
use wiremock::{Mock, MockServer, Request, ResponseTemplate};

fn call(data: &Path, command: &str, args: Value) -> Value {
    let output = std::process::Command::new(env!("CARGO_BIN_EXE_themis"))
        .arg("--data-dir")
        .arg(data)
        .args(["call", command, &args.to_string()])
        .env("HOME", data)
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

const WRITTEN: &str = "# AGENTS.md\n\nRun `cargo test` before claiming a change works.\n";

#[tokio::test(flavor = "multi_thread")]
async fn init_sends_its_instruction_and_the_run_writes_agents_md() {
    let llm = MockServer::builder().start().await;
    Mock::given(method("POST"))
        .respond_with(move |request: &Request| {
            let body: Value = request.body_json().unwrap_or(Value::Null);
            let wrote = body["messages"]
                .as_array()
                .is_some_and(|all| all.iter().any(|m| m["role"] == "tool"));
            if wrote {
                ResponseTemplate::new(200).set_body_json(json!({
                    "choices": [{"message": {"role": "assistant", "content": "Wrote AGENTS.md."}, "finish_reason": "stop"}]
                }))
            } else {
                ResponseTemplate::new(200).set_body_json(json!({
                    "choices": [{"message": {"tool_calls": [{"id": "write-guidance", "type": "function", "function": {
                        "name": "write_file",
                        "arguments": json!({"file_path": "AGENTS.md", "content": WRITTEN}).to_string()
                    }}]}, "finish_reason": "tool_calls"}]
                }))
            }
        })
        .mount(&llm)
        .await;
    let data = tempfile::tempdir().unwrap();
    let project = tempfile::tempdir().unwrap();
    let state = AppState::new_for_test(data.path().join("settings.json"));
    state
        .set_secret("go".into(), "synthetic-test-key".into())
        .await
        .unwrap();
    state.set_go_base_url_override(Some(llm.uri()));
    let thread = state
        .create_thread(
            project.path().to_string_lossy().into(),
            ProviderKind::Go,
            Some("test-model".into()),
        )
        .await
        .unwrap()
        .id;
    state
        .set_thread_approval_mode(
            thread.clone(),
            themis_core::configuration::ApprovalMode::Yolo,
        )
        .await
        .unwrap();
    let server = Server::bind(state, data.path()).await.unwrap();
    let serving = tokio::spawn(async move {
        let _ = server.run().await;
    });
    let (dir, id) = (data.path().to_owned(), thread.clone());
    let (listed, history) = tokio::task::spawn_blocking(move || {
        let listed = call(&dir, "list_prompt_skills", json!({"projectRoot": null}));
        call(
            &dir,
            "send_message",
            json!({"threadId": id, "text": "[[skill:init]]"}),
        );
        let deadline = Instant::now() + Duration::from_secs(30);
        while call(&dir, "get_thread", json!({"threadId": id}))["running"] == true {
            assert!(Instant::now() < deadline, "the run never finished");
            std::thread::sleep(Duration::from_millis(50));
        }
        (
            listed,
            call(&dir, "get_thread_history", json!({"threadId": id})),
        )
    })
    .await
    .unwrap();
    serving.abort();

    // The composer's `/` menu lists it as a built-in, like `/hooks`.
    let init = listed
        .as_array()
        .unwrap()
        .iter()
        .find(|skill| skill["id"] == "init")
        .unwrap_or_else(|| panic!("no init skill in {listed}"));
    assert_eq!(init["scope"], "built-in");
    assert_eq!(init["name"], "Init");

    // The instruction reaches the model in the person's message, word for word.
    let requests = llm.received_requests().await.unwrap();
    let first: Value = requests[0].body_json().unwrap();
    let user = first["messages"]
        .as_array()
        .unwrap()
        .iter()
        .rev()
        .find(|m| m["role"] == "user")
        .and_then(|m| m["content"].as_str())
        .unwrap_or_default()
        .to_owned();
    assert!(
        user.contains(themis_core::prompts::SKILL_INIT),
        "init prompt missing from the request: {user}"
    );
    assert!(!user.contains("[[skill:init]]"), "{user}");

    // The run it starts can write the guidance file.
    assert_eq!(
        std::fs::read_to_string(project.path().join("AGENTS.md")).unwrap(),
        WRITTEN
    );
    assert!(
        history.to_string().contains("Wrote AGENTS.md."),
        "{history}"
    );
}
