//! A provider's context-length error is untrusted text: reading the limit it
//! states must never panic, and a limit it keeps stays a plausible window.
#![no_main]
use libfuzzer_sys::fuzz_target;
use themis_core::context_window::{
    implied_limit, stated_limit, MAX_CONTEXT_TOKENS, MIN_LEARNED_WINDOW,
};

fuzz_target!(|text: &str| {
    if let Some(limit) = stated_limit(text) {
        assert!((MIN_LEARNED_WINDOW..=MAX_CONTEXT_TOKENS).contains(&limit));
        assert_eq!(implied_limit(text, usize::MAX), limit);
    }
    let _ = implied_limit(text, 0);
});
