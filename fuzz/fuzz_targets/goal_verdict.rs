//! The goal check's reply is model output: parsing it must never panic, and
//! what it keeps stays bounded, one line and free of control characters.
#![no_main]
use libfuzzer_sys::fuzz_target;

fuzz_target!(|reply: &str| {
    if let Some(verdict) = themis_core::goal::parse_verdict(reply) {
        assert!(verdict.reason.chars().count() <= 400);
        assert!(!verdict.reason.chars().any(char::is_control));
        if let Some(progress) = verdict.progress {
            assert!(!progress.is_empty() && progress.chars().count() <= 120);
        }
    }
    if let Some(goal) = themis_core::goal::parse_goal_command(reply) {
        assert!(goal.chars().count() <= themis_core::goal::MAX_GOAL_CHARS);
    }
    let _ = themis_core::goal::classify_failure(reply);
    let _ = themis_core::goal::turn_number(reply);
});
