//! Scripted-provider proof that `recall` recovers a detail that exists only in
//! saved originals after a forced compaction. The provider is scripted, so this
//! checks the harness and the tool wiring, not live model quality.

mod common;

use std::sync::{atomic::AtomicBool, Arc};

use serde_json::{json, Value};
use themis_core::providers::{resolve, ProviderConfig, ProviderKind};
use themis_core::runtime::{ConversationRole, ConversationTurn, RunPolicy};
use themis_core::tools::recall::{RecallScope, RecallTool};
use themis_core::tools::{boxed_tools, AllowAllHook, ApprovalHook, ToolT};
use wiremock::matchers::method;
use wiremock::{Mock, MockServer, Request, ResponseTemplate};

const SECRET: &str = "ZEPHYR-OTTER-4417";

#[tokio::test]
async fn recall_finds_a_planted_original_after_compaction() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .respond_with(|request: &Request| {
            let body: Value = serde_json::from_slice(&request.body).unwrap();
            let messages = body["messages"].as_array().unwrap();
            let summary = messages.iter().any(|m| {
                m["content"]
                    .as_str()
                    .is_some_and(|t| t.contains("You summarize agent context"))
            });
            let last = messages.last().unwrap();
            let answer = if summary {
                // The summary deliberately omits the planted detail.
                common::final_text_body("Goal/constraints: answer the user. Confirmed findings: district observations only. Completed changes/tests: observed actions only. Unresolved work: answer the question. Next action: continue from recorded state.")
            } else if last["role"] == "tool" {
                let result = last["content"].as_str().unwrap_or_default();
                common::final_text_body(&format!(
                    "RESULT {}",
                    if result.contains(SECRET) { SECRET } else { "unknown" }
                ))
            } else {
                common::tool_call_body(
                    "recall_1",
                    "recall",
                    json!({"query": "vault passphrase"}),
                )
            };
            ResponseTemplate::new(200).set_body_json(answer)
        })
        .mount(&server)
        .await;

    let project = tempfile::tempdir().unwrap();
    let root = project.path().canonicalize().unwrap();
    let evidence = root.join(".themis/context/thread-a");
    std::fs::create_dir_all(&evidence).unwrap();
    let budget = 24_000;
    let mut history = vec![ConversationTurn {
        role: ConversationRole::User,
        text: format!("Remember this for later: the vault passphrase is {SECRET}."),
    }];
    for index in 0..24 {
        history.push(ConversationTurn {
            role: ConversationRole::User,
            text: format!(
                "Unrelated historical packet {index}: {}",
                format!("district {index} observation; ").repeat(budget / 100)
            ),
        });
    }
    let llm = resolve(
        &ProviderConfig::new(ProviderKind::Go, "test-key")
            .with_model("test-model")
            .with_base_url(server.uri()),
    )
    .await
    .unwrap();
    let approvals: Arc<dyn ApprovalHook> = Arc::new(AllowAllHook);
    let mut tools = boxed_tools(&root, approvals.clone()).unwrap();
    tools.push(Box::new(RecallTool::new(RecallScope {
        evidence_dir: evidence.clone(),
        attachments: Vec::new(),
        attachment_root: Some(root.join(".themis/attachments")),
        project_root: Some(root.clone()),
    })));
    let (tx, mut rx) = tokio::sync::mpsc::channel(4096);
    let answer = themis_core::runtime::run_task_with_evidence(
        llm,
        tools,
        "What was the vault passphrase I asked you to remember?".into(),
        history,
        approvals,
        RunPolicy {
            total_turns: 6,
            context_token_budget: budget,
            recent_messages: 4,
        },
        tx,
        Arc::new(AtomicBool::new(false)),
        String::new(),
        Some(evidence.clone()),
    )
    .await
    .unwrap();
    assert_eq!(answer, format!("RESULT {SECRET}"));
    while rx.try_recv().is_ok() {}

    let requests = server.received_requests().await.unwrap();
    let main: Vec<Value> = requests
        .iter()
        .map(|r| serde_json::from_slice::<Value>(&r.body).unwrap())
        .filter(|b| {
            !b["messages"].as_array().unwrap().iter().any(|m| {
                m["content"]
                    .as_str()
                    .is_some_and(|t| t.contains("You summarize agent context"))
            })
        })
        .collect();
    let system = main.last().unwrap()["messages"][0]["content"]
        .as_str()
        .unwrap();
    assert!(system.contains("Use the `recall` tool to search this chat's saved originals"));
    assert!(!system.contains("Bounded reads include record_role"));
    assert!(main[0]["tools"]
        .as_array()
        .unwrap()
        .iter()
        .any(|t| t["function"]["name"] == "recall"));
    // The checkpoint block is the short pointer too.
    let all = serde_json::to_string(&main.last().unwrap()["messages"]).unwrap();
    assert!(all.contains("Older details are not in this summary"));
    assert!(!all.contains("decode_tool_payload=true searches/pages"));

    // Coordinates in the recall result are valid for the saved snapshot.
    let tool_result: Value = main.last().unwrap()["messages"]
        .as_array()
        .unwrap()
        .iter()
        .rev()
        .find(|m| m["role"] == "tool")
        .map(|m| serde_json::from_str(m["content"].as_str().unwrap()).unwrap())
        .unwrap();
    // The active request also mentions the query words; pick the planted original.
    let hit = tool_result["hits"]
        .as_array()
        .unwrap()
        .iter()
        .find(|hit| hit["excerpt"].as_str().unwrap().contains(SECRET))
        .expect("planted original is among the hits");
    assert_eq!(hit["match"], "phrase");
    assert_eq!(hit["evidence_kind"], "original");
    assert_eq!(hit["role"], "User");
    assert_eq!(hit["pointer"], "/content");
    assert_eq!(tool_result["untrusted"], true);
    let snapshot = evidence.join(hit["snapshot"].as_str().unwrap());
    let text = std::fs::read_to_string(&snapshot).unwrap();
    let line = text.lines().nth(hit["record"].as_u64().unwrap() as usize - 1).unwrap();
    let record: Value = serde_json::from_str(line).unwrap();
    assert!(record["content"].as_str().unwrap().contains(SECRET));
    use sha2::{Digest, Sha256};
    let digest: String = Sha256::digest(line.as_bytes())
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect();
    assert_eq!(hit["sha256"], digest);
    let range = hit["char_range"].as_array().unwrap();
    let chars: Vec<char> = record["content"].as_str().unwrap().chars().collect();
    let slice: String = chars[range[0].as_u64().unwrap() as usize..range[1].as_u64().unwrap() as usize]
        .iter()
        .collect();
    assert_eq!(slice, hit["excerpt"].as_str().unwrap());
}

#[test]
fn recall_tool_metadata_is_stable() {
    let tool = RecallTool::new(RecallScope {
        evidence_dir: std::env::temp_dir().join("none"),
        attachments: Vec::new(),
        attachment_root: None,
        project_root: None,
    });
    assert_eq!(tool.name(), "recall");
    let schema = tool.args_schema();
    for field in ["query", "limit", "kinds", "source", "around"] {
        assert!(schema["properties"][field].is_object(), "{field}");
    }
}
