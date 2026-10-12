//! Checkpoint history and tool output are untrusted: restoring the task ledger
//! must stay within budget, keep its framing, and emit only JSON lines.
#![no_main]
use libfuzzer_sys::{arbitrary, fuzz_target};

#[derive(Debug, arbitrary::Arbitrary)]
struct Input<'a> {
    history: &'a str,
    tool: &'a str,
    arguments: &'a str,
    output: &'a str,
    total: u16,
    limit: u8,
}

fuzz_target!(|input: Input| {
    use themis_core::runtime::fuzzing::*;
    let total = usize::from(input.total);
    let ledger = restore_ledger(input.history, input.tool, input.arguments, input.output, total);
    assert!(ledger.len() <= total);
    if !ledger.is_empty() {
        let (start, end) = LEDGER_MARKERS;
        let body = ledger.strip_prefix(start).and_then(|rest| rest.strip_suffix(end)).expect("framed ledger");
        for line in body.lines() {
            assert!(serde_json::from_str::<serde_json::Value>(line).is_ok_and(|value| value.is_object()), "{line}");
        }
    }
    let (prefix, truncated) = char_prefix(input.history, usize::from(input.limit));
    assert!(input.history.starts_with(&prefix));
    assert_eq!(truncated, input.history.chars().count() > usize::from(input.limit));
});
