//! Configuration, import reports and logs through the production server and CLI.
use serde_json::{json, Value};
use std::path::Path;
use themis_desktop::{server::Server, state::AppState};
#[allow(dead_code)]
mod common;

fn cli(directory: &Path, method: &str, args: Value) -> Value {
    let output = std::process::Command::new(env!("CARGO_BIN_EXE_themis"))
        .args([
            "--data-dir",
            directory.to_str().unwrap(),
            "call",
            method,
            &args.to_string(),
        ])
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

#[test]
fn documented_configuration_examples_use_the_runtime_resolver() {
    let guide = include_str!("../../../docs/configuration.md").replace("\r\n", "\n");
    let examples: Vec<_> = guide
        .split("```jsonc\n")
        .skip(1)
        .map(|block| block.split("```").next().unwrap())
        .collect();
    assert_eq!(examples.len(), 2);
    let resolved =
        themis_core::configuration::resolve(Some(examples[0]), Some(examples[1]), None).unwrap();
    assert_eq!(resolved.config.context_token_budget, Some(200000));
    assert_eq!(resolved.provenance["instructions"], "project");
    assert_eq!(resolved.config.approval.rules[0].tool, "git");
}

#[tokio::test(flavor = "multi_thread")]
async fn shared_configuration_partial_import_and_diagnostics_survive_restart() {
    let directory = tempfile::tempdir().unwrap();
    let project = tempfile::tempdir().unwrap();
    let state = AppState::new_for_test(directory.path().join("settings.json"));
    let server = Server::bind(state.clone(), directory.path()).await.unwrap();
    let host = tokio::spawn(server.run());
    let root = project.path().to_str().unwrap();
    let saved = cli(
        directory.path(),
        "save_runtime_configuration",
        json!({"scope":"user","projectRoot":root,"json":"{ // team defaults\n\"context_token_budget\":200000,\"approval\":{\"default\":\"deny\",\"rules\":[]}}"}),
    );
    assert_eq!(saved["config"]["context_token_budget"], 200000);
    let thread = cli(
        directory.path(),
        "create_thread",
        json!({"projectRoot":root,"provider":"go","model":"test"}),
    );
    let id = thread["id"].as_str().unwrap();
    assert_eq!(
        cli(
            directory.path(),
            "set_thread_approval_mode",
            json!({"threadId":id,"mode":"yolo"})
        )["approval_mode"],
        "yolo"
    );
    let input = json!({"mcpServers":{"good":{"command":"node","args":[]},"legacy":{"type":"sse","url":"https://example.com/mcp"}}}).to_string();
    let preview = cli(
        directory.path(),
        "plugin_action",
        json!({"action":"inspect_json","name":"mixed","content":input}),
    );
    assert_eq!(preview["report"]["status"], "partial");
    assert!(preview["report"]["components"]
        .as_array()
        .unwrap()
        .iter()
        .any(|c| c["field"] == "mcpServers.legacy.type"));
    let installed = cli(
        directory.path(),
        "plugin_action",
        json!({"action":"import_json","name":"mixed","content":input,"allowPartial":true}),
    );
    assert_eq!(installed["spec"]["mcp"]["good"]["enabled"], false);
    assert!(installed["spec"]["mcp"].get("legacy").is_none());
    let logs = cli(
        directory.path(),
        "query_diagnostic_logs",
        json!({"service":"integration"}),
    );
    assert!(logs
        .as_array()
        .unwrap()
        .iter()
        .any(|r| r["details"]["action"] == "inspect_json"));
    cli(directory.path(), "shutdown", Value::Null);
    host.await.unwrap().unwrap();
    let restored = AppState::new_for_test(directory.path().join("settings.json"));
    assert_eq!(
        restored.get_thread(id).await.unwrap().approval_mode,
        themis_core::configuration::ApprovalMode::Yolo
    );
    assert_eq!(
        restored
            .get_runtime_configuration(Some(root.into()))
            .unwrap()["config"]["context_token_budget"],
        200000
    );
    assert!(!restored
        .query_diagnostic_logs(Default::default())
        .unwrap()
        .is_empty());
    std::fs::write(directory.path().join("runtime.jsonc"), "{bad").unwrap();
    let broken = restored
        .get_runtime_configuration(Some(root.into()))
        .unwrap();
    assert!(broken["config"].is_null());
    assert_eq!(broken["user_json"], "{bad");
    assert!(broken["validation_error"].is_string());
    restored
        .save_runtime_configuration("user", Some(root.into()), "{}")
        .unwrap();
}

#[cfg(unix)]
#[test]
fn project_configuration_cannot_read_outside_symlinks() {
    let data = tempfile::tempdir().unwrap();
    let root = tempfile::tempdir().unwrap();
    let outside = tempfile::tempdir().unwrap();
    std::fs::write(
        outside.path().join("config.jsonc"),
        "{\"private\":\"do-not-read\"}",
    )
    .unwrap();
    std::os::unix::fs::symlink(outside.path(), root.path().join(".themis")).unwrap();
    let state = AppState::new_for_test(data.path().join("settings.json"));
    let result = state.get_runtime_configuration(Some(root.path().to_string_lossy().into()));
    assert!(result.unwrap_err().contains("inside the project"));
}

#[tokio::test(flavor = "multi_thread")]
async fn configured_policy_and_repository_guidance_reach_the_actual_run() {
    use themis_desktop::server::Client;
    let (_repository, root) = common::init_repo();
    std::fs::write(
        root.join("AGENTS.md"),
        "CONFIG_GUIDANCE_MARKER: preserve fixtures",
    )
    .unwrap();
    let data = tempfile::tempdir().unwrap();
    let state = AppState::new_for_test(data.path().join("settings.json"));
    let provider = common::mount_scripted_llm().await;
    common::use_mock_llm(&state, &provider).await;
    let server = Server::bind(state.clone(), data.path()).await.unwrap();
    let host = tokio::spawn(server.run());
    cli(
        data.path(),
        "save_runtime_configuration",
        json!({
            "scope":"user", "json":r#"{"instructions":["AGENTS.md"],"approval":{"default":"deny","rules":[]}}"#
        }),
    );
    let thread = cli(
        data.path(),
        "create_thread",
        json!({
            "projectRoot":root,"provider":"go","model":"test-model"
        }),
    );
    let id = thread["id"].as_str().unwrap();
    for mode in ["custom", "yolo"] {
        cli(
            data.path(),
            "set_thread_approval_mode",
            json!({"threadId":id,"mode":mode}),
        );
        let mut events = Client::new(data.path().to_path_buf())
            .subscribe()
            .await
            .unwrap();
        let handle = cli(
            data.path(),
            "send_message",
            json!({"threadId":id,"text":"write hello.txt"}),
        );
        let run_id = handle["run_id"].as_str().unwrap();
        tokio::time::timeout(std::time::Duration::from_secs(30), async {
            loop {
                let event = Client::next_event(&mut events).await.unwrap();
                assert_ne!(
                    event["name"], "approval-request",
                    "policy must decide without asking"
                );
                if event["name"] == "thread-event" && event["payload"]["run_id"] == run_id {
                    match event["payload"]["event"]["kind"].as_str() {
                        Some("finished") => break,
                        Some("failed") => panic!("run failed: {event}"),
                        _ => {}
                    }
                }
            }
        })
        .await
        .unwrap();
        assert_eq!(root.join("hello.txt").exists(), mode == "yolo");
    }
    let requests = provider.received_requests().await.unwrap();
    assert!(requests
        .iter()
        .any(|r| String::from_utf8_lossy(&r.body).contains("CONFIG_GUIDANCE_MARKER")));
    assert!(state
        .query_diagnostic_logs(Default::default())
        .unwrap()
        .iter()
        .any(|r| r.service == "approval" && r.details["decision"] == "Deny"));
    // Legacy confirm-reads still applies when all policy provenance is default.
    provider.reset().await;
    use wiremock::matchers::{method, path};
    use wiremock::{Mock, Request, ResponseTemplate};
    Mock::given(method("POST"))
        .and(path("/chat/completions"))
        .respond_with(|request: &Request| {
            let body: Value = serde_json::from_slice(&request.body).unwrap();
            let response = if body["messages"]
                .as_array()
                .unwrap()
                .iter()
                .any(|m| m["role"] == "tool")
            {
                common::final_text_body()
            } else {
                let mut response = common::tool_call_body();
                response["choices"][0]["message"]["tool_calls"][0]["function"] = json!({
                    "name":"read_file","arguments":r#"{"file_path":"README.md"}"#
                });
                response
            };
            ResponseTemplate::new(200).set_body_json(response)
        })
        .mount(&provider)
        .await;
    cli(
        data.path(),
        "save_runtime_configuration",
        json!({"scope":"user","json":"{}"}),
    );
    cli(
        data.path(),
        "update_settings",
        json!({"patch":{"confirm_reads":true}}),
    );
    let read_thread = cli(
        data.path(),
        "create_thread",
        json!({"projectRoot":root,"provider":"go","model":"test-model"}),
    );
    let read_id = read_thread["id"].as_str().unwrap();
    let mut events = Client::new(data.path().to_path_buf())
        .subscribe()
        .await
        .unwrap();
    cli(
        data.path(),
        "send_message",
        json!({"threadId":read_id,"text":"read README.md"}),
    );
    tokio::time::timeout(std::time::Duration::from_secs(30), async {
        loop {
            let event = Client::next_event(&mut events).await.unwrap();
            if event["name"] == "approval-request" {
                assert_eq!(event["payload"]["thread_id"],read_id);
                cli(data.path(), "approve_action", json!({"threadId":read_id,"approvalId":event["payload"]["approval_id"],"decision":"deny"}));
                break;
            }
            assert_ne!(event["payload"]["event"]["kind"], "finished", "read approval must not be bypassed");
        }
    }).await.unwrap();
    cli(data.path(), "shutdown", Value::Null);
    host.await.unwrap().unwrap();
}

#[tokio::test(flavor = "multi_thread")]
async fn cli_catalog_inspection_distinguishes_partial_and_unsupported() {
    let data = tempfile::tempdir().unwrap();
    let market = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(market.path().join(".claude-plugin")).unwrap();
    for name in ["useful", "empty"] {
        std::fs::create_dir_all(market.path().join(name)).unwrap();
        std::fs::write(
            market.path().join(name).join(".mcp.json"),
            r#"{"mcpServers":{"legacy":{"type":"sse","url":"https://example.com/events"}}}"#,
        )
        .unwrap();
    }
    std::fs::write(
        market.path().join("useful/SKILL.md"),
        "---\nname: useful\ndescription: Useful fixture\n---\nReview the fixture.",
    )
    .unwrap();
    std::fs::write(market.path().join(".claude-plugin/marketplace.json"), r#"{"name":"fixture","plugins":[{"name":"useful","source":"./useful"},{"name":"empty","source":"./empty"}]}"#).unwrap();
    let state = AppState::new_for_test(data.path().join("settings.json"));
    let server = Server::bind(state, data.path()).await.unwrap();
    let host = tokio::spawn(server.run());
    cli(
        data.path(),
        "plugin_action",
        json!({"action":"add_marketplace","name":"fixture","source":market.path()}),
    );
    for (name, status) in [("useful", "partial"), ("empty", "unsupported")] {
        assert_eq!(
            cli(
                data.path(),
                "plugin_action",
                json!({"action":"inspect_marketplace","marketplace":"fixture","name":name})
            )["report"]["status"],
            status
        );
    }
    let output = std::process::Command::new(env!("CARGO_BIN_EXE_themis"))
        .args([
            "--data-dir",
            data.path().to_str().unwrap(),
            "call",
            "plugin_action",
            r#"{"action":"install","marketplace":"fixture","name":"empty","allowPartial":true}"#,
        ])
        .env_remove("OPENCODE_KEY")
        .output()
        .unwrap();
    assert!(!output.status.success());
    assert!(String::from_utf8_lossy(&output.stderr).contains("No supported capabilities"));
    let installed = cli(
        data.path(),
        "plugin_action",
        json!({"action":"install","marketplace":"fixture","name":"useful","allowPartial":true}),
    );
    assert_eq!(installed["spec"]["skills"].as_array().unwrap().len(), 1);
    assert_eq!(
        cli(data.path(), "get_pending_approvals", Value::Null),
        json!([])
    );
    cli(data.path(), "shutdown", Value::Null);
    host.await.unwrap().unwrap();
}
