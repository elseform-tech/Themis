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
    for (description, pinned) in [
        ("Initial description", false),
        ("Updated description", false),
        ("Initial description", true),
    ] {
        if description.starts_with("Updated") {
            let mut spec = plugin(description);
            spec["skills"].as_array_mut().unwrap().push(json!({"id":"new-sibling","name":"New sibling","description":"Newly installed sibling","instructions":"New workflow","allowedTools":[],"scripts":[]}));
            state.plugin_action(json!({"action":"save","scope":"local","projectRoot":project.path(),"spec":spec,"expectedRevision":first["revision"]})).await.unwrap();
        }
        let dir = data.path().to_path_buf();
        let thread_id = id.clone();
        let prompt = if pinned {
            format!(
                "[[skill:l--docs--{}--review]] Inspect documents",
                first["revision"].as_str().unwrap()
            )
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
        assert!(!system.contains("PRIVATE_SKILL_BODY"));
        assert!(system.contains("GLOBAL_LEGACY_DESCRIPTION"));
        assert!(!system.contains("LEGACY_BODY_STAYS_LAZY"));
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
