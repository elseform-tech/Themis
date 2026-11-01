//! A plugin whose MCP configuration holds literal credentials, through the real shared server and
//! the `themis` CLI: the literal is discarded (nothing under the data folder or the project holds
//! it afterwards), the connection shows Secret needed with the names, and once the person adds
//! them the server receives the person's values. One test per file: servers read secrets through
//! a process-wide credential store.

use std::path::Path;
use std::time::Duration;

use serde_json::{json, Value};
use themis_desktop::{server::Server, state::AppState};
use wiremock::matchers::{header, method};
use wiremock::{Mock, MockServer, Request, ResponseTemplate};

/// Looks like the real thing, and appears nowhere it shouldn't.
const SHIPPED_KEY: &str = "sk-shipped-Zq8vK2mX9pL4vB7nR3tY";
const SHIPPED_SECRET: &str = "shipped-secret-value-77";

fn cli(data: &Path, args: &[&str]) -> Value {
    let output = std::process::Command::new(env!("CARGO_BIN_EXE_themis"))
        .arg("--data-dir")
        .arg(data)
        .args(args)
        .output()
        .expect("CLI");
    assert!(
        output.status.success(),
        "{args:?}: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(
        !String::from_utf8_lossy(&output.stdout).contains(SHIPPED_KEY),
        "{args:?}"
    );
    serde_json::from_slice(&output.stdout).unwrap_or(Value::Null)
}

fn call(data: &Path, command: &str, args: Value) -> Value {
    cli(data, &["call", command, &args.to_string()])
}

fn settled(dir: &Path, project: &Path, key: &str, retry: bool) -> Value {
    let deadline = std::time::Instant::now() + Duration::from_secs(20);
    let mut retry = retry;
    loop {
        let all = call(
            dir,
            "plugin_action",
            json!({"action": "mcp_status", "projectRoot": project, "connect": true, "retry": retry}),
        );
        retry = false;
        let status = all[key].clone();
        if status["status"] != "connecting" && !status.is_null() {
            return status;
        }
        assert!(std::time::Instant::now() < deadline, "{all}");
        std::thread::sleep(Duration::from_millis(300));
    }
}

/// Every file under `root`, whatever it is, must be free of both literals.
fn assert_literals_gone(root: &Path) {
    let mut stack = vec![root.to_path_buf()];
    let mut read = 0;
    while let Some(folder) = stack.pop() {
        for entry in std::fs::read_dir(&folder).unwrap().flatten() {
            let path = entry.path();
            if path.is_dir() {
                stack.push(path);
            } else if let Ok(bytes) = std::fs::read(&path) {
                read += 1;
                let text = String::from_utf8_lossy(&bytes);
                for literal in [SHIPPED_KEY, SHIPPED_SECRET] {
                    assert!(!text.contains(literal), "{}", path.display());
                }
            }
        }
    }
    assert!(read > 0, "nothing was scanned under {}", root.display());
}

