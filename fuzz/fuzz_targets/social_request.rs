//! UI text decides whether anchors are injected and whether a checkpoint may wait:
//! it must never panic, and its outputs stay bounded and consistent.
#![no_main]
use libfuzzer_sys::fuzz_target;

fuzz_target!(|text: &str| {
    let (social, terms) = themis_core::runtime::fuzzing::social_request(text);
    assert!(terms.len() <= 12);
    assert!(terms.windows(2).all(|pair| pair[0] < pair[1]));
    // A word that is not social anywhere makes the message a task.
    let task = format!("{text} | qzxyzzy | {text}");
    assert!(!themis_core::runtime::fuzzing::social_request(&task).0);
    if social && text.chars().count() < 500 {
        assert!(themis_core::runtime::fuzzing::social_request(&format!("{text} {text}")).0);
    }
});
