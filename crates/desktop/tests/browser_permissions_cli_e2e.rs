//! Settings › Browser permissions through the real shared server and the `themis` CLI: a
//! Browsing-Blocked site is refused even in YOLO, an Ask site is allowed by YOLO without a
//! prompt, and in Custom "Always allow on this site" answers the prompt and is saved in the
//! user `runtime.jsonc` with the rest of the file kept. Skipped without a Chromium.

use std::path::Path;
use std::time::Duration;

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

fn tool_call(id: &str, name: &str, arguments: Value) -> ResponseTemplate {
    ResponseTemplate::new(200).set_body_json(json!({
        "choices": [{"message": {"role": "assistant", "content": null, "tool_calls": [{
            "id": id, "type": "function",
            "function": {"name": name, "arguments": arguments.to_string()}
        }]}}]
    }))
}

fn tool_results(body: &Value) -> Vec<String> {
    body["messages"]
        .as_array()
        .into_iter()
        .flatten()
        .filter(|message| message["role"] == "tool")
        .map(|message| message["content"].to_string())
        .collect()
}

/// Waits until the model has seen `count` tool results and the run ended; returns them.
async fn results_after(
    llm: &MockServer,
    state: &AppState,
    thread: &str,
    count: usize,
) -> Vec<String> {
    let deadline = std::time::Instant::now() + Duration::from_secs(40);
    loop {
        let requests = llm.received_requests().await.unwrap_or_default();
        let latest = requests
            .iter()
            .map(|request| tool_results(&request.body_json().unwrap_or(Value::Null)))
            .rfind(|results| results.len() >= count);
        if let Some(results) = latest {
            if !state.get_thread(thread).await.is_ok_and(|t| t.running) {
                return results;
            }
        }
        assert!(
            std::time::Instant::now() < deadline,
            "run did not finish; pending approvals: {:?}",
            state.pending_approvals()
        );
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn cli_browser_permissions_block_in_every_mode_and_always_allow_is_saved() {
    if themis_core::browser::launch::find_browser(None, None).is_none() {
        eprintln!("skipping: no Chromium browser installed");
        return;
    }
    // `.invalid` never resolves, so no test reaches the network.
    let llm = MockServer::start().await;
    Mock::given(method("POST"))
        .respond_with(|request: &Request| {
            let body: Value = request.body_json().unwrap_or(Value::Null);
            let task = body.to_string();
            let results = tool_results(&body);
            let first = if task.contains("Open the pay site") {
                "https://www.paypal.com/"
            } else {
                "http://shop.example.invalid/"
            };
            match results.len() {
                0 => tool_call("c1", "browser_open", json!({"url": first})),
                1 if task.contains("Open the pay site") => {
                    tool_call("c2", "browser_open", json!({"url": "http://shop.example.invalid/"}))
                }
                _ => ResponseTemplate::new(200).set_body_json(json!({
                    "choices": [{"message": {"role": "assistant", "content": "Done."}, "finish_reason": "stop"}]
                })),
            }
        })
        .mount(&llm)
        .await;

    let data = tempfile::tempdir().unwrap();
    let project = tempfile::tempdir().unwrap();
    // Outside a Git repository the app refuses changes without asking.
    std::process::Command::new("git")
        .args(["init", "-q"])
        .current_dir(project.path())
        .status()
        .unwrap();
    let file = "{\n  // kept as written\n  \"schema_version\": 1,\n  \"browser\": {\"site_exceptions\": [\n    {\"site\": \"paypal.com\", \"browsing\": \"block\"},\n    {\"site\": \"example.invalid\", \"browsing\": \"ask\"}\n  ]}\n}\n";
    std::fs::write(data.path().join("runtime.jsonc"), file).unwrap();
    let state = AppState::new_for_test(data.path().join("settings.json"));
    state
        .set_secret("go".into(), "synthetic-test-key".into())
        .await
        .unwrap();
    state.set_go_base_url_override(Some(llm.uri()));
    let server = Server::bind(state.clone(), data.path()).await.unwrap();
    let task = tokio::spawn(server.run());
    let root = project.path().to_string_lossy().to_string();

    // YOLO: the blocked site is still refused; the Ask site runs without a prompt.
    let yolo = state
        .create_thread(root.clone(), ProviderKind::Go, Some("test-model".into()))
        .await
        .unwrap();
    {
        let (data, id) = (data.path().to_owned(), yolo.id.clone());
        tokio::task::spawn_blocking(move || {
            call(
                &data,
                "set_thread_approval_mode",
                json!({"threadId": id, "mode": "yolo"}),
            );
            call(
                &data,
                "send_message",
                json!({"threadId": id, "text": "Open the pay site, then the shop."}),
            );
        })
        .await
        .unwrap();
    }
    let results = results_after(&llm, &state, &yolo.id, 2).await;
    assert!(
        results[0].contains("Browsing is blocked for www.paypal.com in Settings › Browser"),
        "{}",
        results[0]
    );
    assert!(
        !results[1].contains("blocked in Settings") && !results[1].contains("approval denied"),
        "{}",
        results[1]
    );

    // Custom: the Ask site prompts a connected window; "Always allow on this site" saves an
    // exception.
    let window = themis_desktop::server::Client::new(data.path().into());
    let _events = window.subscribe().await.unwrap();
    let custom = state
        .create_thread(root, ProviderKind::Go, Some("test-model".into()))
        .await
        .unwrap();
    {
        let (data, id) = (data.path().to_owned(), custom.id.clone());
        tokio::task::spawn_blocking(move || {
            call(
                &data,
                "send_message",
                json!({"threadId": id, "text": "Open the shop."}),
            );
        })
        .await
        .unwrap();
    }
    let deadline = std::time::Instant::now() + Duration::from_secs(30);
    let approval = loop {
        if let Some(request) = state
            .pending_approvals()
            .into_iter()
            .find(|r| r.thread_id == custom.id)
        {
            break request;
        }
        if std::time::Instant::now() > deadline {
            let requests = llm.received_requests().await.unwrap_or_default();
            let seen: Vec<_> = requests
                .iter()
                .map(|r| tool_results(&r.body_json().unwrap_or(Value::Null)))
                .collect();
            let thread = state.get_thread(&custom.id).await;
            panic!("no approval prompt for the Ask site; requests: {seen:?}; thread: {thread:?}");
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    };
    assert_eq!(
        approval.grant_site.as_ref().map(|site| site.tools.as_str()),
        Some("browser")
    );
    {
        let (data, id, approval_id) = (
            data.path().to_owned(),
            custom.id.clone(),
            approval.approval_id.clone(),
        );
        tokio::task::spawn_blocking(move || {
            call(
                &data,
                "approve_action",
                json!({"threadId": id, "approvalId": approval_id, "decision": "chat"}),
            );
        })
        .await
        .unwrap();
    }
    let results = results_after(&llm, &state, &custom.id, 1).await;
    assert!(!results[0].contains("approval denied"), "{}", results[0]);
    let saved = std::fs::read_to_string(data.path().join("runtime.jsonc")).unwrap();
    assert!(saved.contains("// kept as written"), "{saved}");
    let browser = themis_core::configuration::resolve(Some(&saved), None, None)
        .unwrap()
        .config
        .browser;
    let shop = browser
        .site_exceptions
        .iter()
        .find(|exception| exception.site == "shop.example.invalid")
        .expect("the site exception was saved");
    assert_eq!(
        shop.browsing,
        Some(themis_core::configuration::SiteAccess::Allow)
    );
    assert!(browser
        .site_exceptions
        .iter()
        .any(|exception| exception.site == "paypal.com"));
    task.abort();
}

#[tokio::test(flavor = "multi_thread")]
async fn cli_debugging_tools_run_on_a_local_app_and_a_project_can_turn_them_off() {
    if themis_core::browser::launch::find_browser(None, None).is_none() {
        eprintln!("skipping: no Chromium browser installed");
        return;
    }
    let site = MockServer::start().await;
    Mock::given(wiremock::matchers::path("/app"))
        .respond_with(ResponseTemplate::new(200).set_body_raw(
            "<!doctype html><title>Local app</title><script>console.warn('slow start')</script><h1>Hi</h1>",
            "text/html",
        ))
        .mount(&site)
        .await;
    let page = format!("http://127.0.0.1:{}/app", site.address().port());
    let llm = MockServer::start().await;
    Mock::given(method("POST"))
        .respond_with(move |request: &Request| {
            let body: Value = request.body_json().unwrap_or(Value::Null);
            match tool_results(&body).len() {
                0 => tool_call("c1", "browser_open", json!({"url": page})),
                1 if body["tools"].to_string().contains("browser_devtools") => tool_call(
                    "c2",
                    "browser_devtools",
                    json!({"panel": "console", "expression": "document.title + '!'"}),
                ),
                2 => tool_call("c3", "browser_devtools", json!({"panel": "console"})),
                _ => ResponseTemplate::new(200).set_body_json(json!({
                    "choices": [{"message": {"role": "assistant", "content": "Done."}, "finish_reason": "stop"}]
                })),
            }
        })
        .mount(&llm)
        .await;
    let data = tempfile::tempdir().unwrap();
    let project = tempfile::tempdir().unwrap();
    std::process::Command::new("git")
        .args(["init", "-q"])
        .current_dir(project.path())
        .status()
        .unwrap();
    let state = AppState::new_for_test(data.path().join("settings.json"));
    state
        .set_secret("go".into(), "synthetic-test-key".into())
        .await
        .unwrap();
    state.set_go_base_url_override(Some(llm.uri()));
    let server = Server::bind(state.clone(), data.path()).await.unwrap();
    let task = tokio::spawn(server.run());
    let thread = state
        .create_thread(
            project.path().to_string_lossy().to_string(),
            ProviderKind::Go,
            Some("test-model".into()),
        )
        .await
        .unwrap();
    {
        let (data, id) = (data.path().to_owned(), thread.id.clone());
        tokio::task::spawn_blocking(move || {
            call(
                &data,
                "set_thread_approval_mode",
                json!({"threadId": id, "mode": "yolo"}),
            );
            call(
                &data,
                "send_message",
                json!({"threadId": id, "text": "Debug the app."}),
            );
        })
        .await
        .unwrap();
    }
    let results = results_after(&llm, &state, &thread.id, 3).await;
    assert!(results[1].contains("Local app!"), "{}", results[1]);
    assert!(results[2].contains("slow start"), "{}", results[2]);

    // A project that turns Developer mode off: no debugging tool is offered.
    std::fs::create_dir_all(project.path().join(".themis")).unwrap();
    std::fs::write(
        project.path().join(".themis/config.jsonc"),
        r#"{"browser":{"developer_mode":false}}"#,
    )
    .unwrap();
    let before = llm.received_requests().await.unwrap_or_default().len();
    let quiet = state
        .create_thread(
            project.path().to_string_lossy().to_string(),
            ProviderKind::Go,
            Some("test-model".into()),
        )
        .await
        .unwrap();
    {
        let (data, id) = (data.path().to_owned(), quiet.id.clone());
        tokio::task::spawn_blocking(move || {
            call(
                &data,
                "set_thread_approval_mode",
                json!({"threadId": id, "mode": "yolo"}),
            );
            call(
                &data,
                "send_message",
                json!({"threadId": id, "text": "Open it."}),
            );
        })
        .await
        .unwrap();
    }
    let deadline = std::time::Instant::now() + Duration::from_secs(30);
    loop {
        let requests = llm.received_requests().await.unwrap_or_default();
        if requests.len() > before && !state.get_thread(&quiet.id).await.is_ok_and(|t| t.running) {
            let offered = requests[before].body_json::<Value>().unwrap()["tools"].to_string();
            assert!(offered.contains("\"browser_open\""), "{offered}");
            assert!(!offered.contains("\"browser_devtools\""));
            break;
        }
        assert!(
            std::time::Instant::now() < deadline,
            "second run did not finish"
        );
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    task.abort();
}

#[tokio::test(flavor = "multi_thread")]
async fn cli_reads_and_saves_browser_settings_and_manages_browsing_data() {
    let data = tempfile::tempdir().unwrap();
    std::fs::write(
        data.path().join("runtime.jsonc"),
        "{\n  // my notes\n  \"schema_version\": 1\n}\n",
    )
    .unwrap();
    let state = AppState::new_for_test(data.path().join("settings.json"));
    let server = Server::bind(state.clone(), data.path()).await.unwrap();
    let task = tokio::spawn(server.run());
    let dir = data.path().to_owned();
    tokio::task::spawn_blocking(move || {
        let view = call(&dir, "get_browser_settings", json!({}));
        assert_eq!(view["settings"]["permissions"]["debugging"], "block");
        assert!(view["starter_sites"]
            .as_array()
            .unwrap()
            .iter()
            .any(|s| s == "paypal.com"));
        // Only the changed fields are sent, with the revision the page loaded.
        let mut permissions = view["settings"]["permissions"].clone();
        permissions["debugging"] = json!("ask");
        let saved = call(
            &dir,
            "update_browser_settings",
            json!({"changes": {"permissions": permissions, "show_secrets": true}, "revision": view["revision"]}),
        );
        assert_eq!(saved["settings"]["permissions"]["debugging"], "ask");
        assert_ne!(saved["revision"], view["revision"]);
        let file = std::fs::read_to_string(dir.join("runtime.jsonc")).unwrap();
        assert!(file.contains("// my notes"), "{file}");
        assert_eq!(
            call(&dir, "get_browser_settings", json!({}))["settings"]["show_secrets"],
            true
        );
        // A hand edit to the file while the page is open: the stale page's save is refused
        // and the hand edit stays.
        let edited = file.replace("\"developer_mode\": true", "\"developer_mode\": false");
        assert_ne!(edited, file, "{file}");
        std::fs::write(dir.join("runtime.jsonc"), &edited).unwrap();
        let stale = std::process::Command::new(env!("CARGO_BIN_EXE_themis"))
            .arg("--data-dir")
            .arg(&dir)
            .args([
                "call",
                "update_browser_settings",
                &json!({"changes": {"open_workspace": false}, "revision": saved["revision"]}).to_string(),
            ])
            .output()
            .unwrap();
        assert!(!stale.status.success());
        assert!(String::from_utf8_lossy(&stale.stderr).contains("changed in runtime.jsonc"));
        assert_eq!(std::fs::read_to_string(dir.join("runtime.jsonc")).unwrap(), edited);
        let fresh = call(&dir, "get_browser_settings", json!({}));
        assert_eq!(fresh["settings"]["developer_mode"], false);
        let saved = call(
            &dir,
            "update_browser_settings",
            json!({"changes": {"open_workspace": false}, "revision": fresh["revision"]}),
        );
        assert_eq!(saved["settings"]["developer_mode"], false, "the hand edit is kept");
        assert_eq!(saved["settings"]["open_workspace"], false);
        let file = std::fs::read_to_string(dir.join("runtime.jsonc")).unwrap();
        // An invalid site is refused and the file is left as it was.
        let bad = json!({"site_exceptions": [{"site": "https://x.com/path", "browsing": "block"}]});
        let refused = std::process::Command::new(env!("CARGO_BIN_EXE_themis"))
            .arg("--data-dir")
            .arg(&dir)
            .args([
                "call",
                "update_browser_settings",
                &json!({"changes": bad}).to_string(),
            ])
            .output()
            .unwrap();
        assert!(!refused.status.success());
        assert_eq!(
            std::fs::read_to_string(dir.join("runtime.jsonc")).unwrap(),
            file
        );
        // No browsing yet: empty history; clearing works and reports nothing.
        assert_eq!(
            call(
                &dir,
                "browser_history",
                json!({"projectRoot": null, "query": ""})
            )["entries"],
            json!([])
        );
        let cleared = call(
            &dir,
            "clear_browsing_data",
            json!({"projectRoot": null, "range": "all", "types": ["history", "cookies", "cache"]}),
        );
        assert_eq!(cleared["cleared"], json!([]));
        // Settings › Permissions: the editors offered start with the system text editor, and
        // only a known editor can be remembered.
        let editors = call(&dir, "list_editors", json!({}));
        assert_eq!(
            editors["editors"][0],
            json!({"id": "", "name": "Text editor"})
        );
        let chosen = call(
            &dir,
            "update_settings",
            json!({"patch": {"config_editor": "zed"}}),
        );
        assert_eq!(chosen["config_editor"], "zed");
        let refused = std::process::Command::new(env!("CARGO_BIN_EXE_themis"))
            .arg("--data-dir")
            .arg(&dir)
            .args([
                "call",
                "update_settings",
                &json!({"patch": {"config_editor": "sh -c x"}}).to_string(),
            ])
            .output()
            .unwrap();
        assert!(!refused.status.success());
        assert!(String::from_utf8_lossy(&refused.stderr).contains("Unknown editor"));
        let opened = std::process::Command::new(env!("CARGO_BIN_EXE_themis"))
            .arg("--data-dir")
            .arg(&dir)
            .args([
                "call",
                "open_runtime_config",
                &json!({"editor": "not-an-editor"}).to_string(),
            ])
            .output()
            .unwrap();
        assert!(!opened.status.success());
        assert!(String::from_utf8_lossy(&opened.stderr).contains("unknown editor"));
    })
    .await
    .unwrap();
    task.abort();
}
