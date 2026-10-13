//! Proves the compaction-eval plumbing offline: every committed fixture parses,
//! each chunk is compacted through the real runtime path against a scripted
//! provider, and probes are answered from the post-compaction messages only.
//! Run with `--features eval`; no network and no model quality is measured.

use std::path::PathBuf;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;

use serde_json::Value;
use themis_core::providers::{resolve, ProviderConfig, ProviderKind};
use themis_core::runtime::eval::{
    answer_probe, compact_chunks, parse_transcript, record_count, EvalPolicy,
};
use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

mod common;
use common::final_text_body;

const SUMMARY: &str = "Goal/constraints: keep the fixture facts. Confirmed findings: scripted checkpoint. Completed changes/tests: observed actions only. Unresolved work: finish the request. Next action: continue from recorded state.";

fn cases_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../evals/compaction/cases")
}

/// Summaries for checkpoint requests, a fixed marker answer for probes.
async fn scripted_provider(server: &MockServer) -> Arc<AtomicUsize> {
    let summaries = Arc::new(AtomicUsize::new(0));
    let counter = summaries.clone();
    Mock::given(method("POST"))
        .and(path("/chat/completions"))
        .respond_with(move |request: &wiremock::Request| {
            let body: Value = request.body_json().unwrap();
            let system = body["messages"][0]["content"].as_str().unwrap_or_default();
            if system.contains("You summarize agent context") {
                counter.fetch_add(1, Ordering::SeqCst);
                ResponseTemplate::new(200).set_body_json(final_text_body(SUMMARY))
            } else {
                let last = body["messages"].as_array().and_then(|m| m.last()).unwrap();
                assert!(last["content"].as_str().unwrap().contains("Memory check"));
                assert!(body.get("tools").is_none_or(|tools| tools.is_null()));
                ResponseTemplate::new(200).set_body_json(final_text_body("UNKNOWN"))
            }
        })
        .mount(server)
        .await;
    summaries
}

#[tokio::test]
async fn every_fixture_compacts_and_answers_through_the_runtime() {
    let server = MockServer::start().await;
    let summaries = scripted_provider(&server).await;
    let config = ProviderConfig::new(ProviderKind::Go, "mock-key")
        .with_model("mock-model")
        .with_base_url(server.uri());
    let llm = resolve(&config).await.unwrap();
    let mut cases = 0;
    for entry in std::fs::read_dir(cases_dir()).unwrap() {
        let dir = entry.unwrap().path();
        let case: Value =
            serde_json::from_str(&std::fs::read_to_string(dir.join("case.json")).unwrap()).unwrap();
        let probes: Value =
            serde_json::from_str(&std::fs::read_to_string(dir.join("probes.json")).unwrap())
                .unwrap();
        let transcript =
            parse_transcript(&std::fs::read_to_string(dir.join("transcript.jsonl")).unwrap())
                .unwrap();
        let ends: Vec<usize> = case["chunks"]
            .as_array()
            .unwrap()
            .iter()
            .map(|end| end.as_u64().unwrap() as usize)
            .collect();
        assert_eq!(
            case["records"].as_u64().unwrap() as usize,
            record_count(&transcript)
        );
        assert!(ends.windows(2).all(|pair| pair[0] < pair[1]));
        assert!(*ends.last().unwrap() <= record_count(&transcript));
        let evidence = tempfile::tempdir().unwrap();
        let outcome = compact_chunks(
            &llm,
            &transcript,
            &ends,
            EvalPolicy {
                context_token_budget: 24_000,
                recent_messages: 20,
            },
            evidence.path(),
        )
        .await
        .unwrap();
        assert_eq!(outcome.chunks.len(), ends.len(), "{dir:?}");
        for chunk in &outcome.chunks {
            assert!(
                chunk.installed,
                "{dir:?} chunk {}: {:?}",
                chunk.chunk, chunk.error
            );
            assert!(chunk.tokens_after < chunk.tokens_before, "{dir:?}");
            assert!(chunk.requests >= 1, "{dir:?}: stage count not recorded");
        }
        assert!(outcome
            .messages
            .iter()
            .any(|m| m.content.contains("scripted checkpoint")));
        for probe in probes.as_array().unwrap() {
            let answer = answer_probe(&llm, &outcome.messages, probe["question"].as_str().unwrap())
                .await
                .unwrap();
            assert_eq!(answer, "UNKNOWN");
        }
        cases += 1;
    }
    assert!(
        cases >= 8,
        "expected at least 8 committed cases, found {cases}"
    );
    assert!(summaries.load(Ordering::SeqCst) >= cases);
}

#[test]
fn malformed_transcripts_are_rejected() {
    assert!(parse_transcript("{\"role\":\"robot\",\"content\":\"x\"}").is_err());
    assert!(parse_transcript("not json").is_err());
    assert_eq!(
        record_count(&parse_transcript("{\"role\":\"user\",\"content\":\"hi\"}\n").unwrap()),
        1
    );
}
