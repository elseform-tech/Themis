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
    summary_systems: Vec<String>,
    ordinary_bodies: Vec<String>,
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
    fn budget_after_checkpoint(&self) -> &themis_core::runtime::ContextBudget {
        self.events
            .iter()
            .skip_while(|event| !matches!(event, RunEvent::ContextCheckpoint { .. }))
            .find_map(|event| match event {
                RunEvent::ContextBudget(budget) => Some(budget),
                _ => None,
            })
            .expect("a context budget after the checkpoint")
    }
}

/// `packets` historical user messages of about 250 estimated tokens each.
async fn run(packets: usize, message: &str) -> Outcome {
    run_script(
        packets,
        message,
        vec![common::final_text_body("Hi! What can I help with?")],
    )
    .await
}

/// Like `run`, but the scripted model answers with `bodies` (tool calls allowed). The short
/// flag comes from the real `is_short_reply` on `message`, as the desktop computes it.
async fn run_script(packets: usize, message: &str, bodies: Vec<serde_json::Value>) -> Outcome {
    let short = is_short_reply(message, false);
    let server = MockServer::start().await;
    common::mount_script_with_summary(&server, bodies, Some(SUMMARY)).await;
    let project = tempfile::tempdir().unwrap();
    std::fs::write(
        project.path().join("big.txt"),
        "observation line\n".repeat(900),
    )
    .unwrap();
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
        ordinary_bodies: requests
            .iter()
            .filter(|r| !is_summary(r))
            .map(|r| String::from_utf8_lossy(&r.body).into_owned())
            .collect(),
        summary_systems: requests
            .iter()
            .filter(|r| is_summary(r))
            .map(|r| {
                let body: serde_json::Value = r.body_json().unwrap();
                body["messages"][0]["content"].as_str().unwrap().to_owned()
            })
            .collect(),
        _project: project,
    }
}

/// Smallest packet count whose first request is above the semantic threshold.
async fn packets_over_threshold() -> usize {
    for packets in (40..120).step_by(1) {
        let outcome = run(packets, "hi").await;
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
    let greeting = run(packets, "hi").await;
    let budget = greeting.first_budget();
    assert!(budget.used_tokens > budget.compaction_at_tokens);
    assert!(budget.used_tokens <= budget.usable_tokens);
    assert_eq!(greeting.checkpoints(), 0);
    assert_eq!(greeting.summary_requests, 0);
    assert_eq!(greeting.ordinary_requests, 1);

    // The same size with a short task, an acknowledgement or a long task compacts first.
    for message in [
        "fix the auth bug",
        "run the tests",
        "perfect",
        "great, continue",
        "hi, fix the auth bug",
        "Please refactor the parser module and run the tests.",
    ] {
        let task = run(packets, message).await;
        assert_eq!(task.checkpoints(), 1, "{message:?} must compact");
        assert!(task.summary_requests >= 1, "{message:?}");
    }
    for message in ["thanks!", "你好", "नमस्ते", "good morning"] {
        let social = run(packets, message).await;
        assert_eq!(social.checkpoints(), 0, "{message:?} must defer");
    }
}

#[tokio::test]
async fn deferred_greeting_that_reads_a_large_file_compacts_on_the_next_turn_without_failing() {
    let packets = packets_over_threshold().await;
    let outcome = run_script(
        packets,
        "hi",
        vec![
            common::tool_call_body(
                "read",
                "read_file",
                serde_json::json!({"file_path": "big.txt"}),
            ),
            common::final_text_body("The file is read."),
        ],
    )
    .await;
    // The first request is deferred; the large observation then pushes the second over the
    // budget, which compacts normally instead of failing the hard-budget check.
    let budgets = outcome
        .events
        .iter()
        .filter(|event| matches!(event, RunEvent::ContextBudget(_)))
        .count();
    assert!(budgets >= 2);
    assert_eq!(outcome.checkpoints(), 1);
    assert!(!outcome
        .events
        .iter()
        .any(|event| matches!(event, RunEvent::Failed { .. })));
}

#[tokio::test]
async fn short_reply_never_exceeds_the_hard_budget() {
    // Far above the usable budget: the flag cannot postpone the checkpoint.
    let outcome = run(200, "thanks!").await;
    assert!(outcome.checkpoints() >= 1);
    // The budget that triggered the checkpoint is over the limit by design; the request
    // actually sent after it must fit.
    let triggering = outcome.first_budget();
    assert!(triggering.used_tokens > triggering.usable_tokens);
    let sent = outcome.budget_after_checkpoint();
    assert!(sent.used_tokens <= sent.usable_tokens);
    // The constructed checkpoint request asks for the four sections, parked work and reasons.
    let system = &outcome.summary_systems[0];
    for expected in [
        "Goal/constraints:, Unresolved work:, Parked work:, Next action:",
        "if the user continues this task",
        "every decision with its reason",
        "scan the whole span",
        "paused but not finished",
        // From compaction-checkpoint.md, the shared system prompt of every checkpoint request.
        "Read the whole span of records in order before writing",
        "decision together with its reason",
        "keep every other topic or task that was paused but not finished as parked work",
    ] {
        assert!(
            system.contains(expected),
            "missing {expected:?} in {system}"
        );
    }
    // The installed checkpoint reaches the next provider request with the resume guard.
    let request = outcome.ordinary_bodies.last().unwrap();
    assert!(
        request.contains("mention or resume this work only when the user"),
        "checkpoint envelope missing from the ordinary request"
    );
}

#[tokio::test]
async fn small_context_never_compacts_for_a_short_reply() {
    let outcome = run(3, "你好").await;
    assert_eq!(outcome.checkpoints(), 0);
    assert_eq!(outcome.summary_requests, 0);
}

#[test]
fn short_reply_needs_few_characters_and_no_attachments() {
    assert!(is_short_reply("hi", false));
    assert!(is_short_reply("  谢谢！ ", false));
    assert!(is_short_reply(&"thanks ".repeat(28), false));
    assert!(!is_short_reply(&"thanks ".repeat(29), false));
    assert!(!is_short_reply("fix the auth bug", false));
    assert!(!is_short_reply("perfect", false));
    assert!(!is_short_reply("hi", true));
    assert!(!is_short_reply("   ", false));
    // Length counts characters, not bytes.
    assert!(is_short_reply(&"谢谢".repeat(100), false));
    assert!(!is_short_reply(&"谢谢".repeat(101), false));
}
