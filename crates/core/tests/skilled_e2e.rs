//! Skilled-run integration tests: composed prompt + filtered tools on the wire.
//!
//! No real network: a wiremock LLM captures request bodies while serving a
//! scripted final-text response.

mod common;

use std::sync::{Arc, Mutex};

use serde_json::Value;
use themis_core::providers::{resolve, ProviderConfig, ProviderKind};
use themis_core::runtime::{run_task, run_task_skilled};
use themis_core::skills::{Skill, SkillScript};
use themis_core::tools::{boxed_tools, AllowAllHook, ApprovalHook};
use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

fn skill(
    id: &str,
    name: &str,
    instructions: &str,
    allowed_tools: &[&str],
    scripts: Vec<SkillScript>,
) -> Skill {
    Skill {
        id: id.to_owned(),
        name: name.to_owned(),
        description: format!("{name} description"),
        instructions: instructions.to_owned(),
        allowed_tools: allowed_tools.iter().map(ToString::to_string).collect(),
        scripts,
    }
}

/// Serves `POST /chat/completions` with a final-text reply, recording bodies.
async fn mount_capturing_final_text(
    server: &MockServer,
    received: Arc<Mutex<Vec<Value>>>,
    reply: &str,
) {
    let body = common::final_text_body(reply);
    Mock::given(method("POST"))
        .and(path("/chat/completions"))
        .respond_with(move |request: &wiremock::Request| {
            let parsed: Value = serde_json::from_slice(&request.body).unwrap();
            received
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner())
                .push(parsed);
            ResponseTemplate::new(200).set_body_json(body.clone())
        })
        .mount(server)
        .await;
}

fn tool_names_sent(request: &Value) -> Vec<String> {
    request["tools"]
        .as_array()
        .map(|tools| {
            tools
                .iter()
                .filter_map(|tool| tool["function"]["name"].as_str().map(ToString::to_string))
                .collect()
        })
        .unwrap_or_default()
}

fn first_user_text(request: &Value) -> String {
    request["messages"]
        .as_array()
        .and_then(|messages| messages.iter().find(|message| message["role"] == "user"))
        .and_then(|message| message["content"].as_str())
        .unwrap_or_default()
        .to_owned()
}

#[tokio::test]
async fn skilled_run_sends_composed_instructions_and_filtered_tools() {
    let server = MockServer::start().await;
    let received: Arc<Mutex<Vec<Value>>> = Arc::new(Mutex::new(Vec::new()));
    mount_capturing_final_text(&server, Arc::clone(&received), "done").await;

    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().join("proj");
    std::fs::create_dir(&root).unwrap();

    // Intersection of the two grants is exactly `read_file`.
    let skills = vec![
        skill(
            "alpha",
            "Alpha",
            "ALPHA-INSTRUCTIONS-42",
            &["read_file", "list_dir"],
            vec![SkillScript {
                name: "helper.sh".to_owned(),
                content: "echo hi".to_owned(),
            }],
        ),
        skill(
            "beta",
            "Beta",
            "BETA-INSTRUCTIONS-7",
            &["read_file", "shell"],
            vec![],
        ),
    ];

    let config = ProviderConfig::new(ProviderKind::Go, "test-key")
        .with_model("test-model")
        .with_base_url(server.uri());
    let llm = resolve(&config).await.unwrap();
    let approvals: Arc<dyn ApprovalHook> = Arc::new(AllowAllHook);
    let tools = boxed_tools(&root, Arc::clone(&approvals)).unwrap();
    assert_eq!(tools.len(), 11);

    let (events_tx, mut events_rx) = tokio::sync::mpsc::channel(1024);
    let result = run_task_skilled(
        llm,
        tools,
        "base task".to_string(),
        approvals,
        3,
        events_tx,
        &skills,
    )
    .await
    .unwrap();
    assert_eq!(result, "done");
    let events = common::drain(&mut events_rx).await;
    assert!(
        events
            .iter()
            .any(|event| matches!(event, themis_core::runtime::RunEvent::Finished { .. })),
        "events: {events:?}"
    );

    let requests = received.lock().unwrap().clone();
    assert_eq!(requests.len(), 1, "requests: {requests:?}");
    let prompt = first_user_text(&requests[0]);
    assert!(prompt.contains("base task"), "{prompt}");
    assert!(prompt.contains("## Skill: Alpha"), "{prompt}");
    assert!(prompt.contains("ALPHA-INSTRUCTIONS-42"), "{prompt}");
    assert!(prompt.contains("## Skill: Beta"), "{prompt}");
    assert!(prompt.contains("BETA-INSTRUCTIONS-7"), "{prompt}");
    assert!(prompt.contains("`alpha/helper.sh`"), "{prompt}");
    assert_eq!(tool_names_sent(&requests[0]), vec!["read_file".to_owned()]);
}

#[tokio::test]
async fn empty_skills_match_plain_run_task() {
    for skilled in [false, true] {
        let server = MockServer::start().await;
        let received: Arc<Mutex<Vec<Value>>> = Arc::new(Mutex::new(Vec::new()));
        mount_capturing_final_text(&server, Arc::clone(&received), "done").await;

        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path().join("proj");
        std::fs::create_dir(&root).unwrap();

        let config = ProviderConfig::new(ProviderKind::Go, "test-key")
            .with_model("test-model")
            .with_base_url(server.uri());
        let llm = resolve(&config).await.unwrap();
        let approvals: Arc<dyn ApprovalHook> = Arc::new(AllowAllHook);
        let tools = boxed_tools(&root, Arc::clone(&approvals)).unwrap();
        let (events_tx, _events_rx) = tokio::sync::mpsc::channel(1024);

        let result = if skilled {
            run_task_skilled(
                llm,
                tools,
                "plain".to_string(),
                approvals,
                3,
                events_tx,
                &[],
            )
            .await
        } else {
            run_task(llm, tools, "plain".to_string(), approvals, 3, events_tx).await
        }
        .unwrap();
        assert_eq!(result, "done");

        let requests = received.lock().unwrap().clone();
        assert_eq!(requests.len(), 1);
        assert_eq!(first_user_text(&requests[0]), "plain");
        assert_eq!(tool_names_sent(&requests[0]).len(), 11);
    }
}
