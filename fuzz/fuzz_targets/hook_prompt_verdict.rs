//! A `prompt` hook's answer is model output: reading it must never panic, and what it keeps of a
//! reason stays bounded. A verdict of yes comes only from an explicit yes.
#![no_main]
use libfuzzer_sys::fuzz_target;

fuzz_target!(|reply: &str| {
    if let Some(verdict) = themis_core::plugins::hooks::parse_prompt_verdict(reply) {
        if let Some(reason) = verdict.reason {
            assert!(reason.len() <= 1024);
        }
        if verdict.ok {
            assert!(reply.contains("true") || reply.to_ascii_lowercase().contains("approve"));
        }
    }
});
