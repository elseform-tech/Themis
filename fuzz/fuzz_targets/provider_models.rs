//! A configured provider's `/models` response and the `providers` block of a user
//! `runtime.jsonc` are untrusted: parsing never panics, kept model ids are unique and usable,
//! and a resolved provider never carries a literal key or a reserved id.
#![no_main]
use libfuzzer_sys::fuzz_target;
use themis_core::configuration::{resolve, valid_model_id};
use themis_core::providers::parse_model_ids;

fuzz_target!(|data: &[u8]| {
    let text = String::from_utf8_lossy(data);
    if let Ok(ids) = parse_model_ids(&text) {
        let mut seen = std::collections::HashSet::new();
        for id in &ids {
            assert!(valid_model_id(id));
            assert!(seen.insert(id.clone()));
        }
    }
    let file = format!(r#"{{"providers":{text}}}"#);
    if let Ok(resolved) = resolve(Some(&file), None, None) {
        for provider in &resolved.config.providers {
            assert!(provider.id != "go" && provider.id != "opencode-go");
            assert!(provider.secret.starts_with("${") && provider.secret.ends_with('}'));
        }
    }
});
