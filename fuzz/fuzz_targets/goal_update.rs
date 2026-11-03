//! `update_goal`'s arguments are model output, and a `/goal` message is the person's text:
//! parsing either must never panic, and what is kept stays bounded, one line and free of
//! control characters.
#![no_main]
use libfuzzer_sys::fuzz_target;
use themis_core::goal::{self, GoalUpdate};

fuzz_target!(|text: &str| {
    if let Ok(args) = serde_json::from_str::<serde_json::Value>(text) {
        match goal::parse_update(&args) {
            Ok(GoalUpdate::Complete(evidence)) => {
                assert!(!evidence.is_empty() && evidence.len() <= goal::MAX_EVIDENCE);
                for entry in evidence {
                    assert!(entry.requirement.chars().count() <= goal::MAX_REQUIREMENT_CHARS);
                    assert!(entry.proof.chars().count() <= goal::MAX_PROOF_CHARS);
                    assert!(!entry.proof.chars().any(char::is_control));
                }
            }
            Ok(GoalUpdate::Blocked(reason)) => {
                assert!(reason.chars().count() <= goal::MAX_BLOCKED_REASON_CHARS);
            }
            Ok(GoalUpdate::Paused) | Err(_) => {}
        }
    }
    if let Some(objective) = goal::parse_goal_command(text) {
        assert!(objective.chars().count() <= goal::MAX_GOAL_CHARS);
    }
    let _ = goal::classify_failure(text);
    let _ = goal::turn_number(text);
});
