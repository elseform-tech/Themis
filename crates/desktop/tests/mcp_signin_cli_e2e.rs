//! Signing in to a catalog plugin's server through the real shared server and the `themis` CLI,
//! with plugin folders written as the real ones are (their `oauth` blocks as shipped, only the
//! address pointed at a fake provider): import, Sign in needed, the sign-in page opening with the
//! plugin's client, a token in the credential store, and an MCP call. One test per file: servers
//! read tokens through a process-wide credential store. No real account is involved.

use std::collections::HashMap;
use std::path::Path;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use serde_json::{json, Value};
use themis_desktop::{server::Server, state::AppState};
use wiremock::matchers::{body_string_contains, header, method, path};
use wiremock::{Mock, MockServer, Request, ResponseTemplate};

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
    serde_json::from_slice(&output.stdout).unwrap_or(Value::Null)
}

fn call(data: &Path, command: &str, args: Value) -> Value {
    cli(data, &["call", command, &args.to_string()])
}

/// A connection's status once it settles.
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

fn plugin_folder(root: &Path, name: &str, servers: &Value) -> std::path::PathBuf {
    let folder = root.join(name);
    std::fs::create_dir_all(folder.join(".claude-plugin")).unwrap();
    std::fs::write(
        folder.join(".claude-plugin/plugin.json"),
        json!({"name": name, "version": "1.0.0", "description": "x"}).to_string(),
    )
    .unwrap();
    std::fs::write(
        folder.join(".mcp.json"),
        json!({"mcpServers": servers}).to_string(),
    )
    .unwrap();
    folder
}

/// The parameters of an address or a form, decoded.
fn parameters(text: &str) -> HashMap<String, String> {
    let decode = |text: &str| {
        let bytes = text.as_bytes();
        let mut out = Vec::new();
        let mut index = 0;
        while index < bytes.len() {
            match bytes[index] {
                b'%' => {
                    let hex = std::str::from_utf8(&bytes[index + 1..index + 3]).unwrap();
                    out.push(u8::from_str_radix(hex, 16).unwrap());
                    index += 3;
                }
                b'+' => {
                    out.push(b' ');
                    index += 1;
                }
                byte => {
                    out.push(byte);
                    index += 1;
                }
            }
        }
        String::from_utf8(out).unwrap()
    };
    text.rsplit('?')
        .next()
        .unwrap_or_default()
        .split('&')
        .filter_map(|pair| pair.split_once('='))
        .map(|(name, value)| (decode(name), decode(value)))
        .collect()
}

/// "The person's browser": goes straight back to the callback from the sign-in page.
fn browser_comes_back(query: &HashMap<String, String>) {
    use std::io::{Read, Write};
    let (port, callback) = query["redirect_uri"]
        .strip_prefix("http://localhost:")
        .and_then(|rest| rest.split_once('/'))
        .unwrap();
    let port: u16 = port.parse().unwrap();
    let target = format!("/{callback}?code=code-1&state={}", query["state"]);
    std::thread::spawn(move || {
        let mut stream = std::net::TcpStream::connect(("127.0.0.1", port)).unwrap();
        write!(stream, "GET {target} HTTP/1.1\r\nHost: x\r\n\r\n").unwrap();
        let mut answer = String::new();
        stream.read_to_string(&mut answer).unwrap();
        assert!(answer.contains("Signed in"), "{answer}");
    });
}

