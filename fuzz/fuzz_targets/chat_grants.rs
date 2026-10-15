//! A chat's grant file is untrusted text: parsing must never panic, and any
//! accepted entry must be valid, never destructive and never match a
//! destructive action.
#![no_main]
use libfuzzer_sys::fuzz_target;
use themis_core::chat_grants::parse_grants;
use themis_core::tools::RiskLevel;

fuzz_target!(|data: &[u8]| {
    let text = String::from_utf8_lossy(data);
    if let Ok(entries) = parse_grants(&text) {
        for entry in &entries {
            assert!(entry.validate().is_ok());
            assert_ne!(entry.risk, "destructive");
            assert!(!themis_core::tools::is_destructive_variant(&entry.command));
        }
        // Whatever parsed, a destructive action stays unmatched.
        let forced = themis_core::tools::ToolAction {
            display: None,
            command: ["git", "push", "--force"].map(str::to_owned).to_vec(),
            tool: "shell".to_owned(),
            summary: String::new(),
            risk: RiskLevel::Execute,
        };
        assert!(!forced.grantable());
    }
});
