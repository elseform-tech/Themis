//! `/init` through the real server and CLI: the composer lists it, every run's skill catalog
//! offers it, its SKILL.md reaches the model, and the run writes the project's AGENTS.md.

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

const SKILL: &str = include_str!("../../core/builtins/init/SKILL.md");
const WRITTEN: &str = "# AGENTS.md\n\nRun `cargo test` before claiming a change works.\n";

#[tokio::test(flavor = "multi_thread")]
async fn init_sends_its_instruction_and_the_run_writes_agents_md() {
    let llm = MockServer::builder().start().await;
    Mock::given(method("POST"))
        .respond_with(move |request: &Request| {
            let body: Value = request.body_json().unwrap_or(Value::Null);
            let messages = body["messages"].as_array().cloned().unwrap_or_default();
            let wrote = messages.iter().any(|m| m["role"] == "tool");
            let asked = messages
                .iter()
                .rev()
                .find(|m| m["role"] == "user")
                .and_then(|m| m["content"].as_str())
                .is_some_and(|text| text.contains("## Skill: Init"));
            if !asked {
                ResponseTemplate::new(200).set_body_json(json!({
                    "choices": [{"message": {"role": "assistant", "content": "Noted."}, "finish_reason": "stop"}]
                }))
            } else if wrote {
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
        for text in ["Hello", "[[skill:init]]"] {
            call(&dir, "send_message", json!({"threadId": id, "text": text}));
            let deadline = Instant::now() + Duration::from_secs(30);
            while call(&dir, "get_thread", json!({"threadId": id}))["running"] == true {
                assert!(Instant::now() < deadline, "the run never finished");
                std::thread::sleep(Duration::from_millis(50));
            }
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
    let description = SKILL
        .lines()
        .find_map(|line| line.strip_prefix("description: "))
        .expect("SKILL.md frontmatter has a description");
    assert_eq!(init["description"], description);
    for trigger in ["AGENTS.md", "CLAUDE.md", "/init"] {
        assert!(description.contains(trigger), "{trigger}: {description}");
    }

    // An ordinary message's run offers it in the system message's skill catalog with that
    // description, so the model can load it unprompted.
    let requests = llm.received_requests().await.unwrap();
    let plain: Value = requests[0].body_json().unwrap();
    let system = plain["messages"][0]["content"].as_str().unwrap_or_default();
    assert!(system.contains("Available skills"), "{system}");
    assert!(system.contains(r#""id":"init""#), "{system}");
    assert!(
        system.contains(&serde_json::to_string(description).unwrap()),
        "init's description missing from the skill catalog: {system}"
    );

    // `/init` puts the SKILL.md into the person's message, word for word.
    let first: Value = requests[1].body_json().unwrap();
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
        user.contains(SKILL),
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
