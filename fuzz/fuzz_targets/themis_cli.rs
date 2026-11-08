//! Model-written `themis` calls in an agent's shell are untrusted: reading their risk never
//! panics, a call read as a read never also switches something on or removes something, and a
//! wrapper (`sh -c`, `env`, `xargs`) never makes a call a read.
#![no_main]
use libfuzzer_sys::fuzz_target;
use themis_core::tools::themis_cli::{direct_risk, switches_on_code};
use themis_core::tools::{is_destructive_variant, shell_risk, RiskLevel};

fuzz_target!(|data: (u8, Vec<String>)| {
    let (shape, words) = data;
    let mut command = vec!["themis".to_owned()];
    // Steer most inputs into the integration commands the classifier reads closely.
    let kinds = ["plugin", "hook", "mcp", "skill", "call"];
    if let Some(kind) = kinds.get(shape as usize % 8) {
        command.push((*kind).to_owned());
    }
    command.extend(words);
    let risk = shell_risk(&command);
    if direct_risk(&command) == Some(RiskLevel::Read) {
        assert_eq!(risk, RiskLevel::Read);
        assert!(!switches_on_code(&command));
        assert!(!is_destructive_variant(&command));
    }
    let script = command
        .iter()
        .map(|word| format!("'{}'", word.replace('\'', r"'\''")))
        .collect::<Vec<_>>()
        .join(" ");
    for wrapped in [
        vec!["sh".to_owned(), "-c".to_owned(), script],
        [vec!["env".to_owned(), "A=1".to_owned()], command.clone()].concat(),
        [vec!["xargs".to_owned()], command.clone()].concat(),
    ] {
        assert_ne!(shell_risk(&wrapped), RiskLevel::Read);
        if switches_on_code(&command) {
            assert!(switches_on_code(&wrapped));
        }
    }
});
