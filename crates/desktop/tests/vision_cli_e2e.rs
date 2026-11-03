//! Screenshots reach a model that reads images, through the real shared server and the
//! `themis` CLI, against a local mock OpenAI-compatible provider (no real credentials): a
//! scripted model opens a local page and takes a screenshot. On a model whose `models`
//! override says it reads images, the next request carries the screenshot as a JPEG `data:`
//! URL in a user message right after the tool result, the tool is described as giving the
//! image, and its result says so. On a model that reads text only, nothing changes: no
//! image, the same description and note as before. The diagnostics log records how many images and bytes went out, never the image.
//! Skipped without a Chromium.

use std::path::Path;
use std::time::Duration;

use serde_json::{json, Value};
use themis_desktop::{
    server::{Client, Server},
    state::AppState,
};
use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, Request, ResponseTemplate};

fn cli(dir: &Path, command: &str, args: Value) -> Value {
    let output = std::process::Command::new(env!("CARGO_BIN_EXE_themis"))
        .arg("--data-dir")
        .arg(dir)
        .args(["call", command, &args.to_string()])
        .env_remove("OPENCODE_KEY")
        .env_remove("LOCAL_KEY")
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{command}: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    serde_json::from_slice(&output.stdout).unwrap_or(Value::Null)
}

fn tool_call(id: &str, name: &str, arguments: Value) -> ResponseTemplate {
    ResponseTemplate::new(200).set_body_json(json!({
        "choices": [{"message": {"role": "assistant", "content": null, "tool_calls": [{
            "id": id, "type": "function",
            "function": {"name": name, "arguments": arguments.to_string()}
        }]}, "finish_reason": "tool_calls"}]
    }))
}

fn tool_results(body: &Value) -> Vec<Value> {
    body["messages"]
        .as_array()
        .into_iter()
        .flatten()
        .filter(|message| message["role"] == "tool")
        .map(|message| {
            serde_json::from_str(message["content"].as_str().unwrap_or_default())
                .unwrap_or_else(|_| message["content"].clone())
        })
        .collect()
}

/// The chat requests the mock received for `model`, in order.
async fn requests_for(provider: &MockServer, model: &str) -> Vec<Value> {
    provider
        .received_requests()
        .await
        .unwrap()
        .into_iter()
        .filter(|request| request.url.path() == "/v1/chat/completions")
        .filter_map(|request| request.body_json::<Value>().ok())
        .filter(|body| body["model"] == model)
        .collect()
}

fn screenshot_description(body: &Value) -> String {
    body["tools"]
        .as_array()
        .into_iter()
        .flatten()
        .find(|tool| tool["function"]["name"] == "browser_screenshot")
        .map(|tool| tool["function"]["description"].as_str().unwrap().to_owned())
        .unwrap_or_default()
}

/// On the app's own runtime, as in `main`: a run's future with a loaded catalog needs its
/// larger worker stack in debug builds.
#[test]
fn cli_screenshots_reach_only_a_model_that_reads_images() {
    if themis_core::browser::launch::find_browser(None, None).is_none() {
        eprintln!("skipping: no Chromium browser installed");
        return;
    }
    themis_desktop::install_runtime();
    themis_desktop::runtime()
        .unwrap()
        .block_on(screenshots_reach_only_a_model_that_reads_images());
}

