//! Settings › Keys through the real shared server and the `themis` CLI: a remote MCP server
//! whose header reads `${NOTES_KEY}` reports the key missing, connects once the key is saved
//! (sent with its prefix in that header), and reports it missing again once removed. Key
//! values never come back from the server. One test per file: servers read keys through a
//! process-wide credential store.

use std::path::Path;
use std::time::Duration;

use serde_json::{json, Value};
use themis_desktop::{server::Server, state::AppState};
use wiremock::matchers::{header, method};
use wiremock::{Mock, MockServer, Request, ResponseTemplate};

fn call(data: &Path, command: &str, args: Value) -> Value {
    let output = std::process::Command::new(env!("CARGO_BIN_EXE_themis"))
        .arg("--data-dir")
        .arg(data)
        .args(["call", command, &args.to_string()])
        .env_remove("NOTES_KEY")
        .output()
        .expect("CLI");
    assert!(
        output.status.success(),
        "{command}: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    serde_json::from_slice(&output.stdout).unwrap_or(Value::Null)
}

/// Polls the server's status until it settles (anything but connecting).
fn settled(dir: &Path, project: &Path, retry: bool) -> Value {
    let deadline = std::time::Instant::now() + Duration::from_secs(20);
    let mut retry = retry;
    loop {
        let all = call(
            dir,
            "plugin_action",
            json!({"action": "mcp_status", "projectRoot": project, "connect": true, "retry": retry}),
        );
        retry = false;
        let status = all["global:remote:mcp:notes"].clone();
        if status["status"] != "connecting" && !status.is_null() {
            return status;
        }
        assert!(std::time::Instant::now() < deadline, "{all}");
        std::thread::sleep(Duration::from_millis(300));
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn cli_a_server_key_is_saved_used_and_removed() {
    let mcp = MockServer::start().await;
    Mock::given(method("POST"))
        .and(header("x-api-key", "Token secret-123"))
        .respond_with(|request: &Request| {
            let body: Value = serde_json::from_slice(&request.body).unwrap();
            match body.get("id") {
                Some(id) if body["method"] == "tools/list" => ResponseTemplate::new(200).set_body_json(json!({"jsonrpc":"2.0","id":id,"result":{"tools":[]}})),
                Some(id) => ResponseTemplate::new(200).set_body_json(json!({"jsonrpc":"2.0","id":id,"result":{"protocolVersion":"2025-06-18","capabilities":{},"serverInfo":{"name":"notes","version":"1"}}})),
                None => ResponseTemplate::new(202),
            }
        })
        .mount(&mcp)
        .await;
    Mock::given(method("POST"))
        .respond_with(ResponseTemplate::new(403))
        .mount(&mcp)
        .await;
    let data = tempfile::tempdir().unwrap();
    let project = data.path().join("project");
    std::fs::create_dir_all(&project).unwrap();
    let state = AppState::new_for_test(data.path().join("settings.json"));
    let server = Server::bind(state.clone(), data.path()).await.unwrap();
    let task = tokio::spawn(server.run());
    let dir = data.path().to_owned();
    let url = format!("{}/mcp", mcp.uri());
    tokio::task::spawn_blocking(move || {
        call(&dir, "plugin_action", json!({"action": "save", "scope": "global", "spec": {"name": "remote", "mcp": {"notes": {"url": url, "headers": {"X-Api-Key": "Token ${NOTES_KEY}"}}}}}));
        call(&dir, "plugin_action", json!({"action": "set_component_enabled", "scope": "global", "name": "remote", "kind": "mcp", "id": "notes", "enabled": true}));
        // Missing: the status names the key, and Settings › Keys lists it with its user.
        let status = settled(&dir, &project, false);
        assert_eq!(status["status"], "needs_key", "{status}");
        assert_eq!(status["keys"], json!(["NOTES_KEY"]));
        assert!(status.get("url").is_none(), "a server with its own key does not sign in");
        let keys = call(&dir, "list_keys", json!({"projectRoot": project}));
        assert_eq!(keys[0], json!({"name": "NOTES_KEY", "saved": false, "environment": false, "used_by": ["remote/notes"]}));
        // The OpenCode Go key is always listed (its own test covers it).
        assert_eq!(keys[1]["name"], "OPENCODE_KEY");
        // Saved: listed as saved (never its value), and the server connects with it.
        call(&dir, "set_key", json!({"name": "NOTES_KEY", "value": "secret-123"}));
        let keys = call(&dir, "list_keys", json!({}));
        assert_eq!(keys[0]["saved"], true);
        assert!(!keys.to_string().contains("secret-123"));
        assert!(!std::fs::read_to_string(dir.join("keys.json")).unwrap().contains("secret-123"));
        let status = settled(&dir, &project, true);
        assert_eq!(status["status"], "connected", "{status}");
        // Bad names and values are refused.
        for args in [json!({"name": "a-b", "value": "x"}), json!({"name": "OK", "value": "has space"})] {
            let refused = std::process::Command::new(env!("CARGO_BIN_EXE_themis"))
                .arg("--data-dir")
                .arg(&dir)
                .args(["call", "set_key", &args.to_string()])
                .output()
                .unwrap();
            assert!(!refused.status.success(), "{args}");
        }
        // Removed: missing again on the next connection.
        call(&dir, "remove_key", json!({"name": "NOTES_KEY"}));
        call(&dir, "plugin_action", json!({"action": "set_component_enabled", "scope": "global", "name": "remote", "kind": "mcp", "id": "notes", "enabled": false}));
        call(&dir, "plugin_action", json!({"action": "set_component_enabled", "scope": "global", "name": "remote", "kind": "mcp", "id": "notes", "enabled": true}));
        assert_eq!(settled(&dir, &project, true)["status"], "needs_key");
        assert_eq!(call(&dir, "list_keys", json!({}))[0]["saved"], false);
    })
    .await
    .unwrap();
    task.abort();
}