#[tokio::test(flavor = "multi_thread")]
async fn cli_a_plugins_oauth_block_carries_through_import_and_sign_in_to_an_mcp_call() {
    let provider = MockServer::builder().start().await;
    let base = provider.uri();
    // MCP: needs a token.
    Mock::given(method("POST"))
        .and(path("/mcp"))
        .and(header("authorization", "Bearer at-1"))
        .respond_with(|request: &Request| {
            let body: Value = serde_json::from_slice(&request.body).unwrap();
            let result = if body["method"] == "tools/list" {
                json!({"tools":[{"name":"echo","description":"","inputSchema":{"type":"object"}}]})
            } else {
                json!({"protocolVersion":"2025-06-18","capabilities":{},"serverInfo":{"name":"fake","version":"1"}})
            };
            match body.get("id") {
                Some(id) => ResponseTemplate::new(200)
                    .set_body_json(json!({"jsonrpc":"2.0","id":id,"result":result})),
                None => ResponseTemplate::new(202),
            }
        })
        .mount(&provider)
        .await;
    Mock::given(method("POST"))
        .and(path("/mcp"))
        .respond_with(
            ResponseTemplate::new(401).insert_header(
                "WWW-Authenticate",
                format!(
                    r#"Bearer resource_metadata="{base}/.well-known/oauth-protected-resource""#
                )
                .as_str(),
            ),
        )
        .mount(&provider)
        .await;
    Mock::given(method("GET"))
        .and(path("/.well-known/oauth-protected-resource"))
        .respond_with(ResponseTemplate::new(200).set_body_json(
            json!({"resource": base, "authorization_servers": [base], "scopes_supported": ["read"]}),
        ))
        .mount(&provider)
        .await;
    Mock::given(method("GET"))
        .and(path("/.well-known/oauth-authorization-server"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "issuer": base,
            "authorization_endpoint": format!("{base}/authorize"),
            "token_endpoint": format!("{base}/token"),
            "code_challenge_methods_supported": ["S256"]
        })))
        .mount(&provider)
        .await;
    // The code exchange holds the sign-in to what the page was given.
    let seen: Arc<Mutex<HashMap<String, String>>> = Arc::default();
    let (exchange, expected) = (Arc::clone(&seen), base.clone());
    Mock::given(method("POST"))
        .and(path("/token"))
        .and(body_string_contains("grant_type=authorization_code"))
        .respond_with(move |request: &Request| {
            let form = parameters(&String::from_utf8_lossy(&request.body));
            let page = exchange.lock().unwrap();
            // The verifier's hash is checked in `crates/core/tests/mcp_oauth_presets.rs`.
            let good = !form["code_verifier"].is_empty()
                && form["client_id"] == page["client_id"]
                && form["redirect_uri"] == page["redirect_uri"]
                && form["resource"] == expected;
            if !good {
                return ResponseTemplate::new(400).set_body_json(json!({"error":"invalid_grant"}));
            }
            ResponseTemplate::new(200).set_body_json(
                json!({"access_token":"at-1","refresh_token":"rt","expires_in":3600}),
            )
        })
        .mount(&provider)
        .await;

    let data = tempfile::tempdir().unwrap();
    let folders = tempfile::tempdir().unwrap();
    let project = data.path().join("project");
    std::fs::create_dir_all(&project).unwrap();
    let port = std::net::TcpListener::bind("127.0.0.1:0")
        .unwrap()
        .local_addr()
        .unwrap()
        .port();
    // As Slack ships it: a client someone registered, and its callback port.
    let plugin = plugin_folder(
        folders.path(),
        "slack",
        &json!({"slack": {"type": "http", "url": format!("{base}/mcp"), "oauth": {"clientId": "1601.8899", "callbackPort": port}}}),
    );
    let state = AppState::new_for_test(data.path().join("settings.json"));
    let opened = Arc::clone(&seen);
    state.use_url_opener(Arc::new(move |page: &str| {
        let query = parameters(page);
        browser_comes_back(&query);
        *opened.lock().unwrap() = query;
        Ok(())
    }));
    let task = tokio::spawn(
        Server::bind(state.clone(), data.path())
            .await
            .unwrap()
            .run(),
    );
    let dir = data.path().to_owned();
    let url = format!("{base}/mcp");
    tokio::task::spawn_blocking(move || {
        // Installed in full, and off until the person turns it on.
        let imported = cli(&dir, &["plugin", "import", plugin.to_str().unwrap()]);
        assert_eq!(imported["spec"]["import_issues"], json!(null), "{imported}");
        assert_eq!(imported["spec"]["mcp"]["slack"]["oauth"], json!({"client_id": "1601.8899", "callback_port": port}));
        let shown = cli(&dir, &["plugin", "show", "slack"]);
        assert_eq!(shown["spec"]["mcp"]["slack"]["enabled"], false);
        call(&dir, "plugin_action", json!({"action": "set_component_enabled", "scope": "global", "name": "slack", "kind": "mcp", "id": "slack", "enabled": true}));

        let key = "global:slack:mcp:slack";
        let status = settled(&dir, &project, key, false);
        assert_eq!(status["status"], "needs_sign_in", "{status}");
        assert_eq!(status["signed_in"], false);
        assert_eq!(status["url"], url);

        // Sign in: the page gets the plugin's client and its exact callback address.
        let signed = call(&dir, "plugin_action", json!({"action": "mcp_sign_in", "projectRoot": project, "url": url}));
        assert_eq!(signed[key]["signed_in"], true, "{signed}");
        {
            let page = seen.lock().unwrap();
            assert_eq!(page["client_id"], "1601.8899");
            assert_eq!(page["redirect_uri"], format!("http://localhost:{port}/callback"));
            assert_eq!(page["code_challenge_method"], "S256");
            assert_eq!(page["scope"], "read");
        }
        let status = settled(&dir, &project, key, true);
        assert_eq!(status["status"], "connected", "{status}");
        let tools = call(&dir, "plugin_action", json!({"action": "mcp_tools", "scope": "global", "name": "slack", "id": "slack", "projectRoot": project}));
        assert!(tools.to_string().contains("echo"), "{tools}");

        // The token is nowhere under the data folder: it is in the credential store.
        let mut stack = vec![dir.clone()];
        while let Some(folder) = stack.pop() {
            for entry in std::fs::read_dir(&folder).unwrap().flatten() {
                let path = entry.path();
                if path.is_dir() {
                    stack.push(path);
                } else if let Ok(text) = std::fs::read_to_string(&path) {
                    assert!(!text.contains("at-1"), "{}", path.display());
                }
            }
        }

        // Sign out: asked again.
        let out = call(&dir, "plugin_action", json!({"action": "mcp_sign_out", "projectRoot": project, "url": url}));
        assert_eq!(out[key]["signed_in"], false, "{out}");
    })
    .await
    .unwrap();
    task.abort();
}
