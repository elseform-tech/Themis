use super::*;
use crate::tools::{is_destructive_variant, shell_risk, ToolAction};
use proptest::prelude::*;
use ThemisCall::*;

fn argv(words: &[&str]) -> Vec<String> {
    words.iter().map(|word| (*word).to_owned()).collect()
}

/// The owner-approved table (2026-11-07), one example per row and shape the CLI accepts.
const TABLE: &[(&[&str], ThemisCall)] = &[
    (&["status"], Read),
    (&["doctor", "--json"], Read),
    (&["doctor", "--deep"], Read),
    (&["usage", "--json"], Read),
    (&["models"], Read),
    (&["permissions", "--json"], Read),
    (&["auth", "status"], Read),
    (&["--help"], Read),
    (&["plugin", "list"], Read),
    (&["plugin", "show", "demo"], Read),
    (&["plugin", "export", "demo"], Read),
    (&["plugin", "updates"], Read),
    (&["plugin", "inspect", "./pkg"], Read),
    (
        &["plugin", "inspect", "https://example.com/x.git", "x"],
        Read,
    ),
    (&["plugin", "marketplace", "list"], Read),
    (&["plugin", "marketplace", "browse", "official"], Read),
    (&["plugin", "marketplace", "preview", "owner/repo"], Read),
    (&["plugin", "marketplace", "changes", "official"], Read),
    (&["plugin", "marketplace", "check", "official"], Read),
    (&["skill", "list"], Read),
    (&["hook", "list", "demo"], Read),
    (&["mcp", "show", "demo"], Read),
    (&["call", "ping"], Read),
    (&["call", "get_thread_tasks", r#"{"threadId":"t"}"#], Read),
    (&["call", "plugin_action", r#"{"action":"list"}"#], Read),
    (&["plugin", "create", "demo"], Write),
    (&["plugin", "save", "spec.json"], Write),
    (&["plugin", "edit", "demo"], Write),
    (&["plugin", "import", "./pkg"], Write),
    (&["skill", "save", "demo", "skill.json"], Write),
    (&["hook", "add", "demo", "guard", "hook.json"], Write),
    (&["mcp", "update", "demo", "docs", "server.json"], Write),
    (&["plugin", "disable", "demo"], Write),
    (&["hook", "disable", "demo", "guard"], Write),
    (&["skill", "enable", "demo", "notes"], Write),
    (&["plugin", "marketplace", "enable", "official"], Write),
    (
        &[
            "call",
            "plugin_action",
            r#"{"action":"save","spec":{"name":"x"}}"#,
        ],
        Write,
    ),
    (&["plugin", "install", "ralph@official"], Network),
    (&["plugin", "update", "demo"], Network),
    (&["plugin", "import", "https://example.com/x.git"], Network),
    (&["skill", "import", "./pkg", "--ref", "main"], Network),
    (&["plugin", "marketplace", "add", "owner/repo"], Network),
    (&["plugin", "marketplace", "refresh", "official"], Network),
    (&["mcp", "test", "demo", "docs"], Execute),
    (&["hook", "test", "demo", "guard"], Execute),
    (&["skill", "verify", "SKILL.md"], Execute),
    (&["plugin", "delete", "demo"], Destructive),
    (&["plugin", "uninstall", "demo"], Destructive),
    (&["hook", "delete", "demo", "guard"], Destructive),
    (&["mcp", "remove", "demo", "docs"], Destructive),
    (&["skill", "delete", "demo", "notes"], Destructive),
    (
        &["plugin", "marketplace", "remove", "official"],
        Destructive,
    ),
    (
        &["call", "plugin_action", r#"{"action":"delete","name":"x"}"#],
        Destructive,
    ),
    (&["hook", "enable", "demo", "guard"], SwitchOn),
    (&["mcp", "enable", "demo", "docs"], SwitchOn),
    (&["plugin", "enable", "demo"], SwitchOn),
    (
        &[
            "call",
            "plugin_action",
            r#"{"action":"set_component_enabled","kind":"hook","enabled":true}"#,
        ],
        SwitchOn,
    ),
    (
        &["call", "plugin_action", r#"{"action":"enable","name":"x"}"#],
        SwitchOn,
    ),
    // Unknown or unreadable: as strict as they could be.
    (&["hook", "enabel", "demo", "guard"], SwitchOn),
    (&["plugin", "frobnicate"], SwitchOn),
    (&["mcp", "enable", "demo"], SwitchOn),
    (&["plugin", "list", "--force"], SwitchOn),
    (&["plugin", "list", "--project"], SwitchOn),
    (&["call", "plugin_action", "-"], SwitchOn),
    (&["call", "plugin_action", "{not json"], SwitchOn),
    (
        &["call", "plugin_action", r#"{"action":"whatever"}"#],
        SwitchOn,
    ),
    (&["call", "approve_action", "{}"], Execute),
    (&["server", "stop"], Execute),
    (&["cli", "install"], Execute),
    (&["thread", "send", "t", "hi"], Execute),
    (&["--data-dir"], Execute),
];

/// How strict a risk is: a higher rank never asks less.
fn rank(risk: RiskLevel) -> u8 {
    match risk {
        RiskLevel::Read => 0,
        RiskLevel::Write | RiskLevel::Network => 1,
        RiskLevel::Execute => 2,
        RiskLevel::Destructive => 3,
    }
}

#[test]
fn every_row_of_the_table_is_classified_as_approved() {
    for (words, expected) in TABLE {
        let command: Vec<String> = std::iter::once("themis")
            .chain(words.iter().copied())
            .map(str::to_owned)
            .collect();
        assert_eq!(classify(&command), Some(*expected), "{words:?}");
        assert_eq!(shell_risk(&command), expected.risk(), "{words:?}");
        assert_eq!(
            switches_on_code(&command),
            *expected == SwitchOn,
            "{words:?}"
        );
        assert_eq!(
            is_destructive_variant(&command),
            *expected == Destructive,
            "{words:?}"
        );
    }
}

#[test]
fn options_and_the_data_dir_do_not_change_a_call() {
    let cases: &[(&[&str], ThemisCall)] = &[
        (&["themis", "--data-dir", "/x", "plugin", "list"], Read),
        (
            &[
                "themis",
                "plugin",
                "--project",
                "/p",
                "list",
                "--scope",
                "local",
            ],
            Read,
        ),
        (
            &[
                "themis", "hook", "enable", "--scope", "global", "demo", "guard",
            ],
            SwitchOn,
        ),
        (
            &["themis", "plugin", "--allow-partial", "import", "./pkg"],
            Write,
        ),
        (
            &[
                "/Applications/Themis.app/Contents/MacOS/themis-desktop",
                "--themis-cli",
                "hook",
                "enable",
                "p",
                "h",
            ],
            SwitchOn,
        ),
        (&["THEMIS.EXE", "plugin", "delete", "x"], Destructive),
    ];
    for (command, expected) in cases {
        assert_eq!(classify(&argv(command)), Some(*expected), "{command:?}");
    }
    assert_eq!(
        classify(&argv(&["themis-desktop", "plugin", "list"])),
        None,
        "the app, not the CLI"
    );
    assert_eq!(classify(&argv(&["git", "status"])), None);
}

#[test]
fn only_a_bare_themis_is_ever_lowered() {
    // A `themis` the agent could have written is a program like any other.
    for program in ["./themis", "/tmp/themis", "bin/themis"] {
        let command = argv(&[program, "plugin", "list"]);
        assert_eq!(direct_risk(&command), None, "{program}");
        assert_eq!(shell_risk(&command), RiskLevel::Execute, "{program}");
        // Raising still sees it.
        assert!(switches_on_code(&argv(&[
            program, "hook", "enable", "p", "h"
        ])));
        assert!(is_destructive_variant(&argv(&[
            program, "plugin", "delete", "p"
        ])));
    }
    assert_eq!(
        direct_risk(&argv(&["themis", "status"])),
        Some(RiskLevel::Read)
    );
}

#[test]
fn wrappers_never_lower_a_call() {
    let wrapped: &[&[&str]] = &[
        &["sh", "-c", "themis plugin list"],
        &["bash", "-lc", "themis plugin list | head -5"],
        &["env", "A=1", "themis", "plugin", "list"],
        &["xargs", "themis", "plugin", "show"],
        &["timeout", "5", "themis", "status"],
    ];
    for command in wrapped {
        assert_eq!(
            shell_risk(&argv(command)),
            RiskLevel::Execute,
            "{command:?}"
        );
    }
    let enabling: &[&[&str]] = &[
        &[
            "sh",
            "-c",
            "themis plugin list && themis hook enable demo guard",
        ],
        &["env", "X=1", "themis", "hook", "enable", "demo", "guard"],
        &[
            "sudo", "-u", "me", "themis", "mcp", "enable", "demo", "docs",
        ],
        &["xargs", "themis", "hook", "enable"],
        &[
            "find", ".", "-exec", "themis", "hook", "enable", "demo", "{}", ";",
        ],
        &["sh", "-c", "echo $(themis plugin enable demo)"],
        // A script that cannot be read but names `themis`.
        &["sh", "-c", "themis hook enable \"$(cat name)\""],
    ];
    for command in enabling {
        assert!(switches_on_code(&argv(command)), "{command:?}");
    }
    let removing: &[&[&str]] = &[
        &["sh", "-c", "cd x; themis plugin uninstall demo"],
        &[
            "env",
            "THEMIS_X=1",
            "themis",
            "hook",
            "delete",
            "demo",
            "guard",
        ],
    ];
    for command in removing {
        assert_eq!(
            shell_risk(&argv(command)),
            RiskLevel::Destructive,
            "{command:?}"
        );
    }
    assert!(!switches_on_code(&argv(&[
        "sh",
        "-c",
        "cargo test && themis plugin list"
    ])));
    assert!(!switches_on_code(&argv(&["grep", "-r", "hook", "."])));
}

#[test]
fn the_read_methods_are_only_reads() {
    for method in AGENT_READ_METHODS {
        assert!(
            method.starts_with("get_")
                || method.starts_with("list_")
                || matches!(
                    *method,
                    "ping" | "query_diagnostic_logs" | "mcp_config_status" | "resource_usage"
                ),
            "{method}"
        );
    }
}

fn grantable(command: &[String]) -> bool {
    ToolAction {
        display: None,
        tool: "shell".into(),
        summary: String::new(),
        risk: shell_risk(command),
        command: command.to_vec(),
    }
    .grantable()
}

#[test]
fn switching_on_and_removing_are_never_grantable() {
    assert!(grantable(&argv(&["themis", "plugin", "create", "demo"])));
    assert!(grantable(&argv(&[
        "themis",
        "hook",
        "add",
        "demo",
        "guard",
        "hook.json"
    ])));
    for command in [
        &["themis", "hook", "enable", "demo", "guard"][..],
        &["themis", "mcp", "enable", "demo", "docs"],
        &["themis", "plugin", "enable", "demo"],
        &["themis", "plugin", "uninstall", "demo"],
        &["themis", "hook", "delete", "demo", "guard"],
        &["sh", "-c", "themis hook enable demo guard"],
        &["themis", "hook", "unknown-word"],
    ] {
        assert!(!grantable(&argv(command)), "{command:?}");
    }
}

/// A `themis` call from the table, optionally with options mixed in.
fn table_call() -> impl Strategy<Value = (Vec<String>, ThemisCall)> {
    (0..TABLE.len(), any::<bool>(), any::<bool>()).prop_map(|(row, data_dir, scope)| {
        let (words, expected) = TABLE[row];
        let mut command = vec!["themis".to_owned()];
        if data_dir {
            command.extend(argv(&["--data-dir", "/tmp/d"]));
        }
        command.extend(argv(words));
        let integration = matches!(words.first(), Some(&("plugin" | "skill" | "mcp" | "hook")));
        if scope && integration {
            command.extend(argv(&["--scope", "global"]));
        }
        (command, expected)
    })
}

/// Ways to run a command inside something else.
fn wrap(command: &[String], how: u8, other: &str) -> Vec<String> {
    let script = command
        .iter()
        .map(|word| format!("'{}'", word.replace('\'', r"'\''")))
        .collect::<Vec<_>>()
        .join(" ");
    match how % 7 {
        0 => argv(&["sh", "-c", &script]),
        1 => argv(&["bash", "-lc", &format!("{other} && {script}")]),
        2 => argv(&["sh", "-c", &format!("{script} | {other}")]),
        3 => [argv(&["env", "THEMIS_X=1"]), command.to_vec()].concat(),
        4 => [argv(&["xargs"]), command.to_vec()].concat(),
        5 => [argv(&["timeout", "30"]), command.to_vec()].concat(),
        _ => argv(&["zsh", "-c", &format!("{other}; {script}")]),
    }
}

proptest! {
    /// A call is never asked about less than the table says, bare or wrapped, and switching
    /// something on is always caught.
    #[test]
    fn the_risk_is_never_lower_than_the_table(
        (command, expected) in table_call(),
        how in any::<u8>(),
        other in "(ls|cargo test|echo done|git status)",
    ) {
        let bare = shell_risk(&command);
        prop_assert!(rank(bare) >= rank(expected.risk()), "{command:?}: {bare:?}");
        prop_assert_eq!(switches_on_code(&command), expected == SwitchOn);
        let wrapped = wrap(&command, how, &other);
        let risk = shell_risk(&wrapped);
        // A wrapper is a program like any other: never a read, never below the call inside.
        prop_assert!(rank(risk) >= rank(expected.risk()).max(rank(RiskLevel::Execute)), "{wrapped:?}: {risk:?}");
        if expected == SwitchOn {
            prop_assert!(switches_on_code(&wrapped), "{wrapped:?}");
            prop_assert!(!grantable(&wrapped), "{wrapped:?}");
        }
        if expected == Destructive {
            prop_assert!(is_destructive_variant(&wrapped), "{wrapped:?}");
        }
    }

    /// A verb the CLI does not know, under an integration command, is as strict as switching
    /// something on.
    #[test]
    fn unknown_integration_verbs_are_strict(
        kind in "(plugin|skill|mcp|hook)",
        verb in "[a-z]{1,9}",
        rest in proptest::collection::vec("[a-z]{1,8}", 0..3),
    ) {
        const KNOWN: &[&str] = &[
            "list", "show", "export", "updates", "inspect", "import", "create", "save", "edit",
            "disable", "install", "update", "delete", "uninstall", "marketplace", "verify",
            "enable", "add", "remove", "test",
        ];
        prop_assume!(!KNOWN.contains(&verb.as_str()));
        let mut command = argv(&["themis", &kind, &verb]);
        command.extend(rest);
        prop_assert_eq!(classify(&command), Some(SwitchOn), "{:?}", command);
        prop_assert!(switches_on_code(&command));
    }

    /// Whatever words a model sends, classifying them never panics.
    #[test]
    fn arbitrary_words_never_panic(words in proptest::collection::vec("\\PC{0,12}", 0..6)) {
        let mut command = vec!["themis".to_owned()];
        command.extend(words);
        let _ = switches_on_code(&command);
        let _ = is_destructive_variant(&command);
        let _ = shell_risk(&command);
    }
}
