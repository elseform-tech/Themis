use super::*;

#[test]
fn cached_user_grant_cannot_bypass_risk_deny() {
    let policy = ApprovalPolicy {
        default: PolicyAction::Ask,
        rules: vec![ApprovalRule {
            command: None,
            tool: "git".into(),
            action: PolicyAction::Deny,
            risk: Some("destructive".into()),
        }],
        ..Default::default()
    };
    let hook = configured_hook(
        ApprovalMode::Custom,
        policy,
        Arc::new(crate::tools::AllowAllHook),
    );
    let action = |risk| ToolAction {
        display: None,
        command: Vec::new(),
        tool: "git".into(),
        summary: String::new(),
        risk,
    };
    assert_eq!(hook.approve(&action(RiskLevel::Read)), Approval::AllowOnce);
    assert_eq!(hook.approve(&action(RiskLevel::Read)), Approval::AllowOnce);
    assert_eq!(
        hook.approve(&action(RiskLevel::Destructive)),
        Approval::Deny
    );
}

fn shell_action(argv: &[&str]) -> ToolAction {
    ToolAction {
        display: None,
        command: argv.iter().map(|t| t.to_string()).collect(),
        tool: "shell".into(),
        summary: String::new(),
        risk: RiskLevel::Execute,
    }
}

fn decide(rules: &str, argv: &[&str]) -> PolicyAction {
    let policy = resolve(Some(rules), None, None).unwrap().config.approval;
    let mut action = shell_action(argv);
    action.risk = crate::tools::shell_risk(&action.command);
    policy.decision(&action)
}