async fn screenshots_reach_only_a_model_that_reads_images() {
    // Never a real key: the in-process server reads keys from the environment too.
    std::env::remove_var("OPENCODE_KEY");
    std::env::remove_var("LOCAL_KEY");
    let site = MockServer::builder().start().await;
    let port = site.address().port();
    Mock::given(path("/app"))
        .respond_with(ResponseTemplate::new(200).set_body_raw(
            "<!doctype html><title>Board</title><h1 style=\"color:red\">Red heading</h1>",
            "text/html; charset=utf-8",
        ))
        .mount(&site)
        .await;
    let provider = MockServer::builder().start().await;
    Mock::given(method("GET"))
        .and(path("/v1/models"))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_json(json!({"object":"list","data":[{"id":"vision"},{"id":"text"}]})),
        )
        .mount(&provider)
        .await;
    Mock::given(method("POST"))
        .and(path("/v1/chat/completions"))
        .respond_with(move |request: &Request| {
            let body: Value = request.body_json().unwrap_or(Value::Null);
            match tool_results(&body).len() {
                0 => tool_call(
                    "c1",
                    "browser_open",
                    json!({"url": format!("http://127.0.0.1:{port}/app")}),
                ),
                1 => tool_call("c2", "browser_screenshot", json!({})),
                _ => ResponseTemplate::new(200).set_body_json(json!({
                    "choices": [{"message": {"role": "assistant", "content": "The heading is red."}, "finish_reason": "stop"}]
                })),
            }
        })
        .mount(&provider)
        .await;

    let data = tempfile::tempdir().unwrap();
    let project = tempfile::tempdir().unwrap();
    std::fs::write(
        data.path().join("runtime.jsonc"),
        json!({
            "providers": [{"id": "local", "name": "Local mock", "base_url": format!("{}/v1", provider.uri()),
                           "secret": "${LOCAL_KEY}"}],
            "models": {"local/vision": {"images": true}},
            "approval": {"rules": [
                {"tool": "browser_*", "command": ["127.0.0.1"], "action": "allow"},
                {"tool": "*", "risk": "read", "action": "allow"}
            ]}
        })
        .to_string(),
    )
    .unwrap();
    let state = AppState::new_for_test(data.path().join("settings.json"));
    state.set_go_base_url_override(Some(provider.uri()));
    let server = Server::bind(state, data.path()).await.unwrap();
    let serving = tokio::spawn(server.run());
    let client = Client::new(data.path().into());
    let mut events = client.subscribe().await.unwrap();
    let dir = data.path().to_owned();
    let root = project.path().to_owned();

    let ids = tokio::task::spawn_blocking(move || {
        cli(&dir, "set_key", json!({"name": "LOCAL_KEY", "value": "local-test-key"}));
        ["local/vision", "local/text"].map(|model| {
            let thread = cli(
                &dir,
                "create_thread",
                json!({"projectRoot": root, "provider": "go", "model": model}),
            );
            let id = thread["id"].as_str().unwrap().to_owned();
            cli(
                &dir,
                "send_message",
                json!({"threadId": id, "text": "say what colour the heading is", "reasoningEffort": null}),
            );
            id
        })
    })
    .await
    .unwrap();

    // Each run finishes.
    let mut finished = std::collections::HashSet::new();
    tokio::time::timeout(Duration::from_secs(120), async {
        while finished.len() < ids.len() {
            let item = Client::next_event(&mut events).await.unwrap();
            let thread = item["payload"]["thread_id"].as_str().unwrap_or_default();
            if item["name"] != "thread-event" || !ids.iter().any(|id| id == thread) {
                continue;
            }
            let event = &item["payload"]["event"];
            let kind = event["kind"].as_str().unwrap_or_default();
            if ["failed", "incomplete"].contains(&kind) {
                panic!("{event}");
            }
            if kind == "finished" {
                finished.insert(thread.to_owned());
            }
        }
    })
    .await
    .expect("both runs finish");
    // The model that reads images: the screenshot follows its tool result.
    let vision = requests_for(&provider, "vision").await;
    assert_eq!(vision.len(), 3, "open, screenshot, answer");
    let last = &vision[2];
    let messages = last["messages"].as_array().unwrap();
    let tool_at = messages
        .iter()
        .rposition(|message| message["role"] == "tool")
        .unwrap();
    let result: Value =
        serde_json::from_str(messages[tool_at]["content"].as_str().unwrap()).unwrap();
    assert_eq!(result["image_attached"], true, "{result}");
    assert_eq!(
        result["note"],
        themis_core::prompts::TOOL_SCREENSHOT_SEEN_NOTE
    );
    let shot = result["path"].as_str().unwrap();
    assert!(shot.ends_with(".png"), "{shot}");
    let image = &messages[tool_at + 1];
    assert_eq!(image["role"], "user", "{image}");
    assert!(image["content"][0]["text"].as_str().unwrap().contains(shot));
    assert_eq!(image["content"][1]["type"], "image_url");
    let url = image["content"][1]["image_url"]["url"].as_str().unwrap();
    assert!(url.starts_with("data:image/jpeg;base64,/9j/"), "a JPEG");
    assert!(url.len() < 2_100_000, "bounded: {} bytes", url.len());
    assert_eq!(
        screenshot_description(last),
        themis_core::prompts::TOOL_SCREENSHOT_SEEING_DESCRIPTION
    );
    // Only requests after the screenshot carry it.
    assert!(!vision[1].to_string().contains("image_url"));
    // The model that reads text only: exactly the text result it always had.
    let text = requests_for(&provider, "text").await;
    assert_eq!(text.len(), 3);
    for body in &text {
        assert!(!body.to_string().contains("image_url"), "no image");
        for message in body["messages"].as_array().unwrap() {
            assert!(
                message["content"].is_string() || message["content"].is_null(),
                "{message}"
            );
        }
    }
    let result = tool_results(&text[2]).pop().unwrap();
    assert_eq!(result["note"], themis_core::prompts::TOOL_SCREENSHOT_NOTE);
    assert!(result.get("image_attached").is_none(), "{result}");
    assert!(screenshot_description(&text[2]).contains("You cannot see it yourself"));

    // Diagnostics: one image sent, its size, never its bytes.
    let log =
        std::fs::read_to_string(data.path().join("logs").join("diagnostics-0.jsonl")).unwrap();
    let sent: Vec<Value> = log
        .lines()
        .filter_map(|line| serde_json::from_str::<Value>(line).ok())
        .filter(|record| record["event"] == "images_sent")
        .collect();
    assert_eq!(sent.len(), 1, "{log}");
    let fields = sent[0].to_string();
    assert!(fields.contains("\"image_count\":1"), "{fields}");
    assert!(fields.contains("local/vision"), "{fields}");
    assert!(!log.contains("/9j/"), "no image bytes in the log");
    assert!(!log.contains("local-test-key"));

    let stop = std::process::Command::new(env!("CARGO_BIN_EXE_themis"))
        .arg("--data-dir")
        .arg(data.path())
        .args(["server", "stop"])
        .output()
        .unwrap();
    assert!(stop.status.success());
    let _ = tokio::time::timeout(Duration::from_secs(10), serving).await;
}
