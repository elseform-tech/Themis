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
    let server = MockServer::builder().start().await;
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
    // The wire system message: recall guidance with keyword advice, the greeting rule, and
    // no JSONL/read_file recovery recipe while recall is offered.
    assert!(system.contains("call the `recall` tool with 2-5 distinctive keywords"));
    assert!(system.contains("A checkpoint is a lossy paraphrase, not a source"));
    assert!(system.contains("do not recap, summarize or resume earlier or parked work"));
    assert!(!system.contains("Bounded reads include record_role"));
    assert!(!system.contains("decode_tool_payload"));
    assert!(!system.contains("evidence_kind recovery"));
    assert!(system.contains("Archived observations may be stale"));
    assert!(system.contains("tool restrictions"));
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
    let line = text
        .lines()
        .nth(hit["record"].as_u64().unwrap() as usize - 1)
        .unwrap();
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
    let slice: String = chars
        [range[0].as_u64().unwrap() as usize..range[1].as_u64().unwrap() as usize]
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
    let definition =
        json!({"name":tool.name(),"description":tool.description(),"parameters":schema})
            .to_string();
    println!("recall tool definition bytes: {}", definition.len());
    assert!(definition.len() < 2_000, "tool definition stays small");
    for field in ["query", "limit", "kinds", "source", "around"] {
        assert!(schema["properties"][field].is_object(), "{field}");
    }
}

async fn run_scripted_recalls(calls: usize, around: bool) -> anyhow::Result<String> {
    use std::sync::atomic::{AtomicUsize, Ordering};
    let server = MockServer::builder().start().await;
    let counter = Arc::new(AtomicUsize::new(0));
    Mock::given(method("POST"))
        .respond_with(move |_: &Request| {
            let n = counter.fetch_add(1, Ordering::SeqCst);
            if n >= calls {
                return ResponseTemplate::new(200).set_body_json(common::final_text_body("DONE"));
            }
            let args = if around {
                json!({"around": format!("x|1|/content|{n}")})
            } else {
                json!({"query": format!("needle{n}")})
            };
            ResponseTemplate::new(200).set_body_json(common::tool_call_body(
                &format!("c{n}"),
                "recall",
                args,
            ))
        })
        .mount(&server)
        .await;
    let project = tempfile::tempdir().unwrap();
    let root = project.path().canonicalize().unwrap();
    let evidence = root.join(".themis/context/t");
    std::fs::create_dir_all(&evidence).unwrap();
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
        attachment_root: None,
        project_root: Some(root.clone()),
    })));
    let (tx, mut rx) = tokio::sync::mpsc::channel(8192);
    tokio::spawn(async move { while rx.recv().await.is_some() {} });
    themis_core::runtime::run_task_with_evidence(
        llm,
        tools,
        "search".into(),
        Vec::new(),
        approvals,
        RunPolicy {
            total_turns: 400,
            context_token_budget: 200_000,
            recent_messages: 4,
        },
        tx,
        Arc::new(AtomicBool::new(false)),
        String::new(),
        Some(evidence),
    )
    .await
}

#[tokio::test]
async fn many_recall_searches_never_stop_the_run() {
    // Only the turn limit bounds a run; 140 searches used to be stopped at 128.
    assert_eq!(run_scripted_recalls(140, false).await.unwrap(), "DONE");
}

#[tokio::test]
async fn around_paging_is_not_counted_against_the_search_bound() {
    assert_eq!(run_scripted_recalls(140, true).await.unwrap(), "DONE");
}
