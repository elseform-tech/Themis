use super::*;

fn named(names: &[&str]) -> Vec<Hook> {
    names
        .iter()
        .map(|name| Hook {
            name: (*name).into(),
            event: "RunEnd".into(),
            command: "true".into(),
            enabled: false,
            matcher: None,
            failure_policy: None,
            shell: None,
            background: false,
            command_prefix: None,
            command_glob: None,
            rewake: None,
            exit_two_blocks: false,
            timeout_seconds: 10,
            blocking: false,
            plugin_root: None,
            plugin_data: None,
            env_file: None,
            moment_matcher: None,
            prompt: None,
            runtime_identity: None,
        })
        .collect()
}

#[test]
fn names_come_from_the_script_or_else_from_when_the_hook_runs() {
    let name = |event, matcher, command| imported_hook_name(event, matcher, command, &[]);
    assert_eq!(
        name(
            "BeforeTool",
            Some("shell"),
            r#"python3 "${CLAUDE_PLUGIN_ROOT}/hooks/Check_Secrets.py""#
        ),
        "check-secrets"
    );
    assert_eq!(
        name(
            "BeforeTool",
            Some("shell|background_task"),
            "if grep -q x; then echo y; fi"
        ),
        "before-shell"
    );
    assert_eq!(
        name("AfterTool", Some("web_search"), "true"),
        "after-web-search"
    );
    assert_eq!(name("ToolCallFailed", None, "true"), "tool-failed");
    assert_eq!(
        name("PromptSubmit", None, "echo 'see notes/today.txt'"),
        "prompt-submit"
    );
    // A generic script name says nothing, so the event names the hook.
    assert_eq!(
        name("RunEnd", None, "node ${CLAUDE_PLUGIN_ROOT}/hooks/index.js"),
        "run-end"
    );
}

#[test]
fn names_stay_unique_and_valid_within_a_plugin() {
    let taken = named(&["run-end", "run-end-2"]);
    let name = imported_hook_name("RunEnd", None, "true", &taken);
    assert_eq!(name, "run-end-3");
    assert!(crate::plugins::safe_name(&name));
    assert!(crate::plugins::safe_name(&imported_hook_name(
        "BeforeTool",
        None,
        "./a--b.sh",
        &[]
    )));
}
