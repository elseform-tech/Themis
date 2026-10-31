//! Every turn that worked with tools ends with a message to the user, through the shared
//! server and the `themis` CLI: a run that stops on a short progress line is asked once to
//! continue or sum up, and the chat's final answer is that summary.

use std::path::Path;
use std::time::Duration;

use serde_json::{json, Value};
use themis_desktop::{server::Server, state::AppState, types::ProviderKind};
use wiremock::matchers::method;
use wiremock::{Mock, MockServer, Request, ResponseTemplate};

const TURN_SUMMARY: &str =
    "[Runtime note, not from the user] You ended this turn saying what comes next, without doing it.";
const SUMMARY: &str = "I listed the project and found it empty, so nothing changed. Next: tell me which files to add.";

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

fn reply(message: Value) -> ResponseTemplate {
    ResponseTemplate::new(200)
        .set_body_json(json!({"choices":[{"message":message,"finish_reason":"stop"}]}))
}

#[tokio::test(flavor = "multi_thread")]
async fn cli_a_turn_that_stops_on_a_progress_line_ends_with_its_summary() {
    let llm = MockServer::start().await;
    Mock::given(method("POST"))
        .respond_with(|request: &Request| {
            let body: Value = request.body_json().unwrap();
            let messages = body["messages"].as_array().unwrap();
            let last = messages.last().unwrap()["content"].as_str().unwrap_or_default().to_owned();
            if last.starts_with(TURN_SUMMARY) {
                reply(json!({"role":"assistant","content":SUMMARY}))
            } else if !messages.iter().any(|message| message["role"] == "tool") {
                reply(json!({"role":"assistant","content":"Looking.","tool_calls":[{"id":"l","type":"function","function":{"name":"list_dir","arguments":"{\"path\":\".\"}"}}]}))
            } else {
                reply(json!({"role":"assistant","content":"Continuing."}))
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
        .unwrap();
    let server = Server::bind(state.clone(), data.path()).await.unwrap();
    let task = tokio::spawn(server.run());
    let data_dir = data.path().to_owned();
    let thread_id = thread.id.clone();
    let history = tokio::task::spawn_blocking(move || {
        call(
            &data_dir,
            "send_message",
            json!({"threadId": thread_id, "text": "Look at the project."}),
        );
        let deadline = std::time::Instant::now() + Duration::from_secs(60);
        loop {
            let history = call(
                &data_dir,
                "get_thread_history",
                json!({"threadId": thread_id}),
            );
            if history.to_string().contains("\"finished\"") {
                return history;
            }
            assert!(
                std::time::Instant::now() < deadline,
                "the run never finished: {history}"
            );
            std::thread::sleep(Duration::from_millis(200));
        }
    })
    .await
    .unwrap();
    let finished: Vec<&Value> = history
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|item| {
            item.pointer("/envelope/event")
                .filter(|event| event["kind"] == "finished")
        })
        .collect();
    assert_eq!(finished.len(), 1, "{history}");
    assert_eq!(finished[0]["result"], SUMMARY);
    let asked = llm
        .received_requests()
        .await
        .unwrap()
        .iter()
        .filter(|request| {
            request.body_json::<Value>().unwrap()["messages"]
                .as_array()
                .unwrap()
                .last()
                .unwrap()["content"]
                .as_str()
                .is_some_and(|text| text.starts_with(TURN_SUMMARY))
        })
        .count();
    assert_eq!(asked, 1);
    task.abort();
}
