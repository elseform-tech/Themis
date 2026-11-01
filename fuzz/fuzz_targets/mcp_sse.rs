//! What an MCP server sends on an event stream is untrusted: reading it must never panic, what it
//! keeps stays bounded, and a message address it names stays on its server or is refused.
#![no_main]
use libfuzzer_sys::fuzz_target;
use themis_core::plugins::connections::{sse_endpoint, SseParser};

fuzz_target!(|bytes: &[u8]| {
    let mut parser = SseParser::default();
    let (first, second) = bytes.split_at(bytes.len() / 2);
    for chunk in [first, second] {
        if let Ok(events) = parser.feed(chunk) {
            for event in events {
                assert!(event.data.len() <= 2 * 1024 * 1024);
                if let Ok(target) = sse_endpoint("https://mcp.example.com/sse", &event.data) {
                    assert!(target.starts_with("https://mcp.example.com/"), "{target}");
                }
            }
        }
    }
});
