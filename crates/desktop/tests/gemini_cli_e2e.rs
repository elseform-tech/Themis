//! A Gemini model's tool round trip through the shared server and the `themis` CLI: the
//! request goes to Gemini's `generateContent` path with Go's headers, the tool result goes
//! back as a function response, and the step's thought signature returns with its call.
//! The provider is scripted, so this checks the wiring, not live model quality.

use serde_json::{json, Value};
use std::path::Path;
use themis_desktop::{
    server::{Client, Server},
    state::AppState,
};
use wiremock::{matchers::method, Mock, MockServer, Request, ResponseTemplate};

fn cli(dir: &Path, command: &str, args: Value) -> Value {
    let output = std::process::Command::new(env!("CARGO_BIN_EXE_themis"))
        .arg("--data-dir")
        .arg(dir)
        .args(["call", command, &args.to_string()])
        .env_remove("OPENCODE_KEY")
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    serde_json::from_slice(&output.stdout).unwrap()
}

/// A Gemini reply with `parts`, streamed as one event or sent whole, as the request asks.
fn reply(request: &Request, parts: Value) -> ResponseTemplate {
    let body = json!({"candidates": [{"finishReason": "STOP", "content": {"role": "model", "parts": parts}}],
                      "usageMetadata": {"promptTokenCount": 30, "candidatesTokenCount": 6, "totalTokenCount": 36}});
    if request.url.path().ends_with(":streamGenerateContent") {
        ResponseTemplate::new(200)
            .set_body_raw(format!("data: {body}\r\n\r\n"), "text/event-stream")
    } else {
        ResponseTemplate::new(200).set_body_json(body)
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn cli_gemini_chat_reads_a_file_and_returns_the_signature() {
    let data = tempfile::tempdir().unwrap();
    let project = tempfile::tempdir().unwrap();
    std::fs::write(project.path().join("note.txt"), "hello gemini\n").unwrap();
    let provider = MockServer::start().await;
    Mock::given(method("POST"))
        .respond_with(|request: &Request| {
            let body: Value = request.body_json().unwrap();
            let answered = body["contents"]
                .as_array()
                .unwrap()
                .iter()
                .flat_map(|content| content["parts"].as_array().cloned().unwrap_or_default())
                .any(|part| part["functionResponse"].is_object());
            if answered {
                reply(request, json!([{"text": "The note says hello gemini."}]))
            } else {
                reply(
                    request,
                    json!([{"thoughtSignature": "SIG-E2E", "functionCall": {"id": "call_e2e", "name": "read_file", "args": {"file_path": "note.txt"}}}]),
                )
            }
        })
        .mount(&provider)
        .await;
    let state = AppState::new_for_test(data.path().join("settings.json"));
    state
        .set_secret("go".into(), "test-key".into())
        .await
        .unwrap();
    state.set_go_base_url_override(Some(provider.uri()));
    let server = Server::bind(state, data.path()).await.unwrap();
    let serving = tokio::spawn(server.run());
    let client = Client::new(data.path().into());
    let mut events = client.subscribe().await.unwrap();

    let thread = cli(
        data.path(),
        "create_thread",
        json!({"projectRoot": project.path(), "provider": "go", "model": "gemini-test"}),
    );
    let id = thread["id"].as_str().unwrap().to_owned();
    cli(
        data.path(),
        "send_message",
        json!({"threadId": id, "text": "What does note.txt say?", "reasoningEffort": null}),
    );
    let done = tokio::time::timeout(std::time::Duration::from_secs(120), async {
        loop {
            let item = Client::next_event(&mut events).await.unwrap();
            if item["name"] != "thread-event" || item["payload"]["thread_id"] != id.as_str() {
                continue;
            }
            let event = item["payload"]["event"].clone();
            if ["finished", "failed", "incomplete"]
                .contains(&event["kind"].as_str().unwrap_or_default())
            {
                return event;
            }
        }
    })
    .await
    .unwrap();
    assert_eq!(done["kind"], "finished", "{done}");

    let requests: Vec<_> = provider
        .received_requests()
        .await
        .unwrap()
        .into_iter()
        .filter(|request| request.url.path().contains("/models/gemini-test:"))
        .collect();
    assert!(requests.len() >= 2, "{} Gemini requests", requests.len());
    for request in &requests {
        assert_eq!(request.headers["x-goog-api-key"], "test-key");
        assert!(request.headers.contains_key("x-opencode-session"));
    }
    let follow_up: Value = requests[1].body_json().unwrap();
    let parts: Vec<Value> = follow_up["contents"]
        .as_array()
        .unwrap()
        .iter()
        .flat_map(|content| content["parts"].as_array().cloned().unwrap_or_default())
        .collect();
    let call = parts
        .iter()
        .find(|part| part["functionCall"]["id"] == "call_e2e")
        .expect("the call goes back");
    assert_eq!(call["thoughtSignature"], "SIG-E2E");
    let response = parts
        .iter()
        .find(|part| part["functionResponse"]["id"] == "call_e2e")
        .expect("the result goes back");
    assert_eq!(response["functionResponse"]["name"], "read_file");
    assert!(
        response["functionResponse"]["response"]["result"]
            .as_str()
            .unwrap()
            .contains("hello gemini"),
        "{response}"
    );

    let stop = std::process::Command::new(env!("CARGO_BIN_EXE_themis"))
        .arg("--data-dir")
        .arg(data.path())
        .args(["server", "stop"])
        .output()
        .unwrap();
    assert!(stop.status.success());
    serving.await.unwrap().unwrap();
}
