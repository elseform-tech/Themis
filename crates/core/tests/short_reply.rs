//! A short conversational reply must not pay for a checkpoint while the request still fits
//! the hard input budget; the next substantive turn compacts. Scripted provider, no network.

mod common;

use std::sync::{atomic::AtomicBool, Arc};

use themis_core::providers::{resolve, ProviderConfig, ProviderKind};
use themis_core::runtime::{
    is_short_reply, run_task_with_evidence, with_short_reply_turn, ConversationRole,
    ConversationTurn, RunEvent, RunPolicy,
};
use themis_core::tools::{boxed_tools, AllowAllHook, ApprovalHook};
use wiremock::MockServer;

const SUMMARY: &str = "Goal/constraints: finish the parser task. Unresolved work: tests. Parked work: none. Next action: if the user continues this task, run the tests.";

struct Outcome {
    events: Vec<RunEvent>,
    summary_requests: usize,
    ordinary_requests: usize,
    _project: tempfile::TempDir,
}

impl Outcome {
    fn checkpoints(&self) -> usize {
        self.events
            .iter()
            .filter(|event| matches!(event, RunEvent::ContextCheckpoint { .. }))
            .count()
    }
    fn first_budget(&self) -> &themis_core::runtime::ContextBudget {
        self.events
            .iter()
            .find_map(|event| match event {
                RunEvent::ContextBudget(budget) => Some(budget),
                _ => None,
            })
            .expect("a context budget event")
    }
}

/// `packets` historical user messages of about 250 estimated tokens each.
async fn run(packets: usize, message: &str, short: bool) -> Outcome {
    let server = MockServer::start().await;
    common::mount_script_with_summary(
        &server,
        vec![common::final_text_body("Hi! What can I help with?")],
        Some(SUMMARY),
    )
    .await;
    let project = tempfile::tempdir().unwrap();
    let evidence = project.path().join(".themis/context/short-reply");
    std::fs::create_dir_all(&evidence).unwrap();
    let history = (0..packets)
        .map(|index| ConversationTurn {
            role: ConversationRole::User,
            text: format!(
                "Historical packet {index}: {}",
                format!("district {index} observation; ").repeat(32)
            ),
        })
        .collect::<Vec<_>>();
    let llm = resolve(
        &ProviderConfig::new(ProviderKind::Go, "test-key")
            .with_model("test-model")
            .with_base_url(server.uri()),
    )
    .await
    .unwrap();
    let approvals: Arc<dyn ApprovalHook> = Arc::new(AllowAllHook);
    let (tx, mut rx) = tokio::sync::mpsc::channel(2048);
    let result = with_short_reply_turn(
        short,
        run_task_with_evidence(
            llm,
            boxed_tools(project.path(), approvals.clone()).unwrap(),
            message.into(),
            history,
            approvals,
            RunPolicy {
                total_turns: 3,
                context_token_budget: 24_000,
                recent_messages: 4,
            },
            tx,
            Arc::new(AtomicBool::new(false)),
            String::new(),
            Some(evidence),
        ),
    )
    .await;
    assert!(result.is_ok(), "{result:?}");
    let events = common::drain(&mut rx).await;
    let requests = server.received_requests().await.unwrap();
    let is_summary = |request: &wiremock::Request| {
        let body: serde_json::Value = request.body_json().unwrap();
        body["messages"][0]["content"]
            .as_str()
            .is_some_and(|text| text.contains("You summarize agent context"))
    };
    Outcome {
        events,
        summary_requests: requests.iter().filter(|r| is_summary(r)).count(),
        ordinary_requests: requests.iter().filter(|r| !is_summary(r)).count(),
        _project: project,
    }
}

/// Smallest packet count whose first request is above the semantic threshold.
async fn packets_over_threshold() -> usize {
    for packets in (40..120).step_by(1) {
        let outcome = run(packets, "hi", true).await;
        let budget = outcome.first_budget();
        if budget.used_tokens > budget.compaction_at_tokens {
            assert!(
                budget.used_tokens <= budget.usable_tokens,
                "band too narrow"
            );
            return packets;
        }
    }
    panic!("no packet count crossed the threshold");
}

#[tokio::test]
async fn short_greeting_defers_compaction_while_the_request_fits() {
    let packets = packets_over_threshold().await;
    let greeting = run(packets, "hi", true).await;
    let budget = greeting.first_budget();
    assert!(budget.used_tokens > budget.compaction_at_tokens);
    assert!(budget.used_tokens <= budget.usable_tokens);
    assert_eq!(greeting.checkpoints(), 0);
    assert_eq!(greeting.summary_requests, 0);
    assert_eq!(greeting.ordinary_requests, 1);

    // The same size with a substantive request (not marked short) compacts first.
    let task = run(
        packets,
        "Please refactor the parser module and run the tests.",
        false,
    )
    .await;
    assert_eq!(task.checkpoints(), 1);
    assert!(task.summary_requests >= 1);
}

#[tokio::test]
async fn short_reply_never_exceeds_the_hard_budget() {
    // Far above the usable budget: the flag cannot postpone the checkpoint.
    let outcome = run(200, "thanks!", true).await;
    assert!(outcome.checkpoints() >= 1);
    let budget = outcome.first_budget();
    assert!(budget.used_tokens <= budget.usable_tokens);
}

#[tokio::test]
async fn small_context_never_compacts_for_a_short_reply() {
    let outcome = run(3, "你好", true).await;
    assert_eq!(outcome.checkpoints(), 0);
    assert_eq!(outcome.summary_requests, 0);
}

#[test]
fn short_reply_needs_few_characters_and_no_attachments() {
    assert!(is_short_reply("hi", false));
    assert!(is_short_reply("  谢谢！ ", false));
    assert!(is_short_reply(&"a".repeat(200), false));
    assert!(!is_short_reply(&"a".repeat(201), false));
    assert!(!is_short_reply("hi", true));
    assert!(!is_short_reply("   ", false));
    // Length counts characters, not bytes.
    assert!(is_short_reply(&"谢".repeat(200), false));
}