#[tokio::test(flavor = "multi_thread")]
async fn cli_a_literal_is_discarded_the_state_shows_and_the_persons_value_reaches_the_server() {
    let mcp = MockServer::builder().start().await;
    Mock::given(method("POST"))
        .and(header("cld-api-key", "persons-key"))
        .and(header("cld-secret", "persons-secret"))
        .and(header("cld-cloud-name", "persons-cloud"))
        .respond_with(|request: &Request| {
            let body: Value = serde_json::from_slice(&request.body).unwrap();
            match body.get("id") {
                Some(id) if body["method"] == "tools/list" => ResponseTemplate::new(200)
                    .set_body_json(json!({"jsonrpc":"2.0","id":id,"result":{"tools":[]}})),
                Some(id) => ResponseTemplate::new(200).set_body_json(json!({"jsonrpc":"2.0","id":id,"result":{"protocolVersion":"2025-06-18","capabilities":{},"serverInfo":{"name":"m","version":"1"}}})),
                None => ResponseTemplate::new(202),
            }
        })
        .mount(&mcp)
        .await;
    // Anything else, the shipped values above all, is refused.
    Mock::given(method("POST"))
        .respond_with(ResponseTemplate::new(403))
        .mount(&mcp)
        .await;

    let data = tempfile::tempdir().unwrap();
    let project = data.path().join("project");
    std::fs::create_dir_all(&project).unwrap();
    let folders = tempfile::tempdir().unwrap();
    let plugin = folders.path().join("cloudy");
    std::fs::create_dir_all(plugin.join(".claude-plugin")).unwrap();
    std::fs::write(
        plugin.join(".claude-plugin/plugin.json"),
        json!({"name": "cloudy", "version": "1.0.0", "description": "x", "mcpServers": "./mcp.json"})
            .to_string(),
    )
    .unwrap();
    // As Cloudinary's headers, with values that might be real; the field's own name for the third.
    std::fs::write(
        plugin.join("mcp.json"),
        json!({"mcpServers": {"flows": {
            "type": "streamable-http",
            "url": format!("{}/mcp", mcp.uri()),
            "headers": {"cld-api-key": SHIPPED_KEY, "cld-secret": SHIPPED_SECRET, "cld-cloud-name": "cloud_name"}
        }}})
        .to_string(),
    )
    .unwrap();
    // Text elsewhere in the plugin that repeats a real value doesn't keep it either.
    std::fs::write(plugin.join("README.md"), format!("Use {SHIPPED_KEY}.")).unwrap();

    let state = AppState::new_for_test(data.path().join("settings.json"));
    let task = tokio::spawn(
        Server::bind(state.clone(), data.path())
            .await
            .unwrap()
            .run(),
    );
    let dir = data.path().to_owned();
    let folder = plugin.clone();
    tokio::task::spawn_blocking(move || {
        let imported = cli(&dir, &["plugin", "import", folder.to_str().unwrap()]);
        let flows = &imported["spec"]["mcp"]["flows"];
        assert_eq!(flows["headers"]["cld-api-key"], "${CLOUDY_CLD_API_KEY}");
        assert_eq!(flows["headers"]["cld-secret"], "${CLOUDY_CLD_SECRET}");
        assert_eq!(flows["headers"]["cld-cloud-name"], "${CLOUDY_CLD_CLOUD_NAME}");
        call(&dir, "plugin_action", json!({"action": "set_component_enabled", "scope": "global", "name": "cloudy", "kind": "mcp", "id": "flows", "enabled": true}));

        // Secret needed, every name at once, and listed in Settings › Secrets.
        let key = "global:cloudy:mcp:flows";
        let status = settled(&dir, &project, key, false);
        assert_eq!(status["status"], "needs_key", "{status}");
        assert_eq!(
            status["keys"],
            json!(["CLOUDY_CLD_API_KEY", "CLOUDY_CLD_CLOUD_NAME", "CLOUDY_CLD_SECRET"])
        );
        let keys = call(&dir, "list_keys", json!({"projectRoot": project}));
        assert_eq!(keys[0]["used_by"], json!(["cloudy/flows"]));
        assert_literals_gone(&dir);

        // The person's values reach the server (it answers nothing else).
        for (name, value) in [
            ("CLOUDY_CLD_API_KEY", "persons-key"),
            ("CLOUDY_CLD_SECRET", "persons-secret"),
            ("CLOUDY_CLD_CLOUD_NAME", "persons-cloud"),
        ] {
            call(&dir, "set_key", json!({"name": name, "value": value}));
        }
        // Saved, and still not sent until the person allows that name for that host.
        let status = settled(&dir, &project, key, true);
        assert_eq!(status["status"], "needs_secret_ok", "{status}");
        let allowed = call(&dir, "plugin_action", json!({
            "action": "mcp_allow_secrets", "projectRoot": project, "scope": "global", "name": "cloudy", "id": "flows",
            "secrets": ["CLOUDY_CLD_API_KEY", "CLOUDY_CLD_SECRET", "CLOUDY_CLD_CLOUD_NAME"],
        }));
        assert!(allowed.get(key).is_some(), "{allowed}");
        let status = settled(&dir, &project, key, false);
        assert_eq!(status["status"], "connected", "{status}");

        // Connecting put the plugin's files in the project, and the logs have run: still gone.
        assert_literals_gone(&dir);
        let files = project.join(".themis/plugin-files");
        assert!(files.is_dir(), "the plugin's files are in the project");
        assert_literals_gone(&files);
        let readme = walk_for(&files, "README.md");
        assert!(std::fs::read_to_string(readme).unwrap().contains("${CLOUDY_CLD_API_KEY}"));
    })
    .await
    .unwrap();
    // What the person's own folder holds is theirs: it isn't rewritten.
    assert!(std::fs::read_to_string(plugin.join("README.md"))
        .unwrap()
        .contains(SHIPPED_KEY));
    // Nothing shipped was ever sent.
    for request in mcp.received_requests().await.unwrap() {
        for (_, value) in request.headers.iter() {
            let value = value.to_str().unwrap_or_default();
            assert!(value != SHIPPED_KEY && value != SHIPPED_SECRET && value != "cloud_name");
        }
    }
    task.abort();
}

fn walk_for(root: &Path, name: &str) -> std::path::PathBuf {
    let mut stack = vec![root.to_path_buf()];
    while let Some(folder) = stack.pop() {
        for entry in std::fs::read_dir(&folder).unwrap().flatten() {
            let path = entry.path();
            if path.is_dir() {
                stack.push(path);
            } else if path.file_name().is_some_and(|file| file == name) {
                return path;
            }
        }
    }
    panic!("no {name} under {}", root.display());
}
