//! SearXNG responses are untrusted: parsing must never panic, and kept results
//! are unique http(s) URLs within the requested count.
#![no_main]
use libfuzzer_sys::fuzz_target;
use themis_core::tools::web::parse_searxng;

fuzz_target!(|data: &[u8]| {
    let Some((&max, body)) = data.split_first() else {
        return;
    };
    let max = usize::from(max % 24);
    if let Ok(parsed) = parse_searxng(body, max) {
        assert!(parsed.results.len() <= max);
        let mut seen = std::collections::HashSet::new();
        for hit in &parsed.results {
            assert!(hit.url.starts_with("http://") || hit.url.starts_with("https://"));
            assert!(seen.insert(hit.url.clone()));
        }
    }
});