#[test]
fn sandbox_defaults_layering_and_provenance() {
    let defaults = resolve(None, None, None).unwrap();
    assert!(defaults.config.sandbox.enabled);
    assert_eq!(
        defaults.config.sandbox.network,
        crate::sandbox::NetworkPolicy::Allow
    );
    assert_eq!(defaults.provenance["sandbox.enabled"], "default");
    let user = r#"{"sandbox":{"writable_paths":["~/.cargo","/opt/cache"]}}"#;
    let project = r#"{"sandbox":{"network":"block","writable_paths":["~/.cargo"]}}"#;
    let resolved = resolve(Some(user), Some(project), None).unwrap();
    assert_eq!(
        resolved.config.sandbox.network,
        crate::sandbox::NetworkPolicy::Block
    );
    assert_eq!(resolved.config.sandbox.writable_paths, vec!["~/.cargo"]);
    assert_eq!(resolved.provenance["sandbox.network"], "project");
    assert_eq!(resolved.provenance["sandbox.writable_paths"], "project");
    for (layer, needle) in [
        (
            r#"{"sandbox":{"enabled":false}}"#,
            "cannot disable the sandbox",
        ),
        (
            r#"{"sandbox":{"writable_paths":["/etc"]}}"#,
            "cannot add sandbox.writable_paths",
        ),
        (
            r#"{"sandbox":{"backend":"landlock"}}"#,
            "cannot change sandbox.backend",
        ),
    ] {
        let error = format!("{:#}", resolve(Some(user), Some(layer), None).unwrap_err());
        assert!(error.contains(needle), "{error}");
        // A run override is held to the same rule.
        let error = format!("{:#}", resolve(Some(user), None, Some(layer)).unwrap_err());
        assert!(error.contains(needle), "{error}");
    }
    let blocked = r#"{"sandbox":{"network":"block"}}"#;
    assert!(resolve(
        Some(blocked),
        Some(r#"{"sandbox":{"network":"allow"}}"#),
        None
    )
    .is_err());
    // The user file may do anything valid, including turning it off.
    assert!(
        !resolve(Some(r#"{"sandbox":{"enabled":false}}"#), None, None)
            .unwrap()
            .config
            .sandbox
            .enabled
    );
    assert!(resolve(
        Some(r#"{"sandbox":{"writable_paths":["relative"]}}"#),
        None,
        None
    )
    .is_err());
    assert!(resolve(Some(r#"{"sandbox":{"mystery":1}}"#), None, None).is_err());
    let other_os = if cfg!(target_os = "macos") {
        "bubblewrap"
    } else {
        "seatbelt"
    };
    let error = format!(
        "{:#}",
        resolve(
            Some(&format!(r#"{{"sandbox":{{"backend":"{other_os}"}}}}"#)),
            None,
            None
        )
        .unwrap_err()
    );
    assert!(error.contains("not supported"), "{error}");
    assert!(schema()["properties"]["sandbox"]["properties"]["backend"]["enum"].is_array());
}

#[test]
fn rule_sources_follow_the_layers() {
    let resolved = resolve(None, None, None).unwrap();
    assert_eq!(resolved.rule_sources, vec!["default"]);
    let user = r#"{"approval":{"rules":[{"tool":"shell","command":["cargo","test"],"action":"allow"},{"tool":"*","risk":"read","action":"allow"}]}}"#;
    let project = r#"{"approval":{"rules":[{"tool":"git","action":"deny"},{"tool":"shell","command":["rm"],"action":"deny"}]}}"#;
    let resolved = resolve(Some(user), Some(project), None).unwrap();
    assert_eq!(
        resolved.rule_sources,
        vec!["project", "project", "user", "user"]
    );
    assert_eq!(resolved.config.approval.rules.len(), 4);
    let resolved = resolve(None, Some(project), None).unwrap();
    assert_eq!(resolved.rule_sources, vec!["project", "project", "default"]);
    let deny_all = r#"{"approval":{"default":"deny"}}"#;
    let resolved = resolve(None, Some(deny_all), None).unwrap();
    assert_eq!(resolved.rule_sources, vec!["project", "default"]);
}

#[test]
fn configuration_file_problems_carry_their_line() {
    assert_eq!(
        check_file("{\n  // fine\n  \"logging\": {\"level\": \"info\"},\n}"),
        None
    );
    let issue =
        check_file("{\n  \"sandbox\": {\n    \"enabled\": tru, \"network\": \"block\"\n  }\n}")
            .unwrap();
    assert_eq!(issue.line, Some(3), "{issue:?}");
    let issue = check_file("{\n  \"sandbox\": {\n    \"netwrk\": \"block\"\n  }\n}").unwrap();
    assert_eq!(issue.line, Some(3), "{issue:?}");
    assert!(issue.message.contains("netwrk"), "{issue:?}");
    assert!(!issue.message.contains(" at line "), "{issue:?}");
    let issue = check_file("[]").unwrap();
    assert_eq!(issue.line, Some(1));
    assert!(check_file("/* open").is_some());
    let text = "{\n  \"sandbox\": {\n    \"backend\": \"landlock\"\n  }\n}";
    assert_eq!(
        locate_error_line(text, "sandbox.backend \"landlock\" is not supported"),
        Some(3)
    );
    assert_eq!(locate_error_line(text, "no field here"), None);
}

#[test]
fn deny_rules_hold_across_shell_and_the_git_tool() {
    let action = |tool: &str, command: &[&str]| ToolAction {
        display: None,
        command: command.iter().map(|word| (*word).to_owned()).collect(),
        tool: tool.into(),
        summary: String::new(),
        risk: RiskLevel::Execute,
    };
    let policy = |rule: &str| ApprovalPolicy {
        default: PolicyAction::Allow,
        rules: vec![serde_json::from_str(rule).unwrap()],
        ..Default::default()
    };
    let shell_rule = policy(r#"{"tool":"shell","command":["git","push"],"action":"deny"}"#);
    let git_rule = policy(r#"{"tool":"git","command":["git","push"],"action":"deny"}"#);
    for rules in [&shell_rule, &git_rule] {
        for denied in [
            action("shell", &["git", "-C", ".", "push"]),
            action(
                "shell",
                &["git", "-c", "a=b", "--git-dir", "x", "push", "-f"],
            ),
            action("git", &["git", "push"]),
            action("git", &["git", "-C", ".", "push"]),
            action("shell", &["git", "push"]),
            action("shell", &["sh", "-c", "x && git push"]),
        ] {
            assert_eq!(
                rules.yolo_decision(&denied),
                PolicyAction::Deny,
                "{denied:?}"
            );
        }
        let status = action("git", &["git", "status"]);
        assert_eq!(rules.yolo_decision(&status), PolicyAction::Allow);
    }
    // A whole-tool rule on `shell` takes the `git` tool too; one on `git` stays its own.
    let all_shell = policy(r#"{"tool":"shell","action":"deny"}"#);
    assert_eq!(
        all_shell.yolo_decision(&action("git", &["git", "status"])),
        PolicyAction::Deny
    );
    let all_git = policy(r#"{"tool":"git","action":"deny"}"#);
    assert_eq!(
        all_git.yolo_decision(&action("shell", &["ls"])),
        PolicyAction::Allow
    );
    // Only deny crosses: an allow on `shell` never allows the `git` tool.
    let allow = ApprovalPolicy {
        default: PolicyAction::Ask,
        rules: vec![serde_json::from_str(
            r#"{"tool":"shell","command":["git","push"],"action":"allow"}"#,
        )
        .unwrap()],
        ..Default::default()
    };
    assert_eq!(
        allow.decision(&action("git", &["git", "push"])),
        PolicyAction::Ask
    );
}

#[test]
fn deny_rules_match_equivalent_flag_spellings() {
    let deny = r#"{"approval":{"default":"allow","rules":[
            {"tool":"shell","command":["git","push","--force"],"action":"deny"},
            {"tool":"shell","command":["rm","-rf"],"action":"deny"}]}}"#;
    for denied in [
        vec!["git", "push", "--force"],
        vec!["git", "push", "-f", "origin", "main"],
        vec!["git", "push", "origin", "main", "--force"],
        vec!["/usr/bin/git", "push", "--force=yes"],
        vec!["rm", "-rf", "build"],
        vec!["rm", "-fr", "build"],
        vec!["rm", "-r", "-f", "build"],
        vec!["rm", "--recursive", "--force", "build"],
        vec!["rm", "-Rf", "build"],
    ] {
        assert_eq!(decide(deny, &denied), PolicyAction::Deny, "{denied:?}");
    }
    for allowed in [
        vec!["git", "push", "origin", "main"],
        vec!["git", "pull", "--force"],
        vec!["rm", "-r", "build"],
        vec!["rm", "file", "--", "-rf"],
    ] {
        assert_ne!(decide(deny, &allowed), PolicyAction::Deny, "{allowed:?}");
    }
}

#[test]
fn broad_shell_allow_does_not_cover_destructive_variants() {
    let broad = r#"{"approval":{"rules":[{"tool":"shell","action":"allow"}]}}"#;
    assert_eq!(decide(broad, &["cargo", "test"]), PolicyAction::Allow);
    for variant in [
        vec!["git", "push", "--force"],
        vec!["git", "reset", "--hard"],
        vec!["git", "clean", "-fdx"],
        vec!["rm", "-rf", "target"],
        vec!["cargo", "publish"],
    ] {
        assert_eq!(decide(broad, &variant), PolicyAction::Ask, "{variant:?}");
    }
    let wildcard = r#"{"approval":{"rules":[{"tool":"*","action":"allow"}]}}"#;
    assert_eq!(decide(wildcard, &["rm", "-rf", "x"]), PolicyAction::Ask);
    let explicit =
        r#"{"approval":{"rules":[{"tool":"shell","risk":"destructive","action":"allow"}]}}"#;
    assert_eq!(decide(explicit, &["rm", "-rf", "x"]), PolicyAction::Allow);
    let execute = r#"{"approval":{"rules":[{"tool":"shell","risk":"execute","action":"allow"}]}}"#;
    assert_eq!(
        decide(execute, &["git", "reset", "--hard"]),
        PolicyAction::Ask
    );
    assert_eq!(decide(execute, &["git", "status"]), PolicyAction::Allow);
}

#[test]
fn shell_destructive_variants_carry_the_destructive_risk() {
    for variant in [
        vec!["git", "push", "--force"],
        vec!["git", "reset", "--hard"],
        vec!["git", "clean", "-fdx"],
        vec!["rm", "-rf", "/tmp/x"],
        vec!["rm", "--recursive", "x"],
    ] {
        assert_eq!(
            crate::tools::shell_risk(&argv(&variant)),
            RiskLevel::Destructive,
            "{variant:?}"
        );
    }
    for ordinary in [
        vec!["rm", "file"],
        vec!["rm", "-f", "file"],
        vec!["ls", "-R"],
    ] {
        assert_eq!(
            crate::tools::shell_risk(&argv(&ordinary)),
            RiskLevel::Execute,
            "{ordinary:?}"
        );
    }
}

#[test]
fn publish_words_as_operands_of_other_subcommands_are_harmless() {
    for harmless in [
        vec!["cargo", "test", "publish"],
        vec!["cargo", "test", "--", "publish"],
        vec!["cargo", "build", "-p", "publish"],
        vec!["cargo", "run", "--bin", "yank"],
        vec!["npm", "test", "publish"],
        vec!["pnpm", "install", "publisher"],
        vec!["yarn", "add", "deprecated-pkg"],
    ] {
        assert!(!is_destructive_variant(&argv(&harmless)), "{harmless:?}");
    }
    for danger in [
        vec!["npm", "run", "publish"],
        vec!["cargo", "publish", "--dry-run"],
        vec!["pnpm", "-r", "publish"],
    ] {
        assert!(is_destructive_variant(&argv(&danger)), "{danger:?}");
    }
}

#[test]
fn command_rules_match_token_prefixes_only() {
    let config = resolve(
        Some(
            r#"{"approval":{"rules":[
                {"tool":"shell","command":["cargo","test"],"action":"allow"},
                {"tool":"shell","command":["echo"],"action":"allow"}]}}"#,
        ),
        Some(r#"{"approval":{"rules":[{"tool":"shell","command":["rm"],"action":"deny"}]}}"#),
        None,
    )
    .unwrap()
    .config;
    let decide = |argv: &[&str]| config.approval.decision(&shell_action(argv));
    assert_eq!(decide(&["cargo", "test", "--lib"]), PolicyAction::Allow);
    assert_eq!(decide(&["echo"]), PolicyAction::Allow);
    assert_eq!(decide(&["cargo", "build"]), PolicyAction::Ask);
    assert_eq!(decide(&["cargo", "testing"]), PolicyAction::Ask);
    assert_eq!(decide(&["cargo"]), PolicyAction::Ask);
    assert_eq!(decide(&["/bin/echo", "x"]), PolicyAction::Ask);
    assert_eq!(decide(&["rm", "-rf", "x"]), PolicyAction::Deny);
    // Deny rules also catch path-qualified spellings of the program...
    assert_eq!(decide(&["/bin/rm", "x"]), PolicyAction::Deny);
    assert_eq!(decide(&["./rm"]), PolicyAction::Deny);
    // ...but allow rules never widen: a local ./echo is not `echo`.
    assert_eq!(decide(&["./echo", "hi"]), PolicyAction::Ask);
    assert_eq!(decide(&["/bin/echo"]), PolicyAction::Ask);
    // Deny rules see through wrappers and shell scripts.
    assert_eq!(decide(&["env", "rm", "x"]), PolicyAction::Deny);
    assert_eq!(decide(&["sh", "-c", "ls && rm x"]), PolicyAction::Deny);
    assert_eq!(
        decide(&["sudo", "-u", "me", "/bin/rm", "x"]),
        PolicyAction::Deny
    );
    // ...but allow rules stay exact: `echo` does not allow a script that echoes.
    assert_eq!(decide(&["sh", "-c", "echo hi"]), PolicyAction::Ask);
    #[cfg(any(target_os = "macos", windows))]
    assert_eq!(decide(&["RM", "x"]), PolicyAction::Deny);
    #[cfg(windows)]
    assert_eq!(decide(&["rm.exe", "x"]), PolicyAction::Deny);
    // A command rule never matches a tool that carries no argv.
    let mut other = shell_action(&[]);
    other.tool = "git".into();
    assert_eq!(config.approval.decision(&other), PolicyAction::Ask);
}

#[test]
fn command_rule_validation() {
    let bad = |rule: &str| {
        resolve(
            Some(&format!(r#"{{"approval":{{"rules":[{rule}]}}}}"#)),
            None,
            None,
        )
        .is_err()
    };
    assert!(!bad(
        r#"{"tool":"shell","command":["ls"],"action":"allow"}"#
    ));
    assert!(bad(r#"{"tool":"shell","command":[],"action":"allow"}"#));
    assert!(bad(r#"{"tool":"shell","command":[""],"action":"allow"}"#));
    assert!(bad(
        r#"{"tool":"git","command":["status"],"action":"allow"}"#
    ));
    assert!(bad(
        r#"{"tool":"shell*","command":["ls"],"action":"allow"}"#
    ));
    assert!(bad(
        r#"{"tool":"shell","command":["a"],"action":"allow","x":1}"#
    ));
    let seventeen = vec!["a"; 17].join("\",\"");
    assert!(bad(&format!(
        r#"{{"tool":"shell","command":["{seventeen}"],"action":"allow"}}"#
    )));
    let long = "a".repeat(257);
    assert!(bad(&format!(
        r#"{{"tool":"shell","command":["{long}"],"action":"allow"}}"#
    )));
    // Project layer may add command denies but not allows.
    assert!(resolve(
        None,
        Some(r#"{"approval":{"rules":[{"tool":"shell","command":["ls"],"action":"allow"}]}}"#),
        None
    )
    .is_err());
}

#[test]
fn grant_key_scopes_shell_to_program_and_subcommand() {
    let key = |argv: &[&str]| shell_action(argv).grant_key();
    assert_eq!(key(&["cargo", "test", "--lib"]), key(&["cargo", "test"]));
    assert_ne!(key(&["cargo", "test"]), key(&["cargo", "publish"]));
    assert_ne!(key(&["cargo", "test"]), key(&["rm", "-rf", "x"]));
    // Flag-first calls are keyed by their full argv.
    assert_ne!(key(&["bash", "-c", "ls"]), key(&["bash", "-c", "rm x"]));
    assert_eq!(key(&["ls"]), key(&["ls"]));
    // Only subcommand-style CLIs share a key across their arguments.
    assert_eq!(key(&["cargo", "test", "--lib"]), key(&["cargo", "test"]));
    assert_ne!(
        key(&["find", ".", "-name", "x"]),
        key(&["find", ".", "-delete"])
    );
    assert_ne!(
        key(&["sudo", "rm", "foo"]),
        key(&["sudo", "rm", "-rf", "/"])
    );
    assert_ne!(key(&["env", "X=1", "rm"]), key(&["env", "X=1", "ls"]));
    assert_ne!(
        key(&["cargo", "run", "--", "a"]),
        key(&["cargo", "run", "--", "b"])
    );
    assert_ne!(key(&["npm", "exec", "a"]), key(&["npm", "exec", "b"]));
    assert_ne!(key(&["ssh", "host", "ls"]), key(&["ssh", "host", "rm"]));
    let mut git = shell_action(&[]);
    git.tool = "git".into();
    assert_eq!(git.grant_key(), "git");
    // The git tool carries `["git", args...]`: subcommand-scoped, but
    // destructive checkout/branch variants get their own key.
    let git_key = |argv: &[&str]| {
        let mut action = shell_action(argv);
        action.tool = "git".into();
        action.grant_key()
    };
    assert_eq!(git_key(&["git", "add", "a"]), git_key(&["git", "add", "b"]));
    assert_eq!(
        git_key(&["git", "checkout", "main"]),
        git_key(&["git", "checkout", "dev"])
    );
    assert_ne!(
        git_key(&["git", "checkout", "main"]),
        git_key(&["git", "checkout", "-f", "main"])
    );
    assert_ne!(
        git_key(&["git", "branch"]),
        git_key(&["git", "branch", "-D", "x"])
    );
    assert_ne!(
        git_key(&["git", "add", "a"]),
        shell_action(&["git", "add", "a"]).grant_key()
    );
}

#[test]
fn destructive_variants_get_their_own_grant_key() {
    let key = |argv: &[&str]| shell_action(argv).grant_key();
    // Harmless siblings still share a key.
    assert_eq!(
        key(&["git", "push"]),
        key(&["git", "push", "origin", "main"])
    );
    assert_eq!(
        key(&["git", "clean", "-n"]),
        key(&["git", "clean", "--dry-run"])
    );
    assert_eq!(key(&["git", "reset"]), key(&["git", "reset", "HEAD"]));
    assert_eq!(key(&["git", "branch", "-d", "x"]), key(&["git", "branch"]));
    assert!(key(&["cargo", "publish", "--dry-run"]).contains("--dry-run"));
    for (safe, danger) in [
        (vec!["git", "push"], vec!["git", "push", "--force"]),
        (
            vec!["git", "push"],
            vec!["git", "push", "-f", "origin", "main"],
        ),
        (
            vec!["git", "push"],
            vec!["git", "push", "--force-with-lease"],
        ),
        (vec!["git", "push"], vec!["git", "push", "--mirror"]),
        (
            vec!["git", "push"],
            vec!["git", "push", "--delete", "origin", "x"],
        ),
        (vec!["git", "push"], vec!["git", "push", "origin", ":x"]),
        (vec!["git", "push"], vec!["git", "push", "origin", "+main"]),
        (vec!["git", "push"], vec!["git", "push", "-fu"]),
        (vec!["git", "clean", "-n"], vec!["git", "clean", "-fdx"]),
        (vec!["git", "clean", "-n"], vec!["git", "clean", "-nd"]),
        (vec!["git", "reset"], vec!["git", "reset", "--hard"]),
        (
            vec!["git", "checkout", "main"],
            vec!["git", "checkout", "-f", "main"],
        ),
        (
            vec!["git", "checkout", "main"],
            vec!["git", "checkout", "--force", "main"],
        ),
        (
            vec!["git", "switch", "main"],
            vec!["git", "switch", "--discard-changes", "main"],
        ),
        (
            vec!["git", "restore", "--staged", "f"],
            vec!["git", "restore", "f"],
        ),
        (vec!["git", "branch"], vec!["git", "branch", "-D", "x"]),
        (
            vec!["git", "branch"],
            vec!["git", "branch", "--delete", "--force", "x"],
        ),
        (vec!["git", "log"], vec!["git", "rebase", "main"]),
        (vec!["npm", "test"], vec!["npm", "publish"]),
        (vec!["pnpm", "test"], vec!["pnpm", "publish", "--tag", "x"]),
        (vec!["yarn", "test"], vec!["yarn", "publish"]),
        (vec!["cargo", "build"], vec!["cargo", "publish"]),
        (vec!["cargo", "build"], vec!["cargo", "yank", "x"]),
    ] {
        assert_ne!(key(&safe), key(&danger), "{safe:?} vs {danger:?}");
        assert!(
            is_destructive_variant(&danger.iter().map(|t| t.to_string()).collect::<Vec<_>>()),
            "{danger:?}"
        );
    }
    // Different destructive calls do not share a key with each other either.
    assert_ne!(
        key(&["git", "push", "--force", "origin", "a"]),
        key(&["git", "push", "--force", "origin", "b"])
    );
    // Global options before the subcommand do not hide the variant.
    assert!(is_destructive_variant(&[
        "git".into(),
        "-C".into(),
        "d".into(),
        "push".into(),
        "-f".into()
    ]));
    assert!(is_destructive_variant(&[
        "/usr/bin/GIT.exe".into(),
        "reset".into(),
        "--hard".into()
    ]));
    assert!(!is_destructive_variant(&[
        "git".into(),
        "status".into(),
        "-f".into()
    ]));
}

fn argv(parts: &[&str]) -> Vec<String> {
    parts.iter().map(|t| t.to_string()).collect()
}

#[test]
fn unflagged_work_and_history_discarding_git_variants_are_destructive() {
    for danger in [
        vec!["git", "checkout", "."],
        vec!["git", "checkout", "--", "src/a.rs"],
        vec!["git", "checkout", "main", "--", "a"],
        vec!["git", "stash", "drop"],
        vec!["git", "stash", "clear"],
        vec!["git", "-C", "d", "stash", "drop", "stash@{1}"],
        vec!["git", "reset", "HEAD~3"],
        vec!["git", "reset", "--soft", "abc123"],
        vec!["git", "commit", "--amend"],
        vec!["git", "commit", "-m", "x", "--amend"],
        vec!["git", "tag", "-d", "v1"],
        vec!["git", "tag", "--delete", "v1"],
        vec!["git", "update-ref", "-d", "refs/heads/x"],
        vec!["git", "reflog", "expire", "--expire=now", "--all"],
        vec!["git", "reflog", "delete", "HEAD@{1}"],
        vec!["git", "gc", "--prune=now"],
        vec!["git", "prune"],
        vec!["git", "filter-branch", "--all"],
        vec!["git", "worktree", "remove", "-f", "w"],
        vec!["git", "worktree", "remove", "--force", "w"],
    ] {
        assert!(is_destructive_variant(&argv(&danger)), "{danger:?}");
    }
    for harmless in [
        vec!["git", "checkout", "main"],
        vec!["git", "checkout", "-b", "feature"],
        vec!["git", "stash"],
        vec!["git", "stash", "list"],
        vec!["git", "reset"],
        vec!["git", "reset", "HEAD"],
        vec!["git", "reset", "--keep"],
        vec!["git", "reset", "--", "file"],
        vec!["git", "commit", "-m", "x"],
        vec!["git", "tag", "v1"],
        vec!["git", "reflog"],
        vec!["git", "gc"],
        vec!["git", "worktree", "list"],
        vec!["git", "worktree", "remove", "w"],
    ] {
        assert!(!is_destructive_variant(&argv(&harmless)), "{harmless:?}");
    }
    // The run-grant scenario: approving `git checkout main` never covers `git checkout -- .`.
    let key = |parts: &[&str]| shell_action(parts).grant_key();
    assert_ne!(
        key(&["git", "checkout", "main"]),
        key(&["git", "checkout", "--", "."])
    );
}

#[test]
fn git_long_option_abbreviations_are_still_destructive() {
    for danger in [
        vec!["git", "reset", "--har"],
        vec!["git", "push", "--mir"],
        vec!["git", "push", "--dele", "origin", "x"],
        vec!["git", "push", "--forc", "origin"],
        vec!["git", "push", "--force-with", "origin"],
        vec!["git", "clean", "--forc"],
        vec!["git", "checkout", "--discard"],
        vec!["git", "commit", "--am"],
        vec!["git", "branch", "--forc", "x"],
    ] {
        assert!(is_destructive_variant(&argv(&danger)), "{danger:?}");
    }
    assert!(!is_destructive_variant(&argv(&[
        "git",
        "push",
        "--set-upstream"
    ])));
    assert!(!is_destructive_variant(&argv(&[
        "git", "restore", "--stag", "f"
    ])));
}

#[test]
fn package_publish_is_found_behind_value_taking_options_and_abbreviations() {
    for danger in [
        vec!["pnpm", "--filter", "pkg", "publish"],
        vec!["pnpm", "-F", "pkg", "publish"],
        vec!["npm", "-w", "x", "publish"],
        vec!["npm", "--workspace", "x", "publish"],
        vec!["npm", "--otp", "123456", "publish"],
        vec!["npm", "--registry", "https://r.example", "unpublish", "x"],
        vec!["npm", "publi"],
        vec!["npm", "pub"],
        vec!["npm", "deprec", "x", "msg"],
        vec!["yarn", "--cwd", "pkg", "npm", "publish"],
        vec!["yarn", "npm", "publish"],
        vec!["yarn", "workspace", "pkg", "npm", "publish"],
        vec!["cargo", "--config", "k=v", "publish"],
        vec!["cargo", "+nightly", "publish"],
        vec!["cargo", "-Z", "unstable-options", "yank", "x"],
    ] {
        assert!(is_destructive_variant(&argv(&danger)), "{danger:?}");
    }
    for harmless in [
        vec!["npm", "test"],
        vec!["npm", "-w", "x", "test"],
        vec!["yarn", "npm", "whoami"],
        vec!["cargo", "--config", "k=v", "build"],
        vec!["pnpm", "--filter", "pkg", "test"],
    ] {
        assert!(!is_destructive_variant(&argv(&harmless)), "{harmless:?}");
    }
    let key = |parts: &[&str]| shell_action(parts).grant_key();
    assert_ne!(
        key(&["yarn", "npm", "whoami"]),
        key(&["yarn", "npm", "publish"])
    );
    // git `--config-env <name>=<ENV>` takes a separate value.
    assert!(is_destructive_variant(&argv(&[
        "git",
        "--config-env",
        "a=B",
        "push",
        "-f"
    ])));
}

#[test]
fn allow_rule_for_git_push_does_not_cover_forced_push() {
    let config = resolve(
            Some(r#"{"approval":{"rules":[{"tool":"shell","command":["git","push"],"action":"allow"}]}}"#),
            None,
            None,
        )
        .unwrap();
    let policy = config.config.approval.clone();
    let decide = |argv: &[&str]| policy.decision(&shell_action(argv));
    assert_eq!(decide(&["git", "push"]), PolicyAction::Allow);
    assert_eq!(
        decide(&["git", "push", "origin", "main"]),
        PolicyAction::Allow
    );
    for argv in [
        ["git", "push", "--force"].as_slice(),
        &["git", "push", "origin", "main", "-f"],
        &["git", "push", "origin", ":old"],
    ] {
        assert_eq!(decide(argv), PolicyAction::Ask, "{argv:?}");
    }
    // A rule that names the variant explicitly is honored.
    let explicit = resolve(
            Some(r#"{"approval":{"rules":[{"tool":"shell","command":["git","push","--force"],"action":"allow"}]}}"#),
            None,
            None,
        )
        .unwrap();
    assert_eq!(
        explicit
            .config
            .approval
            .decision(&shell_action(&["git", "push", "--force", "origin"])),
        PolicyAction::Allow
    );
    // Deny rules are unaffected.
    let deny = resolve(
        Some(
            r#"{"approval":{"rules":[{"tool":"shell","command":["git","push"],"action":"deny"}]}}"#,
        ),
        None,
        None,
    )
    .unwrap();
    assert_eq!(
        deny.config
            .approval
            .decision(&shell_action(&["git", "push", "--force"])),
        PolicyAction::Deny
    );
}

#[test]
fn approving_git_push_once_always_still_asks_for_force_push() {
    let calls = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let counter = Arc::clone(&calls);
    let hook = configured_hook(
        ApprovalMode::Custom,
        ApprovalPolicy::default(),
        Arc::new(crate::tools::FnHook::new(move |_: &ToolAction| {
            counter.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            Approval::AllowAlways
        })),
    );
    hook.approve(&shell_action(&["git", "push"]));
    hook.approve(&shell_action(&["git", "push", "origin", "main"]));
    assert_eq!(calls.load(std::sync::atomic::Ordering::SeqCst), 1);
    hook.approve(&shell_action(&["git", "push", "--force"]));
    assert_eq!(calls.load(std::sync::atomic::Ordering::SeqCst), 2);
}

#[test]
fn destructive_shell_variant_scope_is_the_exact_argv() {
    let danger = shell_action(&["git", "push", "--force", "origin", "main"]);
    let scope = danger.grant_scope().unwrap();
    assert_eq!(scope.argv, ["git", "push", "--force", "origin", "main"]);
    assert!(!scope.covers_further_args);
    assert_eq!(
        danger.grant_key(),
        format!("shell\0{}", scope.argv.join("\0"))
    );
    let safe = shell_action(&["git", "push", "origin", "main"])
        .grant_scope()
        .unwrap();
    assert_eq!(safe.argv, ["git", "push"]);
    assert!(safe.covers_further_args);
}

#[test]
fn grant_scope_matches_grant_key_semantics() {
    let cases: &[&[&str]] = &[
        &["cargo", "test", "--lib"],
        &["cargo", "run", "--", "a"],
        &["find", ".", "-delete"],
        &["rm", "-rf /"],
        &["rm", "-rf", "/"],
        &["git", "-C", "x", "status"],
        &["echo", "a\nb`c\u{202e}"],
    ];
    for argv in cases {
        let action = shell_action(argv);
        let scope = action.grant_scope().expect("shell has a scope");
        // The key is exactly the scope's tokens: equal scopes <=> equal keys.
        assert_eq!(
            action.grant_key(),
            format!("shell\0{}", scope.argv.join("\0"))
        );
        assert_eq!(scope.covers_further_args, scope.argv.len() < argv.len());
        assert_eq!(
            scope.argv[..],
            argv.iter().map(|t| t.to_string()).collect::<Vec<_>>()[..scope.argv.len()]
        );
    }
    assert_eq!(
        shell_action(&["cargo", "test", "--lib"])
            .grant_scope()
            .unwrap()
            .argv,
        ["cargo", "test"]
    );
    // Tokens stay distinct: a space inside one argument is not two arguments.
    assert_ne!(
        shell_action(&["rm", "-rf /"]).grant_scope(),
        shell_action(&["rm", "-rf", "/"]).grant_scope()
    );
    let mut other = shell_action(&["status"]);
    other.tool = "git".into();
    assert_eq!(other.grant_scope(), None);
}

#[test]
fn cached_shell_grant_does_not_extend_to_another_program() {
    let calls = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let counter = Arc::clone(&calls);
    let hook = configured_hook(
        ApprovalMode::Custom,
        ApprovalPolicy::default(),
        Arc::new(crate::tools::FnHook::new(move |_: &ToolAction| {
            counter.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            Approval::AllowAlways
        })),
    );
    assert_eq!(
        hook.approve(&shell_action(&["cargo", "test"])),
        Approval::AllowOnce
    );
    assert_eq!(
        hook.approve(&shell_action(&["cargo", "test", "-q"])),
        Approval::AllowOnce
    );
    assert_eq!(calls.load(std::sync::atomic::Ordering::SeqCst), 1);
    hook.approve(&shell_action(&["rm", "-rf", "x"]));
    assert_eq!(calls.load(std::sync::atomic::Ordering::SeqCst), 2);
}

fn counting_hook(
    calls: &Arc<std::sync::atomic::AtomicUsize>,
    answer: Approval,
) -> Arc<dyn ApprovalHook> {
    let calls = calls.clone();
    Arc::new(crate::tools::FnHook::new(move |_| {
        calls.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        answer
    }))
}

fn escape(argv: &[&str]) -> ToolAction {
    let mut action = shell_action(argv);
    action.tool = crate::tools::UNSANDBOXED_TOOL.into();
    action
}

/// Decision for an escape under `rules`, with an interactive hook that
/// answers `answer`; returns the decision and how often a person was asked.
fn escape_with(
    rules: &str,
    mode: ApprovalMode,
    answer: Approval,
    grants: Arc<ChatGrants>,
) -> (Approval, usize) {
    let calls = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let policy = resolve(Some(rules), None, None).unwrap().config.approval;
    let hook = configured_hook_with_grants(mode, policy, counting_hook(&calls, answer), grants);
    let decision = hook.approve(&escape(&["touch", "/outside"]));
    (decision, calls.load(std::sync::atomic::Ordering::SeqCst))
}

#[test]
fn broad_allow_rules_never_let_a_command_leave_the_sandbox() {
    for rules in [
        r#"{"approval":{"rules":[{"tool":"*","action":"allow"}]}}"#,
        r#"{"approval":{"rules":[{"tool":"shell*","action":"allow"}]}}"#,
        r#"{"approval":{"rules":[{"tool":"*","risk":"execute","action":"allow"}]}}"#,
        r#"{"approval":{"default":"allow","rules":[]}}"#,
        "{}",
    ] {
        // Without a person (the dialog denies) it never runs outside...
        let (decision, asked) = escape_with(
            rules,
            ApprovalMode::Custom,
            Approval::Deny,
            ChatGrants::in_memory(),
        );
        assert_eq!(decision, Approval::Deny, "{rules}");
        assert_eq!(asked, 1, "a person is asked: {rules}");
        // ...and the person's Allow once is what lets it.
        let (decision, _) = escape_with(
            rules,
            ApprovalMode::Custom,
            Approval::AllowOnce,
            ChatGrants::in_memory(),
        );
        assert_eq!(decision, Approval::AllowOnce, "{rules}");
    }
}

#[test]
fn deny_rules_of_any_shape_refuse_the_escape_without_asking() {
    for rules in [
        r#"{"approval":{"rules":[{"tool":"*","action":"deny"}]}}"#,
        r#"{"approval":{"rules":[{"tool":"shell_unsandboxed","action":"deny"}]}}"#,
        r#"{"approval":{"default":"deny","rules":[]}}"#,
    ] {
        let (decision, asked) = escape_with(
            rules,
            ApprovalMode::Custom,
            Approval::AllowOnce,
            ChatGrants::in_memory(),
        );
        assert_eq!((decision, asked), (Approval::Deny, 0), "{rules}");
    }
}

#[test]
fn only_a_grant_an_exact_rule_or_a_person_lets_a_command_leave() {
    let grants = ChatGrants::in_memory();
    assert!(grants.remember(&escape(&["touch", "/outside"])));
    let (decision, asked) = escape_with("{}", ApprovalMode::Custom, Approval::Deny, grants);
    assert_eq!((decision, asked), (Approval::AllowOnce, 0));
    // A plain `shell` grant for the same argv does not cover the escape.
    let shell_only = ChatGrants::in_memory();
    assert!(shell_only.remember(&shell_action(&["touch", "/outside"])));
    let (decision, asked) = escape_with("{}", ApprovalMode::Custom, Approval::Deny, shell_only);
    assert_eq!((decision, asked), (Approval::Deny, 1));
    let exact = r#"{"approval":{"rules":[{"tool":"shell_unsandboxed","command":["touch"],"action":"allow"}]}}"#;
    let (decision, asked) = escape_with(
        exact,
        ApprovalMode::Custom,
        Approval::Deny,
        ChatGrants::in_memory(),
    );
    assert_eq!((decision, asked), (Approval::AllowOnce, 0));
    // YOLO never asks, so it never leaves the sandbox either.
    let (decision, asked) = escape_with(
        "{}",
        ApprovalMode::Yolo,
        Approval::AllowOnce,
        ChatGrants::in_memory(),
    );
    assert_eq!((decision, asked), (Approval::Deny, 0));
}

/// Auto never asks the person: rules decide first (a deny rule beats the reviewer, an allow
/// rule skips it), the reviewer decides where Custom would ask, a soft block carries its
/// reason, the third on the same target turns hard, a hard block tells the person, and with
/// no reviewer nothing that would ask runs.
#[test]
fn auto_asks_its_reviewer_never_the_person() {
    use crate::tools::{AutoReviewer, AutoVerdict};
    struct Scripted {
        verdict: AutoVerdict,
        reviewed: std::sync::atomic::AtomicUsize,
        told: std::sync::atomic::AtomicUsize,
    }
    impl AutoReviewer for Scripted {
        fn review(&self, _: &ToolAction) -> AutoVerdict {
            self.reviewed
                .fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            self.verdict.clone()
        }
        fn blocked(&self, _: &ToolAction, _: &str, _: &str) {
            self.told.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        }
        fn allowed_once(&self, _: &ToolAction) -> bool {
            false
        }
    }
    let rules = r#"{"approval":{"rules":[
            {"tool":"shell","command":["rm"],"action":"deny"},
            {"tool":"shell","command":["cargo"],"action":"allow"}]}}"#;
    let policy = resolve(Some(rules), None, None).unwrap().config.approval;
    let with = |verdict: AutoVerdict| {
        let reviewer = Arc::new(Scripted {
            verdict,
            reviewed: Default::default(),
            told: Default::default(),
        });
        let calls = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let hook = configured_hook_with_reviewer(
            ApprovalMode::Auto,
            policy.clone(),
            counting_hook(&calls, Approval::AllowOnce),
            ChatGrants::in_memory(),
            None,
            Some(reviewer.clone() as Arc<dyn AutoReviewer>),
        );
        (hook, reviewer, calls)
    };
    let count = |n: &std::sync::atomic::AtomicUsize| n.load(std::sync::atomic::Ordering::SeqCst);

    let (hook, reviewer, person) = with(AutoVerdict::Allow);
    assert_eq!(
        hook.approve(&shell_action(&["rm", "-rf", "x"])),
        Approval::Deny
    );
    assert_eq!(
        hook.approve(&shell_action(&["cargo", "test"])),
        Approval::AllowOnce
    );
    assert_eq!(count(&reviewer.reviewed), 0, "rules decide first");
    assert_eq!(hook.approve(&shell_action(&["make"])), Approval::AllowOnce);
    assert_eq!(count(&reviewer.reviewed), 1);
    assert!(!hook.would_ask(&shell_action(&["make"])));
    assert_eq!(count(&person), 0, "the person is never asked");

    let (hook, reviewer, _) = with(AutoVerdict::Soft {
        reason: "it rewrites history".into(),
        safer: "push to a new branch".into(),
    });
    let push = shell_action(&["git", "push", "--force"]);
    assert_eq!(hook.approve(&push), Approval::DenyReviewed);
    let note = crate::tools::take_review_note(&push).unwrap();
    assert!(
        note.contains("it rewrites history") && note.contains("push to a new branch"),
        "{note}"
    );
    assert_eq!(count(&reviewer.told), 0, "a soft block asks nobody");
    hook.approve(&push);
    hook.approve(&push);
    assert_eq!(
        count(&reviewer.told),
        1,
        "the third soft block is a hard one"
    );
    assert!(crate::tools::take_review_note(&push)
        .unwrap()
        .contains("blocked this"));

    let (hook, reviewer, _) = with(AutoVerdict::Hard {
        reason: "it deletes outside the project".into(),
        safer: String::new(),
    });
    assert_eq!(
        hook.approve(&shell_action(&["make", "wipe"])),
        Approval::DenyReviewed
    );
    assert_eq!(count(&reviewer.told), 1);

    let calls = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let lone = configured_hook_with_reviewer(
        ApprovalMode::Auto,
        policy.clone(),
        counting_hook(&calls, Approval::AllowOnce),
        ChatGrants::in_memory(),
        None,
        None,
    );
    assert_eq!(
        lone.approve(&shell_action(&["make"])),
        Approval::DenyReviewed
    );
    assert_eq!(count(&calls), 0);

    // Running outside the sandbox needs a person: even a reviewer that allows everything
    // never opens it; the person is asked.
    let (hook, reviewer, person) = with(AutoVerdict::Allow);
    let escape = ToolAction {
        tool: crate::tools::UNSANDBOXED_TOOL.into(),
        ..shell_action(&["make", "deploy"])
    };
    assert_eq!(hook.approve(&escape), Approval::DenyReviewed);
    assert_eq!(
        count(&reviewer.reviewed),
        0,
        "the model never judges a sandbox escape"
    );
    assert_eq!(count(&reviewer.told), 1, "the person is asked");
    assert_eq!(count(&person), 0);
}

/// The reviewer's model is the person's to choose: a project may not set it.
#[test]
fn auto_model_is_user_file_only_and_a_model_id() {
    let user = r#"{"approval":{"auto_model":"muse-spark-1.3-contributor"}}"#;
    let resolved = resolve(Some(user), None, None).unwrap().config;
    assert_eq!(
        resolved.approval.auto_model.as_deref(),
        Some("muse-spark-1.3-contributor")
    );
    let project = r#"{"approval":{"auto_model":"cheap"}}"#;
    let refused = resolve(None, Some(project), None).unwrap_err().to_string();
    assert!(refused.contains("user file only"), "{refused}");
    assert!(resolve(
        Some(r#"{"approval":{"auto_model":"bad model!"}}"#),
        None,
        None
    )
    .is_err());
}

/// YOLO never asks, but a deny rule someone wrote still holds; ask rules and a
/// `deny` default (no rule matched) do not stop it.
#[test]
fn yolo_keeps_written_deny_rules_and_nothing_else() {
    let rules = r#"{"approval":{"default":"deny","rules":[
            {"tool":"shell","command":["git","push","--force"],"action":"deny"},
            {"tool":"shell","command":["make"],"action":"ask"}]}}"#;
    let policy = resolve(Some(rules), None, None).unwrap().config.approval;
    let run = |argv: &[&str]| {
        let calls = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let hook = configured_hook_with_grants(
            ApprovalMode::Yolo,
            policy.clone(),
            counting_hook(&calls, Approval::Deny),
            ChatGrants::in_memory(),
        );
        let action = shell_action(argv);
        let decision = (hook.approve(&action), hook.denied_by_rule(&action));
        assert_eq!(
            calls.load(std::sync::atomic::Ordering::SeqCst),
            0,
            "YOLO never asks"
        );
        decision
    };
    assert_eq!(
        run(&["git", "push", "--force", "origin"]),
        (Approval::Deny, true)
    );
    assert_eq!(run(&["make", "all"]), (Approval::AllowOnce, false));
    assert_eq!(run(&["cargo", "test"]), (Approval::AllowOnce, false));
    assert_eq!(
        policy.yolo_decision(&shell_action(&["git", "push", "--force"])),
        PolicyAction::Deny
    );
    assert_eq!(
        policy.yolo_decision(&shell_action(&["ls"])),
        PolicyAction::Allow
    );
}

#[test]
fn chat_grant_made_in_one_run_covers_a_later_run_of_the_same_chat_only() {
    let dir = tempfile::tempdir().unwrap();
    let chat = dir.path().join("chat-a.json");
    let calls = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let run = |path: &std::path::Path| {
        configured_hook_with_grants(
            ApprovalMode::Custom,
            ApprovalPolicy::default(),
            counting_hook(&calls, Approval::AllowAlways),
            ChatGrants::load(path),
        )
    };
    let first = run(&chat);
    first.approve(&shell_action(&["cargo", "test"]));
    // Covered at once inside the same run.
    first.approve(&shell_action(&["cargo", "test", "-q"]));
    assert_eq!(calls.load(std::sync::atomic::Ordering::SeqCst), 1);
    // A second run of the same chat loads the saved grant.
    run(&chat).approve(&shell_action(&["cargo", "test", "--lib"]));
    assert_eq!(calls.load(std::sync::atomic::Ordering::SeqCst), 1);
    // Another chat asks again.
    run(&dir.path().join("chat-b.json")).approve(&shell_action(&["cargo", "test"]));
    assert_eq!(calls.load(std::sync::atomic::Ordering::SeqCst), 2);
    // A different command family in the same chat also asks.
    run(&chat).approve(&shell_action(&["cargo", "run"]));
    assert_eq!(calls.load(std::sync::atomic::Ordering::SeqCst), 3);
}

#[test]
fn destructive_variants_ask_every_time_despite_a_chat_grant() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("chat.json");
    let calls = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let hook = configured_hook_with_grants(
        ApprovalMode::Custom,
        ApprovalPolicy::default(),
        counting_hook(&calls, Approval::AllowAlways),
        ChatGrants::load(&path),
    );
    let forced = shell_action(&["git", "push", "--force", "origin", "main"]);
    assert_eq!(hook.approve(&forced), Approval::AllowOnce);
    assert_eq!(hook.approve(&forced), Approval::AllowOnce);
    assert_eq!(calls.load(std::sync::atomic::Ordering::SeqCst), 2);
    let mut delete = shell_action(&["x"]);
    delete.tool = "delete_file".into();
    delete.command.clear();
    delete.risk = RiskLevel::Destructive;
    hook.approve(&delete);
    hook.approve(&delete);
    assert_eq!(calls.load(std::sync::atomic::Ordering::SeqCst), 4);
    assert!(!path.exists(), "nothing destructive is ever stored");
}

#[test]
fn deny_rules_beat_chat_grants() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("chat.json");
    let grants = ChatGrants::load(&path);
    assert!(grants.remember(&shell_action(&["rm", "build.log"])));
    let calls = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let policy = resolve(
        Some(r#"{"approval":{"rules":[{"tool":"shell","command":["rm"],"action":"deny"}]}}"#),
        None,
        None,
    )
    .unwrap()
    .config
    .approval;
    let hook = configured_hook_with_grants(
        ApprovalMode::Custom,
        policy,
        counting_hook(&calls, Approval::AllowOnce),
        grants,
    );
    assert_eq!(
        hook.approve(&shell_action(&["rm", "build.log"])),
        Approval::Deny
    );
    assert_eq!(calls.load(std::sync::atomic::Ordering::SeqCst), 0);
}

#[test]
fn invalid_grant_file_never_widens_permissions() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("chat.json");
    std::fs::write(
            &path,
            r#"{"version":1,"grants":[{"tool":"shell","command":["echo"],"risk":"execute"},{"tool":"shell","command":[],"risk":"execute"}]}"#,
        )
        .unwrap();
    let calls = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let hook = configured_hook_with_grants(
        ApprovalMode::Custom,
        ApprovalPolicy::default(),
        counting_hook(&calls, Approval::Deny),
        ChatGrants::load(&path),
    );
    assert_eq!(hook.approve(&shell_action(&["echo", "x"])), Approval::Deny);
    assert_eq!(calls.load(std::sync::atomic::Ordering::SeqCst), 1);
}

mod prop {
    use super::*;
    use proptest::prelude::*;

    proptest! {
               #[test]
               fn prefix_match_agrees_with_slice_prefix(
                   command in proptest::collection::vec(".{0,8}", 0..6),
                   prefix in proptest::collection::vec(".{0,8}", 0..6),
               ) {
                   let expected = !prefix.is_empty() && command.starts_with(&prefix);
                   prop_assert_eq!(command_has_prefix(&command, &prefix), expected);
                   // Any non-empty prefix of the command itself always matches.
                   for n in 1..=command.len() {
                       prop_assert!(command_has_prefix(&command, &command[..n]));
                   }
               }

        #[test]
        fn a_repeated_key_is_found_and_distinct_keys_never_are(
            keys in proptest::collection::btree_set("[a-z\"\\\\]{1,6}", 1..6),
            nested in any::<bool>(),
            repeat in any::<proptest::sample::Index>(),
        ) {
            let keys: Vec<String> = keys.into_iter().collect();
            let entry = |key: &String| format!("{}: [{{\"x\": 1}}, \"x\"]", serde_json::to_string(key).unwrap());
            let body: Vec<String> = keys.iter().map(entry).collect();
            let wrap = |inner: String| if nested { format!("{{\"outer\": {{{inner}}}, \"x\": 1}}") } else { format!("{{{inner}}}") };
            let distinct = wrap(body.join(",\n"));
            prop_assert_eq!(repeated_key(distinct.as_bytes()), None);
            let mut doubled = body.clone();
            doubled.push(entry(repeat.get(&keys)));
            let doubled = wrap(doubled.join(",\n"));
            prop_assert!(repeated_key(doubled.as_bytes()).is_some(), "{doubled}");
        }

        #[test]
        fn deny_matching_only_widens_exact_prefixes(
            command in proptest::collection::vec("-{0,2}[a-zA-Z]{0,4}", 0..7),
            prefix in proptest::collection::vec("-{0,2}[a-zA-Z]{0,4}", 0..5),
        ) {
            // Never panics, and every exact prefix match is also a deny match.
            let widened = deny_matches(&command, &prefix);
            if command_has_prefix(&command, &prefix) {
                prop_assert!(widened, "{command:?} {prefix:?}");
            }
        }

        #[test]
        fn deny_flags_match_in_any_order_and_cluster(
            letters in proptest::sample::subsequence(vec!['r', 'f', 'v', 'i'], 1..=4),
            split in proptest::bool::ANY,
        ) {
            let rule: Vec<String> = vec!["rm".into(), format!("-{}", letters.iter().collect::<String>())];
            let mut command = vec!["rm".to_string()];
            if split {
                command.extend(letters.iter().rev().map(|c| format!("-{c}")));
            } else {
                command.push(format!("-{}", letters.iter().rev().collect::<String>()));
            }
            command.push("target".into());
            prop_assert!(deny_matches(&command, &rule), "{command:?} {rule:?}");
        }

        #[test]
        fn destructive_flag_never_shares_the_harmless_key(
            sub in proptest::sample::select(vec!["push", "clean", "reset", "checkout", "branch"]),
            tail in proptest::collection::vec("[a-z/:+.-]{0,6}", 0..4),
        ) {
            let argv = |extra: &[String]| {
                let mut v = vec!["git".to_string(), sub.to_string()];
                v.extend(extra.iter().cloned());
                v
            };
            let mk = |command: Vec<String>| ToolAction {
                display: None, command, tool: "shell".into(), summary: String::new(), risk: RiskLevel::Execute,
            };
            let danger = mk(argv(&tail));
            let harmless = mk(argv(&[]));
            if is_destructive_variant(&danger.command) {
                prop_assert_ne!(danger.grant_key(), harmless.grant_key());
                // Full argv: never shorter than the whole command.
                prop_assert_eq!(danger.grant_key().split('\0').count(), 1 + danger.command.len());
            } else {
                prop_assert_eq!(danger.grant_key(), harmless.grant_key());
            }
        }

        #[test]
        fn any_prefix_of_a_destructive_long_option_is_destructive(
            case in proptest::sample::select(vec![
                ("push", "force"), ("push", "mirror"), ("push", "delete"),
                ("reset", "hard"), ("clean", "force"), ("commit", "amend"),
                ("branch", "force"), ("tag", "delete"), ("gc", "prune"),
            ]),
            len in 1usize..=8,
        ) {
            let (sub, long) = case;
            let prefix = &long[..len.min(long.len())];
            let command = vec!["git".to_string(), sub.to_string(), format!("--{prefix}")];
            prop_assert!(is_destructive_variant(&command), "{command:?}");
        }

        #[test]
        fn registry_publish_is_found_after_any_option_prefix(
            tool in proptest::sample::select(vec!["npm", "pnpm", "yarn"]),
            flags in proptest::collection::vec("-{1,2}[a-z]{1,6}", 0..4),
            values in proptest::collection::vec("[a-z0-9=]{1,6}", 0..3),
        ) {
            let mut command = vec![tool.to_string()];
            for (i, flag) in flags.iter().enumerate() {
                command.push(flag.clone());
                if let Some(value) = values.get(i) { command.push(value.clone()); }
            }
            command.push("publish".into());
            prop_assert!(is_destructive_variant(&command), "{command:?}");
        }

        #[test]
        fn is_destructive_variant_never_panics(command in proptest::collection::vec(".{0,8}", 0..6)) {
            let _ = is_destructive_variant(&command);
        }

        #[test]
        fn grant_key_never_panics_and_is_stable(command in proptest::collection::vec(".{0,8}", 0..6)) {
            let action = ToolAction {
                display: None,
                command,
                tool: "shell".into(),
                summary: String::new(),
                risk: RiskLevel::Execute,
            };
            prop_assert_eq!(action.grant_key(), action.grant_key());
        }
    }
}

#[test]
fn a_read_file_deny_also_denies_recall() {
    let action = |tool: &str| ToolAction {
        display: None,
        command: Vec::new(),
        tool: tool.into(),
        summary: String::new(),
        risk: RiskLevel::Read,
    };
    let deny_read_file = resolve(
        Some(r#"{"approval":{"rules":[{"tool":"read_file","action":"deny"}]}}"#),
        None,
        None,
    )
    .unwrap()
    .config
    .approval;
    assert_eq!(
        deny_read_file.decision(&action("recall")),
        PolicyAction::Deny
    );
    let deny_recall = resolve(
        Some(r#"{"approval":{"rules":[{"tool":"recall","action":"deny"}]}}"#),
        None,
        None,
    )
    .unwrap()
    .config
    .approval;
    assert_eq!(deny_recall.decision(&action("recall")), PolicyAction::Deny);
    assert_ne!(
        deny_recall.decision(&action("read_file")),
        PolicyAction::Deny
    );
    // Defaults keep recall a no-prompt read.
    let defaults = ApprovalPolicy::default();
    assert_eq!(defaults.decision(&action("recall")), PolicyAction::Allow);
}

#[test]
fn policy_order_and_project_restrictions() {
    let config = resolve(
        Some(r#"{"approval":{"default":"deny","rules":[{"tool":"read*","action":"allow"}]}}"#),
        Some(r#"{"approval":{"rules":[{"tool":"read_secret","action":"deny"}]}}"#),
        None,
    )
    .unwrap()
    .config;
    let action = |tool: &str| ToolAction {
        display: None,
        command: Vec::new(),
        tool: tool.into(),
        summary: String::new(),
        risk: RiskLevel::Read,
    };
    assert_eq!(
        config.approval.decision(&action("read_secret")),
        PolicyAction::Deny
    );
    assert_eq!(
        config.approval.decision(&action("read_file")),
        PolicyAction::Allow
    );
    assert!(resolve(None, Some(r#"{"approval":{"default":"allow"}}"#), None).is_err());
    let hook = configured_hook(
        ApprovalMode::Yolo,
        config.approval,
        Arc::new(crate::tools::DenyAllHook),
    );
    assert_eq!(hook.approve(&action("shell")), Approval::AllowOnce);
}

/// A person who answers `answer` and can be asked unless `attended` is false (an automation).
struct Person {
    answer: Approval,
    attended: bool,
    asked: std::sync::atomic::AtomicUsize,
}

impl ApprovalHook for Person {
    fn approve(&self, _: &ToolAction) -> Approval {
        self.asked.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        self.answer
    }
    fn would_ask(&self, _: &ToolAction) -> bool {
        self.attended
    }
}

/// Auto's reviewer, always answering `verdict`, counting its reviews.
struct Reviews {
    verdict: crate::tools::AutoVerdict,
    reviewed: std::sync::atomic::AtomicUsize,
}

impl crate::tools::AutoReviewer for Reviews {
    fn review(&self, _: &ToolAction) -> crate::tools::AutoVerdict {
        self.reviewed
            .fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        self.verdict.clone()
    }
    fn blocked(&self, _: &ToolAction, _: &str, _: &str) {}
    fn allowed_once(&self, _: &ToolAction) -> bool {
        false
    }
}

/// In Auto and YOLO a destructive step asks the person first, whatever the rules allow: their
/// answer decides, and with no answer in time the mode decides as before (YOLO runs it, Auto
/// asks its reviewer). A deny rule still refuses without asking, an automation (nobody to ask)
/// goes straight to the mode, ordinary steps are never asked, and Custom asks once, as ever.
#[test]
fn destructive_steps_ask_the_person_first_in_auto_and_yolo() {
    use std::sync::atomic::Ordering::SeqCst;
    let rules = r#"{"approval":{"rules":[
            {"tool":"shell","command":["git","clean"],"action":"deny"},
            {"tool":"shell","command":["rm"],"action":"allow"}]}}"#;
    let policy = resolve(Some(rules), None, None).unwrap().config.approval;
    let rm = shell_action(&["rm", "-rf", "build"]);
    let hook = |mode, answer, attended, verdict| {
        let person = Arc::new(Person {
            answer,
            attended,
            asked: Default::default(),
        });
        let reviewer = Arc::new(Reviews {
            verdict,
            reviewed: Default::default(),
        });
        let hook = configured_hook_with_reviewer(
            mode,
            policy.clone(),
            person.clone(),
            ChatGrants::in_memory(),
            None,
            Some(reviewer.clone() as Arc<dyn crate::tools::AutoReviewer>),
        );
        (hook, person, reviewer)
    };
    let allow = crate::tools::AutoVerdict::Allow;
    let hard = || crate::tools::AutoVerdict::Hard {
        reason: "it deletes the build".into(),
        safer: String::new(),
    };
    for mode in [ApprovalMode::Yolo, ApprovalMode::Auto] {
        // The person's answer decides, even over an allow rule for `rm`.
        let (h, person, _) = hook(mode, Approval::AllowOnce, true, hard());
        assert!(h.would_ask(&rm), "{mode:?}");
        assert_eq!(h.approve(&rm), Approval::AllowOnce, "{mode:?}");
        assert_eq!(person.asked.load(SeqCst), 1, "{mode:?}");
        let (h, _, _) = hook(mode, Approval::Deny, true, allow.clone());
        assert_eq!(h.approve(&rm), Approval::Deny, "{mode:?}");
        // Not destructive: never asked first.
        let (h, person, _) = hook(mode, Approval::Deny, true, allow.clone());
        assert_eq!(
            h.approve(&shell_action(&["ls"])),
            Approval::AllowOnce,
            "{mode:?}"
        );
        assert_eq!(person.asked.load(SeqCst), 0, "{mode:?}");
        // A deny rule refuses without asking.
        let (h, person, _) = hook(mode, Approval::AllowOnce, true, allow.clone());
        assert_eq!(
            h.approve(&shell_action(&["git", "clean", "-fd"])),
            Approval::Deny
        );
        assert_eq!(person.asked.load(SeqCst), 0, "{mode:?}");
    }
    // No answer in time: YOLO runs it; Auto decides as before (a broad `rm` allow rule never
    // covers `rm -rf`, so its reviewer does).
    let (h, _, _) = hook(ApprovalMode::Yolo, Approval::DenyTimedOut, true, hard());
    assert_eq!(h.approve(&rm), Approval::AllowOnce);
    let (h, _, reviewer) = hook(
        ApprovalMode::Auto,
        Approval::DenyTimedOut,
        true,
        allow.clone(),
    );
    assert_eq!(h.approve(&rm), Approval::AllowOnce);
    assert_eq!(reviewer.reviewed.load(SeqCst), 1);
    let push = shell_action(&["git", "push", "--force"]);
    let (h, person, reviewer) = hook(ApprovalMode::Auto, Approval::DenyTimedOut, true, hard());
    assert_eq!(
        h.approve(&push),
        Approval::DenyReviewed,
        "the reviewer decides"
    );
    assert_eq!(
        (person.asked.load(SeqCst), reviewer.reviewed.load(SeqCst)),
        (1, 1)
    );
    // An automation: nobody to ask, the mode decides at once.
    let (h, person, _) = hook(ApprovalMode::Yolo, Approval::Deny, false, allow.clone());
    assert_eq!(h.approve(&push), Approval::AllowOnce);
    assert_eq!(person.asked.load(SeqCst), 0);
    // Custom asks once, as it always did.
    let (h, person, _) = hook(ApprovalMode::Custom, Approval::AllowOnce, true, allow);
    assert_eq!(h.approve(&push), Approval::AllowOnce);
    assert_eq!(person.asked.load(SeqCst), 1);
}
