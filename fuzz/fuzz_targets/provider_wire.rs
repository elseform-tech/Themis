//! Model tool arguments and provider JSON are untrusted: outgoing requests must
//! always carry object arguments, and response/stream converters must not panic.
#![no_main]
use libfuzzer_sys::fuzz_target;
use themis_core::providers::fuzzing::{convert_provider_json, outgoing_bodies};

fuzz_target!(|data: &[u8]| {
    let raw = String::from_utf8_lossy(data);
    for body in outgoing_bodies(&raw) {
        assert!(body.is_object());
    }
    if let Ok(value) = serde_json::from_slice::<serde_json::Value>(data) {
        convert_provider_json(value);
    }
});
