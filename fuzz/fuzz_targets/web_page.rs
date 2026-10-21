//! `fetch_url` bodies are untrusted: turning any bytes with any content type
//! into page text must never panic, and refusals are plain messages.
#![no_main]
use libfuzzer_sys::fuzz_target;
use themis_core::tools::web::page_text;

const TYPES: [&str; 6] = [
    "text/html",
    "text/html; charset=iso-8859-1",
    "application/json",
    "",
    "application/pdf",
    "text/plain; charset=\"shift_jis\"",
];

fuzz_target!(|data: &[u8]| {
    let Some((&selector, body)) = data.split_first() else {
        return;
    };
    let content_type = TYPES[usize::from(selector) % TYPES.len()];
    match page_text(body, content_type, Some("https://example.com/")) {
        Ok(page) => assert!(!page.title.contains('\n')),
        Err(message) => assert!(!message.is_empty()),
    }
});
