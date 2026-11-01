//! Durable token usage through the shared server and the `themis` CLI: a scripted
//! provider answers chat requests with usage, usage rows land in `usage.sqlite3`
//! (numbers and ids only), and `get_usage_summary`, `get_chat_usage` and `themis usage`
//! report them.

use serde_json::{json, Value};
use std::path::Path;
use std::time::Duration;
use themis_desktop::{
    server::{Client, Server},
    state::AppState,
};
use wiremock::{matchers::method, Mock, MockServer, ResponseTemplate};

fn themis(dir: &Path, args: &[&str]) -> String {
    let output = std::process::Command::new(env!("CARGO_BIN_EXE_themis"))
        .arg("--data-dir")
        .arg(dir)
        .args(args)
        .env_remove("OPENCODE_KEY")
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8(output.stdout).unwrap()
}

fn cli(dir: &Path, command: &str, args: Value) -> Value {
    serde_json::from_str(&themis(dir, &["call", command, &args.to_string()])).unwrap()
}

#[tokio::test(flavor = "multi_thread")]
async fn a_completed_request_is_recorded_and_summarized_without_content() {
    let data = tempfile::tempdir().unwrap();
    let project = tempfile::tempdir().unwrap();
    let provider = MockServer::builder().start().await;
    Mock::given(method("POST"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "choices": [{"message": {"role": "assistant", "content": "PRIVATE-REPLY-4471"}, "finish_reason": "stop"}],
            "usage": {"prompt_tokens": 1200, "completion_tokens": 80, "total_tokens": 1280,
                      "prompt_tokens_details": {"cached_tokens": 1000},
                      "completion_tokens_details": {"reasoning_tokens": 30}}
        })))
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
        json!({"projectRoot": project.path(), "provider": "go", "model": "usage-model"}),
    );
    let id = thread["id"].as_str().unwrap().to_owned();
    cli(
        data.path(),
        "send_message",
        json!({"threadId": id, "text": "PRIVATE-PROMPT-4471", "reasoningEffort": null}),
    );
    let done = tokio::time::timeout(Duration::from_secs(120), async {
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

    let data_dir = data.path().to_owned();
    let summary = tokio::task::spawn_blocking(move || {
        let deadline = std::time::Instant::now() + Duration::from_secs(20);
        loop {
            let summary = cli(&data_dir, "get_usage_summary", json!({}));
            if summary["windows"]["today"]["requests"]
                .as_u64()
                .unwrap_or(0)
                >= 1
            {
                return summary;
            }
            assert!(std::time::Instant::now() < deadline, "{summary}");
            std::thread::sleep(Duration::from_millis(200));
        }
    })
    .await
    .unwrap();
    assert_eq!(summary["available"], true);
    assert_eq!(summary["scope"], "themis_only");
    let requests = summary["windows"]["five_hours"]["requests"]
        .as_u64()
        .unwrap();
    for window in ["five_hours", "today", "week", "month"] {
        let totals = &summary["windows"][window];
        assert_eq!(totals["requests"].as_u64().unwrap(), requests, "{window}");
        assert_eq!(totals["input_tokens"].as_u64().unwrap(), 1200 * requests);
        assert_eq!(
            totals["cached_input_tokens"].as_u64().unwrap(),
            1000 * requests
        );
        assert_eq!(totals["output_tokens"].as_u64().unwrap(), 80 * requests);
        assert_eq!(totals["reasoning_tokens"].as_u64().unwrap(), 30 * requests);
    }
    assert_eq!(summary["days"].as_array().unwrap().len(), 30);
    assert_eq!(summary["by_provider"][0]["key"], "opencode-go");
    assert_eq!(summary["by_model"][0]["key"], "usage-model");
    assert_eq!(summary["by_chat"][0]["key"], id.as_str());
    assert_eq!(summary["billing"][0]["kind"], "plan_windows");
    assert_eq!(summary["billing"][0]["windows"], json!([]));

    let chat = cli(data.path(), "get_chat_usage", json!({"threadId": id}));
    assert_eq!(chat["requests"].as_u64().unwrap(), requests);
    assert_eq!(chat["input_tokens"].as_u64().unwrap(), 1200 * requests);

    let text = themis(data.path(), &["usage"]);
    assert!(text.contains("Tokens from Themis only"), "{text}");
    assert!(text.contains("usage-model"), "{text}");
    let raw: Value = serde_json::from_str(&themis(data.path(), &["usage", "--json"])).unwrap();
    assert_eq!(raw["by_model"][0]["key"], "usage-model");

    // Numbers and ids only: no prompt, reply or key in the usage store.
    let mut stored = Vec::new();
    for name in ["usage.sqlite3", "usage.sqlite3-wal"] {
        stored.extend(std::fs::read(data.path().join(name)).unwrap_or_default());
    }
    let stored = String::from_utf8_lossy(&stored);
    assert!(stored.contains("usage-model"));
    for secret in ["PRIVATE-PROMPT-4471", "PRIVATE-REPLY-4471", "test-key"] {
        assert!(!stored.contains(secret), "{secret} stored");
    }
    serving.abort();
}
