//! Git diff output is external text: parsing must never panic, and every
//! parsed hunk line must come from the input.
#![no_main]
use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
    let text = String::from_utf8_lossy(data);
    for file in themis_desktop::diff::parse_diff(&text) {
        for hunk in &file.hunks {
            for line in &hunk.lines {
                assert!(text.contains(line.text.as_str()));
            }
        }
    }
});
