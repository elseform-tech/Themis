//! A tool call a model writes as text never reaches the chat, through the real server and CLI.

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

fn reply(text: &str) -> ResponseTemplate {
    ResponseTemplate::new(200).set_body_json(json!({
        "choices": [{"message": {"role": "assistant", "content": text}, "finish_reason": "stop"}]
    }))
}

#[tokio::test(flavor = "multi_thread")]
async fn a_written_call_and_the_planning_after_it_stay_out_of_the_chat_and_the_next_request() {
    const LEAK: &str = "Committed both.\n\n<|func_arg_name|>args<|func_arg_value|>[\"git\"]\n\nAnother shell call: SECRET-PLAN";
    let llm = MockServer::builder().start().await;
    Mock::given(method("POST"))
        .respond_with(move |request: &Request| {
            let body: Value = request.body_json().unwrap_or(Value::Null);
            let asked_again = body["messages"]
                .as_array()
                .is_some_and(|all| all.iter().filter(|m| m["role"] == "assistant").count() > 0);
            if asked_again {
                reply("I committed hello.py and the ignore file.")
            } else {
                reply(LEAK)
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
    let server = Server::bind(state, data.path()).await.unwrap();
    let serving = tokio::spawn(async move {
        let _ = server.run().await;
    });
    let (dir, id) = (data.path().to_owned(), thread.clone());
    let history = tokio::task::spawn_blocking(move || {
        call(&dir, "send_message", json!({"threadId": id, "text": "go"}));
        let deadline = Instant::now() + Duration::from_secs(30);
        while call(&dir, "get_thread", json!({"threadId": id}))["running"] == true {
            assert!(Instant::now() < deadline, "the run never finished");
            std::thread::sleep(Duration::from_millis(50));
        }
        call(&dir, "get_thread_history", json!({"threadId": id}))
    })
    .await
    .unwrap();
    let seen = history.to_string();
    assert!(seen.contains("Committed both."), "{seen}");
    assert!(seen.contains("I committed hello.py"), "{seen}");
    for leaked in ["func_arg", "SECRET-PLAN", "Another shell call"] {
        assert!(!seen.contains(leaked), "{leaked}: {seen}");
    }
    let requests = llm.received_requests().await.unwrap();
    assert!(requests.len() >= 2, "it wasn't asked again");
    let last = String::from_utf8_lossy(&requests.last().unwrap().body).into_owned();
    assert!(!last.contains("SECRET-PLAN"), "{last}");
    serving.abort();
}
