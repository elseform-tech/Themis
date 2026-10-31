//! A plugin's `agents/*.md` is third-party text: reading it must never panic, and an accepted
//! profile must be valid, bounded and name only tools a subagent may have.
#![no_main]
use libfuzzer_sys::fuzz_target;
use themis_core::plugins::agents::{parse_profile, READ_ONLY_TOOLS, WRITE_TOOLS};

fuzz_target!(|data: &[u8]| {
    let text = String::from_utf8_lossy(data);
    if let Ok(parsed) = parse_profile(&text) {
        let profile = parsed.profile;
        assert!(themis_core::plugins::safe_name(&profile.name));
        assert!(!profile.instructions.trim().is_empty());
        assert!(profile.description.len() <= 1024);
        assert!(!profile.description.chars().any(char::is_control));
        assert!(profile.dropped_tools.len() <= 32);
        for tool in profile.tools.iter().flatten() {
            assert!(READ_ONLY_TOOLS.contains(&tool.as_str()) || WRITE_TOOLS.contains(&tool.as_str()));
        }
    }
});
