use serde_json::{json, Value};
use std::path::Path;
use themis_desktop::{
    server::{Client, Server},
    state::AppState,
};

fn cli(dir: &Path, args: &[&str]) -> Value {
    let out = std::process::Command::new(env!("CARGO_BIN_EXE_themis"))
        .arg("--data-dir")
        .arg(dir)
        .args(args)
        .env_remove("OPENCODE_KEY")
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    serde_json::from_slice(&out.stdout).unwrap()
}
#[tokio::test(flavor = "multi_thread")]
async fn cli_plugin_crud_is_visible_to_app_and_preserves_pinned_skills() {
    let data = tempfile::tempdir().unwrap();
    let project = tempfile::tempdir().unwrap();
    let state = AppState::new_for_test(data.path().join("settings.json"));
    let server = Server::bind(state.clone(), data.path()).await.unwrap();
    let task = tokio::spawn(server.run());
    let created = cli(
        data.path(),
        &[
            "plugin",
            "create",
            "personal",
            "--scope",
            "local",
            "--project",
            project.path().to_str().unwrap(),
        ],
    );
    assert_eq!(created["spec"]["name"], "personal");
    let draft = project.path().join("review.json");
    std::fs::write(&draft,r#"{"id":"review","name":"Review","description":"Inspect changes","instructions":"Inspect ownership","allowedTools":[],"scripts":[]}"#).unwrap();
    cli(
        data.path(),
        &[
            "skill",
            "save",
            "personal",
            draft.to_str().unwrap(),
            "--scope",
            "local",
            "--project",
            project.path().to_str().unwrap(),
        ],
    );
    let skills = state
        .prompt_skills(Some(project.path().to_str().unwrap().into()))
        .await
        .unwrap();
    assert!(skills.iter().any(|s| s.name == "Review"));
    cli(
        data.path(),
        &[
            "plugin",
            "disable",
            "personal",
            "--scope",
            "local",
            "--project",
            project.path().to_str().unwrap(),
        ],
    );
    assert!(!state
        .prompt_skills(Some(project.path().to_str().unwrap().into()))
        .await
        .unwrap()
        .iter()
        .any(|s| s.name == "Review"));
    Client::new(data.path().into())
        .call("shutdown", json!({}))
        .await
        .unwrap();
    task.await.unwrap().unwrap();
}

#[tokio::test(flavor = "multi_thread")]
async fn plain_cli_prompt_loads_catalog_mcp_hooks_and_refreshes_next_turn() {
    use std::time::Duration;
    use wiremock::{matchers::method, Mock, MockServer, Request, ResponseTemplate};
    let data = tempfile::tempdir().unwrap();
    let project = tempfile::tempdir().unwrap();
    let state = AppState::new_for_test(data.path().join("settings.json"));
    let llm = MockServer::start().await;
    Mock::given(method("POST")).respond_with(ResponseTemplate::new(200).set_body_json(json!({"choices":[{"message":{"role":"assistant","content":"done"},"finish_reason":"stop"}]}))).mount(&llm).await;
    state
        .set_secret("go".into(), "test-key".into())
        .await
        .unwrap();
    state.set_go_base_url_override(Some(llm.uri()));
    state.create_skill(serde_json::from_value(json!({"name":"Global legacy","description":"GLOBAL_LEGACY_DESCRIPTION","instructions":"LEGACY_BODY_STAYS_LAZY","allowed_tools":[],"scripts":[]})).unwrap()).await.unwrap();
    let mcp = MockServer::start().await;
    Mock::given(method("POST")).respond_with(|request: &Request| {
        let body: Value = serde_json::from_slice(&request.body).unwrap();
        let result = match body["method"].as_str().unwrap() {
            "initialize" => json!({"protocolVersion":"2025-03-26","capabilities":{},"serverInfo":{"name":"test","version":"1"}}),
            "tools/list" => json!({"tools":[{"name":"lookup","description":"Lookup external documents","inputSchema":{"type":"object","properties":{}}}]}),
            _ => json!({}),
        };
        ResponseTemplate::new(200).set_body_json(json!({"jsonrpc":"2.0","id":body["id"],"result":result}))
    }).mount(&mcp).await;
    let plugin = |description: &str| json!({"name":"docs","skills":[{"id":"review","name":"Review docs","description":description,"instructions":"PRIVATE_SKILL_BODY","allowedTools":[],"scripts":[]}],"mcp":{"docs":{"url":mcp.uri(),"enabled":true}},"hooks":[{"name":"started","event":"RunStart","command":"cat > run-start.json","enabled":true}],"files":{}});
    let first = state.plugin_action(json!({"action":"save","scope":"local","projectRoot":project.path(),"spec":plugin("Initial description")})).await.unwrap();
    let server = Server::bind(state.clone(), data.path()).await.unwrap();
    let task = tokio::spawn(server.run());
    let created = cli(
        data.path(),
        &[
            "thread",
            "create",
            project.path().to_str().unwrap(),
            "test-model",
        ],
    );
    let id = created["id"].as_str().unwrap().to_owned();
    let mut events = Client::new(data.path().into()).subscribe().await.unwrap();
    for (description, pinned, mentioned) in [
        ("Initial description", false, false),
        ("Updated description", false, false),
        ("Initial description", true, false),
        ("Updated description", false, true),
    ] {
        if description.starts_with("Updated") && !mentioned {
            let mut spec = plugin(description);
            spec["skills"].as_array_mut().unwrap().push(json!({"id":"new-sibling","name":"New sibling","description":"Newly installed sibling","instructions":"New workflow","allowedTools":[],"scripts":[]}));
            spec["manual_skills"] = json!(["new-sibling"]);
            state.plugin_action(json!({"action":"save","scope":"local","projectRoot":project.path(),"spec":spec,"expectedRevision":first["revision"]})).await.unwrap();
        }
        let dir = data.path().to_path_buf();
        let thread_id = id.clone();
        let prompt = if pinned {
            format!(
                "[[skill:l--docs--{}--review]] Inspect documents",
                first["revision"].as_str().unwrap()
            )
        } else if mentioned {
            "[[plugin:l--docs]] Inspect documents".into()
        } else {
            "Inspect documents".into()
        };
        let send = tokio::task::spawn_blocking(move || {
            cli(&dir, &["thread", "send", &thread_id, &prompt, "--detach"])
        });
        tokio::time::timeout(Duration::from_secs(30), async {
            loop {
                let event = Client::next_event(&mut events).await.unwrap();
                if event["name"] == "approval-request" {
                    Client::new(data.path().into()).call("approve_action",json!({"threadId":id,"approvalId":event["payload"]["approval_id"],"decision":"once"})).await.unwrap();
                }
                if event["name"] == "thread-event" && event["payload"]["event"]["kind"] == "finished" { break; }
            }
        }).await.unwrap();
        send.await.unwrap();
        let requests = llm.received_requests().await.unwrap();
        let request: Value = serde_json::from_slice(&requests.last().unwrap().body).unwrap();
        let system = request["messages"][0]["content"].as_str().unwrap();
        assert!(system.contains(description), "{system}");
        let catalog: Value = serde_json::from_str(
            system
                .split("Available skills (metadata only): ")
                .nth(1)
                .unwrap()
                .lines()
                .next()
                .unwrap(),
        )
        .unwrap();
        let manager = catalog
            .as_array()
            .unwrap()
            .iter()
            .find(|skill| skill["id"] == "manage-plugins")
            .unwrap();
        let guide = std::fs::read_to_string(manager["path"].as_str().unwrap()).unwrap();
        assert!(guide.contains("configure and troubleshoot integrations in chat"));
        assert!(guide.contains("basic install, enable, disable and uninstall controls"));
        assert!(!system.contains("Do not direct users to configuration forms"));
        assert!(!system.contains("PRIVATE_SKILL_BODY"));
        assert!(system.contains("GLOBAL_LEGACY_DESCRIPTION"));
        assert!(!system.contains("LEGACY_BODY_STAYS_LAZY"));
        assert_eq!(system.contains("Newly installed sibling"), mentioned);
        if mentioned {
            assert!(!system.contains("New workflow"));
            assert!(request["messages"]
                .as_array()
                .unwrap()
                .iter()
                .any(|message| message["role"] == "user"
                    && message["content"]
                        .as_str()
                        .is_some_and(|text| text.contains("@docs Inspect documents"))));
        }
        assert!(request["tools"]
            .as_array()
            .unwrap()
            .iter()
            .any(|tool| tool["function"]["description"] == "Lookup external documents"));
        let hook: Value =
            serde_json::from_slice(&std::fs::read(project.path().join("run-start.json")).unwrap())
                .unwrap();
        assert_eq!(hook["event"], "RunStart");
        assert_eq!(hook["thread_id"], id);
    }
    Client::new(data.path().into())
        .call("shutdown", json!({}))
        .await
        .unwrap();
    task.await.unwrap().unwrap();
}

#[tokio::test(flavor = "multi_thread")]
async fn agent_can_disable_broken_enabled_mcp_without_losing_chat() {
    use std::time::Duration;
    use wiremock::{matchers::method, Mock, MockServer, Request, ResponseTemplate};
    let data = tempfile::tempdir().unwrap();
    let project = tempfile::tempdir().unwrap();
    let state = AppState::new_for_test(data.path().join("settings.json"));
    let provider = MockServer::start().await;
    Mock::given(method("POST")).respond_with(|request: &Request| {
        let body: Value = serde_json::from_slice(&request.body).unwrap();
        let handled = body["messages"].as_array().unwrap().iter().any(|message| message["role"] == "tool");
        let message = if handled { json!({"role":"assistant","content":"Disabled the broken connection"}) }
            else { json!({"role":"assistant","tool_calls":[{"id":"disable","type":"function","function":{"name":"manage_integrations","arguments":json!({"action":"set_component_enabled","name":"broken","kind":"mcp","id":"missing","enabled":false}).to_string()}}]}) };
        ResponseTemplate::new(200).set_body_json(json!({"choices":[{"message":message,"finish_reason":if handled {"stop"} else {"tool_calls"}}]}))
    }).mount(&provider).await;
    state
        .set_secret("go".into(), "test-key".into())
        .await
        .unwrap();
    state.set_go_base_url_override(Some(provider.uri()));
    state.plugin_action(json!({"action":"save","scope":"local","projectRoot":project.path(),"spec":{"name":"broken","mcp":{"missing":{"command":project.path().join("nonexistent-mcp"),"args":["SENSITIVE_TEST_ARGUMENT"],"enabled":true}}}})).await.unwrap();
    let server = Server::bind(state.clone(), data.path()).await.unwrap();
    let serving = tokio::spawn(server.run());
    let created = cli(
        data.path(),
        &[
            "thread",
            "create",
            project.path().to_str().unwrap(),
            "test-model",
        ],
    );
    let id = created["id"].as_str().unwrap().to_owned();
    let mut events = Client::new(data.path().into()).subscribe().await.unwrap();
    let directory = data.path().to_path_buf();
    let thread_id = id.clone();
    let send = tokio::task::spawn_blocking(move || {
        cli(
            &directory,
            &[
                "thread",
                "send",
                &thread_id,
                "Disable the broken connection",
                "--detach",
            ],
        )
    });
    let mut startup_warning = None;
    let terminal = tokio::time::timeout(Duration::from_secs(15), async {
        loop {
            let event = Client::next_event(&mut events).await.unwrap();
            if event["name"] == "approval-request" {
                Client::new(data.path().into()).call("approve_action",json!({"threadId":id,"approvalId":event["payload"]["approval_id"],"decision":"once"})).await.unwrap();
            }
            if event["name"] == "thread-event" && event["payload"]["event"]["kind"] == "tool_finished"
                && event["payload"]["event"]["tool"].as_str().is_some_and(|name| name.starts_with("mcp_start_")) {
                assert_eq!(event["payload"]["event"]["ok"], false);
                startup_warning = Some(event["payload"]["event"]["output"].as_str().unwrap().to_owned());
            }
            if event["name"] == "thread-event" && matches!(event["payload"]["event"]["kind"].as_str(),Some("finished" | "failed")) { break event; }
        }
    }).await.unwrap();
    assert_eq!(
        terminal["payload"]["event"]["kind"], "finished",
        "{terminal}"
    );
    send.await.unwrap();
    let saved = state
        .plugin_action(json!({"action":"list","projectRoot":project.path()}))
        .await
        .unwrap();
    assert_eq!(
        saved
            .as_array()
            .unwrap()
            .iter()
            .find(|plugin| plugin["spec"]["name"] == "broken")
            .unwrap()["spec"]["mcp"]["missing"]["enabled"],
        false
    );
    let requests = provider.received_requests().await.unwrap();
    let first: Value = serde_json::from_slice(&requests[0].body).unwrap();
    let system = first["messages"][0]["content"].as_str().unwrap();
    assert!(system.contains("broken/missing"));
    assert!(system.contains("unavailable"));
    assert!(!system.contains("SENSITIVE_TEST_ARGUMENT"));
    let warning = startup_warning.expect("MCP startup failure is surfaced in run events");
    assert!(warning.contains("broken/missing"));
    assert!(warning.contains("server process could not start"));
    assert!(!warning.contains("SENSITIVE_TEST_ARGUMENT"));
    assert!(warning.len() < 1024);
    assert!(first["tools"]
        .as_array()
        .unwrap()
        .iter()
        .any(|tool| tool["function"]["name"] == "manage_integrations"));
    Client::new(data.path().into())
        .call("shutdown", json!({}))
        .await
        .unwrap();
    serving.await.unwrap().unwrap();
}

#[tokio::test(flavor = "multi_thread")]
async fn agent_marketplace_management_dispatches_approvals_and_refreshes_catalog() {
    use std::time::Duration;
    use wiremock::{matchers::method, Mock, MockServer, Request, ResponseTemplate};
    let data = tempfile::tempdir().unwrap();
    let project = tempfile::tempdir().unwrap();
    let marketplace = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(marketplace.path().join(".claude-plugin")).unwrap();
    std::fs::create_dir_all(marketplace.path().join("bundle/skills/review")).unwrap();
    std::fs::write(
        marketplace.path().join(".claude-plugin/marketplace.json"),
        r#"{"plugins":[{"name":"agent-bundle","source":"./bundle"}]}"#,
    )
    .unwrap();
    std::fs::write(marketplace.path().join("bundle/skills/review/SKILL.md"),"---\nname: review\ndescription: UNIQUE_INSTALLED_REVIEW_METADATA\n---\nPRIVATE_INSTALLED_REVIEW_BODY\n").unwrap();
    let state = AppState::new_for_test(data.path().join("settings.json"));
    state.plugin_action(json!({"action":"add_marketplace","name":"fixture","source":marketplace.path(),"projectRoot":project.path()})).await.unwrap();
    let provider = MockServer::start().await;
    Mock::given(method("POST")).respond_with(|request: &Request| {
        let body: Value = serde_json::from_slice(&request.body).unwrap();
        let messages = body["messages"].as_array().unwrap();
        let latest = messages.iter().rfind(|message|message["role"] == "user").unwrap()["content"].as_str().unwrap();
        let step = messages.iter().filter(|message|message["role"] == "tool").count();
        let actions = if latest.contains("Clean up") { vec![
            json!({"action":"set_component_enabled","name":"agent-bundle","kind":"skill","id":"review","enabled":false}),
            json!({"action":"remove_component","name":"agent-bundle","kind":"skill","id":"review"}),
            json!({"action":"delete","name":"agent-bundle"}),
        ] } else if latest.contains("Install") { vec![
            json!({"action":"preview","marketplace":"fixture","name":"agent-bundle"}),
            json!({"action":"install","marketplace":"fixture","name":"agent-bundle"}),
        ] } else { vec![] };
        let message = if let Some(args) = actions.get(step) {
            json!({"role":"assistant","tool_calls":[{"id":format!("operation-{step}"),"type":"function","function":{"name":"manage_integrations","arguments":args.to_string()}}]})
        } else { json!({"role":"assistant","content":"Requested management completed"}) };
        ResponseTemplate::new(200).set_body_json(json!({"choices":[{"message":message,"finish_reason":if step<actions.len() {"tool_calls"} else {"stop"}}]}))
    }).mount(&provider).await;
    state
        .set_secret("go".into(), "test-key".into())
        .await
        .unwrap();
    state.set_go_base_url_override(Some(provider.uri()));
    let server = Server::bind(state.clone(), data.path()).await.unwrap();
    let serving = tokio::spawn(server.run());
    let thread = cli(
        data.path(),
        &[
            "thread",
            "create",
            project.path().to_str().unwrap(),
            "test-model",
        ],
    );
    let id = thread["id"].as_str().unwrap().to_owned();
    let mut events = Client::new(data.path().into()).subscribe().await.unwrap();
    let mut approval_count = 0;
    for (prompt, expected_metadata, expected_tool_results) in [
        ("Install the public package", false, 2),
        ("Clean up the installed package", true, 3),
        ("Confirm cleanup", false, 0),
    ] {
        let request_start = provider.received_requests().await.unwrap().len();
        cli(data.path(), &["thread", "send", &id, prompt, "--detach"]);
        let terminal = tokio::time::timeout(Duration::from_secs(5), async {
            loop {
                let event = Client::next_event(&mut events).await.unwrap();
                if event["name"] == "approval-request" {
                    approval_count += 1;
                    Client::new(data.path().into()).call("approve_action",json!({"threadId":id,"approvalId":event["payload"]["approval_id"],"decision":"once"})).await.unwrap();
                }
                if event["name"] == "thread-event" && matches!(event["payload"]["event"]["kind"].as_str(),Some("finished"|"failed")) { break event; }
            }
        }).await.unwrap();
        assert_eq!(
            terminal["payload"]["event"]["kind"], "finished",
            "{terminal}"
        );
        let requests = provider.received_requests().await.unwrap();
        let first: Value = serde_json::from_slice(&requests[request_start].body).unwrap();
        let system = first["messages"][0]["content"].as_str().unwrap();
        assert_eq!(
            system.contains("UNIQUE_INSTALLED_REVIEW_METADATA"),
            expected_metadata
        );
        assert!(!system.contains("PRIVATE_INSTALLED_REVIEW_BODY"));
        let last: Value = serde_json::from_slice(&requests.last().unwrap().body).unwrap();
        let results: Vec<_> = last["messages"]
            .as_array()
            .unwrap()
            .iter()
            .filter(|message| message["role"] == "tool")
            .collect();
        assert_eq!(results.len(), expected_tool_results);
        for result in results {
            serde_json::from_str::<Value>(result["content"].as_str().unwrap())
                .expect("Shared management mutations return JSON, not tool errors");
        }
        let packages = state
            .plugin_action(json!({"action":"list","projectRoot":project.path()}))
            .await
            .unwrap();
        let installed = packages
            .as_array()
            .unwrap()
            .iter()
            .find(|plugin| plugin["spec"]["name"] == "agent-bundle");
        if prompt.contains("Install") {
            assert_eq!(installed.unwrap()["spec"]["package_kind"], "plugin");
        } else {
            assert!(installed.is_none());
        }
    }
    assert_eq!(
        approval_count, 5,
        "Every preview/install/mutation received explicit one-time approval"
    );
    Client::new(data.path().into())
        .call("shutdown", json!({}))
        .await
        .unwrap();
    serving.await.unwrap().unwrap();
}
