//! The OpenCode Go key is the Settings › Keys key `OPENCODE_KEY`, through the real shared
//! server and the `themis` CLI: a key saved before keys joined Settings › Keys is copied there
//! on first read, Settings › Keys lists it as used by OpenCode Go and by an MCP server whose
//! header reads `${OPENCODE_KEY}`, and that server receives it. One test per file: servers
//! read keys through a process-wide credential store.

use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

use serde_json::{json, Value};
use themis_desktop::secrets::{MemoryStore, SecretStore, GO_ENTRY, LEGACY_GO_ENTRY};
use themis_desktop::{server::Server, state::AppState};
use wiremock::matchers::{header, method};
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

#[tokio::test(flavor = "multi_thread")]
async fn cli_the_provider_key_is_a_settings_key_that_mcp_servers_can_use() {
    let mcp = MockServer::start().await;
    Mock::given(method("POST"))
        .and(header("authorization", "Bearer go-test-key"))
        .respond_with(|request: &Request| {
            let body: Value = serde_json::from_slice(&request.body).unwrap();
            match body.get("id") {
                Some(id) if body["method"] == "tools/list" => ResponseTemplate::new(200).set_body_json(json!({"jsonrpc":"2.0","id":id,"result":{"tools":[]}})),
                Some(id) => ResponseTemplate::new(200).set_body_json(json!({"jsonrpc":"2.0","id":id,"result":{"protocolVersion":"2025-06-18","capabilities":{},"serverInfo":{"name":"gateway","version":"1"}}})),
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
    // A key saved by an earlier version, under its old entry.
    let store = Arc::new(MemoryStore::new());
    store.set(LEGACY_GO_ENTRY, "go-test-key").unwrap();
    let state = AppState::new_for_test_with_secret_store(
        data.path().join("settings.json"),
        Arc::clone(&store) as Arc<dyn SecretStore>,
    );
    let server = Server::bind(state.clone(), data.path()).await.unwrap();
    let task = tokio::spawn(server.run());
    let dir = data.path().to_owned();
    let url = format!("{}/mcp", mcp.uri());
    let moved = Arc::clone(&store);
    tokio::task::spawn_blocking(move || {
        // First read: still available, now kept as the key OPENCODE_KEY. The old entry is
        // left alone (no Keychain prompt to delete it) and never read again.
        assert_eq!(call(&dir, "get_secret_status", json!({}))["go"], true);
        assert_eq!(moved.get(GO_ENTRY).as_deref(), Some("go-test-key"));

        call(&dir, "plugin_action", json!({"action": "save", "scope": "global", "spec": {"name": "remote", "mcp": {"gateway": {"url": url, "headers": {"Authorization": "Bearer ${OPENCODE_KEY}"}}}}}));
        call(&dir, "plugin_action", json!({"action": "set_component_enabled", "scope": "global", "name": "remote", "kind": "mcp", "id": "gateway", "enabled": true}));
        let keys = call(&dir, "list_keys", json!({"projectRoot": project}));
        assert_eq!(keys, json!([{"name": "OPENCODE_KEY", "saved": true, "environment": false, "used_by": ["OpenCode Go", "remote/gateway"]}]));
        assert!(!keys.to_string().contains("go-test-key"));

        // The MCP server receives the provider key in its header.
        let deadline = std::time::Instant::now() + Duration::from_secs(20);
        let mut retry = true;
        loop {
            let all = call(&dir, "plugin_action", json!({"action": "mcp_status", "projectRoot": project, "connect": true, "retry": retry}));
            retry = false;
            let status = &all["global:remote:mcp:gateway"];
            if status["status"] == "connected" {
                break;
            }
            assert!(status["status"] == "connecting" || status.is_null(), "{all}");
            assert!(std::time::Instant::now() < deadline, "{all}");
            std::thread::sleep(Duration::from_millis(300));
        }

        // Another secret can be chosen in Settings › Models; the choice is an app setting,
        // kept in settings.json (so it holds at every launch), and bad names are refused.
        call(&dir, "update_settings", json!({"patch": {"go_secret": "WORK_GO_KEY"}}));
        assert_eq!(call(&dir, "get_secret_status", json!({}))["go"], false);
        call(&dir, "set_key", json!({"name": "WORK_GO_KEY", "value": "go-work-key"}));
        assert_eq!(call(&dir, "get_secret_status", json!({}))["go"], true);
        let keys = call(&dir, "list_keys", json!({}));
        let work = keys.as_array().unwrap().iter().find(|k| k["name"] == "WORK_GO_KEY").unwrap();
        assert_eq!(work["used_by"], json!(["OpenCode Go"]));
        let saved: Value = serde_json::from_str(&std::fs::read_to_string(dir.join("settings.json")).unwrap()).unwrap();
        assert_eq!(saved["go_secret"], "WORK_GO_KEY");
        let refused = std::process::Command::new(env!("CARGO_BIN_EXE_themis"))
            .arg("--data-dir")
            .arg(&dir)
            .args(["call", "update_settings", &json!({"patch": {"go_secret": "bad name"}}).to_string()])
            .output()
            .unwrap();
        assert!(!refused.status.success());
        call(&dir, "update_settings", json!({"patch": {"go_secret": "OPENCODE_KEY"}}));

        // Removing the secret in Settings › Secrets removes the provider key too, old copy
        // included, so it cannot come back.
        call(&dir, "remove_key", json!({"name": "OPENCODE_KEY"}));
        assert_eq!(call(&dir, "get_secret_status", json!({}))["go"], false);
        assert_eq!(moved.get(LEGACY_GO_ENTRY), None);
    })
    .await
    .unwrap();
    task.abort();
}
