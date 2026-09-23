//! Structural coverage for diagnostics error recording: every fallible
//! Tauri command in `src/commands.rs` must wrap its handler call in the
//! `record_cmd` macro so its `Err` lands in the diagnostics ring. A command
//! added without the wrapper fails this test (a missing wrapper is a bug).

/// Commands that never fail, and therefore legitimately skip the macro.
/// Keep in sync with the module docs in `src/commands.rs`.
const INFALLIBLE: &[&str] = &[
    "ping",
    "get_settings",
    "get_secret_status",
    "list_skills",
    "list_automations",
    "list_review_items",
    "get_diagnostics",
    "check_for_updates",
];

#[test]
fn every_fallible_command_records_its_errors() {
    let source = include_str!("../src/commands.rs");
    // NOTE: the needle is spelled so this file's own text can never match
    // it — the count below must reflect `src/commands.rs` only.
    let needle = ["record_cmd", "!("].concat();
    let commands = source
        .lines()
        .filter(|line| line.trim() == "#[tauri::command]")
        .count();
    let recorded = source.matches(&needle).count();
    assert!(
        commands > 0 && recorded > 0,
        "expected commands and wrappers in src/commands.rs"
    );
    assert_eq!(
        recorded,
        commands - INFALLIBLE.len(),
        "every fallible command must wrap its handler call so its errors are recorded; \
         wrap the new command or extend INFALLIBLE with a documented reason"
    );
    // Each recorded name must name a real, fallible command function.
    for fragment in source.split(&needle).skip(1) {
        let name = fragment
            .split('"')
            .nth(1)
            .expect("wrapper carries a command-name literal");
        assert!(
            source.contains(&format!("pub async fn {name}(")),
            "recorded name {name} names no command"
        );
        assert!(
            !INFALLIBLE.contains(&name),
            "infallible command {name} needs no wrapper"
        );
    }
    for name in INFALLIBLE {
        assert!(
            source.contains(&format!("pub async fn {name}(")),
            "infallible command {name} missing from src/commands.rs"
        );
    }
}
