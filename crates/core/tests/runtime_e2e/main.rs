//! Runtime integration tests: scripted mock-LLM multi-turn runs.
//!
//! No real network: the LLM is a wiremock script. The only live test is
//! `live_go_chat_completions_run`, marked `#[ignore]` behind
//! `THEMIS_GO_API_KEY` + `THEMIS_GO_MODEL`.

#[path = "../common/mod.rs"]
mod common;

use std::sync::{atomic::AtomicBool, Arc};

use serde_json::json;
use themis_core::providers::{resolve, ProviderConfig, ProviderKind};
use themis_core::runtime::{run_task, run_task_with_policy, ApprovalDecision, RunEvent, RunPolicy};
use themis_core::tools::{boxed_tools, AllowAllHook, ApprovalHook, DenyAllHook};
use wiremock::MockServer;

fn has_finished(events: &[RunEvent]) -> bool {
    events
        .iter()
        .any(|event| matches!(event, RunEvent::Finished { .. }))
}

mod budget_and_notes;
mod checkpoints_and_reads;
mod evidence_and_runs;
