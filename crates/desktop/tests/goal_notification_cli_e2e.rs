//! Goal notifications through the real shared server and the `themis` CLI: the
//! app's goal watch picks the reached goal out of the live event stream, the
//! switch persists, and attempts are logged as metadata only.

use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

use serde_json::{json, Value};
use themis_desktop::notifications::{goal_change, GoalNotice, GoalWatch};
use themis_desktop::server::{Client, Server};
use themis_desktop::sink::{ChannelSink, EventSink};
use themis_desktop::state::AppState;
use themis_desktop::types::ProviderKind;
use wiremock::matchers::method;
use wiremock::{Mock, MockServer, Request, ResponseTemplate};

const CHECK_MARKER: &str = "You decide whether an agent has reached a goal";

fn call(data: &Path, command: &str, args: Value) -> Result<Value, String> {
    let output = std::process::Command::new(env!("CARGO_BIN_EXE_themis"))
        .arg("--data-dir")
        .arg(data)
        .args(["call", command, &args.to_string()])
        .env_remove("OPENCODE_KEY")
        .output()
        .expect("CLI");
    if !output.status.success() {
        return Err(String::from_utf8_lossy(&output.stderr).into_owned());
    }
    Ok(serde_json::from_slice(&output.stdout).unwrap_or(Value::Null))
}

fn reply(text: &str) -> ResponseTemplate {
    ResponseTemplate::new(200).set_body_json(json!({
        "choices": [{"message": {"role": "assistant", "content": text}, "finish_reason": "stop"}]
    }))
}

#[tokio::test(flavor = "multi_thread")]
async fn a_reached_goal_notifies_once_and_attempts_log_metadata_only() {
    let llm = MockServer::builder().start().await;
    Mock::given(method("POST"))
        .respond_with(|request: &Request| {
            let check = request.body_json::<Value>().ok().is_some_and(|body| {
                body["messages"][0]["content"]
                    .as_str()
                    .is_some_and(|text| text.contains(CHECK_MARKER))
            });
            if check {
                reply(r#"{"verdict":"reached","reason":"The README has a title.","progress":"title present"}"#)
            } else {
                reply("Added a title.")
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
    let mut stream = Client::new(data.path().to_owned())
        .subscribe()
        .await
        .unwrap();

    let data_dir = data.path().to_owned();
    let thread_id = thread.id.clone();
    tokio::task::spawn_blocking(move || {
        // The switch defaults on and persists when turned off.
        let settings = call(&data_dir, "get_settings", Value::Null).unwrap();
        assert_eq!(settings["goal_notifications"], true);
        let patch = json!({"patch": {"goal_notifications": false}});
        assert_eq!(
            call(&data_dir, "update_settings", patch).unwrap()["goal_notifications"],
            false
        );
        assert_eq!(
            call(&data_dir, "get_settings", Value::Null).unwrap()["goal_notifications"],
            false
        );
        call(
            &data_dir,
            "send_message",
            json!({"threadId": thread_id, "text": "/goal give the README a title"}),
        )
        .unwrap();
    })
    .await
    .unwrap();

    // The app's watch reads the live stream: one notice, when the goal is reached.
    let mut watch = GoalWatch::default();
    let mut notices = Vec::new();
    let deadline = tokio::time::Instant::now() + Duration::from_secs(60);
    while notices.is_empty() {
        let frame = tokio::time::timeout_at(deadline, Client::next_event(&mut stream))
            .await
            .expect("goal never reached")
            .unwrap();
        if let Some((id, goal)) = goal_change(&frame) {
            assert_eq!(id, thread.id);
            notices.extend(watch.observe(&id, goal.as_ref()));
        }
    }
    assert_eq!(notices, [GoalNotice::Reached]);

    let data_dir = data.path().to_owned();
    let thread_id = thread.id.clone();
    tokio::task::spawn_blocking(move || {
        let record =
            |record: Value| call(&data_dir, "record_notification", json!({"record": record}));
        record(json!({"thread_id": thread_id, "kind": "reached", "outcome": "sent"})).unwrap();
        record(json!({"thread_id": thread_id, "kind": "quota", "outcome": "denied"})).unwrap();
        // Anything beyond the metadata, or an id that is not a chat id, is refused.
        assert!(record(
            json!({"thread_id": thread_id, "kind": "reached", "outcome": "sent", "title": "README"})
        )
        .is_err());
        assert!(
            record(json!({"thread_id": "../x", "kind": "reached", "outcome": "sent"})).is_err()
        );
        let logs = call(
            &data_dir,
            "query_diagnostic_logs",
            json!({"service": "notifications"}),
        )
        .unwrap();
        let logs = logs.as_array().unwrap();
        assert_eq!(logs.len(), 2, "{logs:?}");
        assert_eq!(
            logs.iter()
                .map(|entry| (entry["level"].clone(), entry["details"].clone()))
                .collect::<Vec<_>>(),
            [
                (
                    json!("info"),
                    json!({"thread_id": thread_id, "kind": "reached", "outcome": "sent"})
                ),
                (
                    json!("warn"),
                    json!({"thread_id": thread_id, "kind": "quota", "outcome": "denied"})
                ),
            ]
        );
    })
    .await
    .unwrap();

    task.abort();
    let sink: Arc<dyn EventSink> = Arc::new(ChannelSink::channel().0);
    let _ = state.remove_goal(sink, thread.id.clone()).await;
}
