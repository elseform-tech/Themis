//! Settings written through the shared server (here by the `themis` CLI) are announced on
//! the subscription stream as `settings-changed`, so an open window reads them again instead
//! of showing stale values until a reload. Reads and refused writes announce nothing.

use std::time::Duration;

use serde_json::{json, Value};
use themis_desktop::server::{Client, Server};
use themis_desktop::state::AppState;

/// `themis call <method> -` against the data dir's server, as a person would; `None` on failure.
fn cli_call(data_dir: &std::path::Path, method: &str, args: Value) -> Option<Value> {
    use std::io::Write;
    let mut child = std::process::Command::new(env!("CARGO_BIN_EXE_themis"))
        .arg("--data-dir")
        .arg(data_dir)
        .env_remove("OPENCODE_KEY")
        .args(["call", method, "-"])
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .expect("run CLI");
    child
        .stdin
        .take()
        .unwrap()
        .write_all(args.to_string().as_bytes())
        .unwrap();
    let output = child.wait_with_output().unwrap();
    output
        .status
        .success()
        .then(|| serde_json::from_slice(&output.stdout).unwrap_or(Value::Null))
}

/// The scope of the next `settings-changed` event.
async fn next_settings_change(stream: &mut tokio::net::TcpStream) -> Value {
    loop {
        let event = tokio::time::timeout(Duration::from_secs(20), Client::next_event(stream))
            .await
            .expect("event within 20s")
            .expect("event frame");
        if event["name"] == "settings-changed" {
            assert_eq!(
                event["payload"].as_object().map(|payload| payload.len()),
                Some(1),
                "metadata only: {event}"
            );
            return event["payload"]["scope"].clone();
        }
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn settings_written_through_the_cli_reach_subscribed_windows() {
    let dir = tempfile::tempdir().unwrap();
    let state = AppState::new_for_test(dir.path().join("settings.json"));
    let server = Server::bind(state, dir.path()).await.unwrap();
    let task = tokio::spawn(server.run());
    let client = Client::new(dir.path().to_path_buf());
    let mut events = client.subscribe().await.unwrap();
    let data = dir.path();

    // The live-check case: approval mode, approval timeout and default model.
    let patch = json!({"patch": {
        "default_approval_mode": "custom",
        "approval_timeout_seconds": 30,
        "default_model": "glm-5"
    }});
    assert!(cli_call(data, "update_settings", patch).is_some());
    assert_eq!(next_settings_change(&mut events).await, "app");
    let settings = client.call("get_settings", json!({})).await.unwrap();
    assert_eq!(settings["default_approval_mode"], "custom");
    assert_eq!(settings["approval_timeout_seconds"], 30);
    assert_eq!(settings["default_model"], "glm-5");

    // A refused write and a read announce nothing: the next event is the next real write.
    let refused = json!({"patch": {"approval_timeout_seconds": 5}});
    assert!(cli_call(data, "update_settings", refused).is_none());
    assert!(cli_call(data, "get_settings", json!({})).is_some());
    let subagents = json!({"changes": {"allow_web": false}, "revision": null});
    assert!(cli_call(data, "update_subagent_settings", subagents).is_some());
    assert_eq!(next_settings_change(&mut events).await, "runtime");

    // Every Settings section that writes runtime.jsonc, and the whole-file save.
    let writes = [
        (
            "update_web_settings",
            json!({"searchUrl": "http://127.0.0.1:9"}),
        ),
        (
            "update_memory_settings",
            json!({"enabled": false, "revision": null}),
        ),
        (
            "update_browser_settings",
            json!({"changes": {"screenshots_to_model": false}, "revision": null}),
        ),
        (
            "save_runtime_configuration",
            json!({"scope": "user", "projectRoot": null, "json": "{}"}),
        ),
    ];
    for (method, args) in writes {
        assert!(cli_call(data, method, args).is_some(), "{method}");
        assert_eq!(
            next_settings_change(&mut events).await,
            "runtime",
            "{method}"
        );
    }

    // Stored keys: the event names the scope, never the key or its value.
    let key = json!({"name": "LIVE_SYNC_KEY", "value": "not-a-real-key"});
    assert!(cli_call(data, "set_key", key).is_some());
    assert_eq!(next_settings_change(&mut events).await, "secrets");
    assert!(cli_call(data, "remove_key", json!({"name": "LIVE_SYNC_KEY"})).is_some());
    assert_eq!(next_settings_change(&mut events).await, "secrets");
    task.abort();
}
