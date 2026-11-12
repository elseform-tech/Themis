//! Every frontend backend command must be handled by the local server.

use std::collections::BTreeSet;

#[test]
fn server_exposes_all_frontend_backend_commands() {
    let frontend = concat!(
        include_str!("../../../web/src/lib/tauri.ts"),
        include_str!("../../../web/src/lib/browser.ts"),
        include_str!("../../../web/src/lib/terminal.ts"),
        include_str!("../../../web/src/lib/visions.ts")
    );
    let server = concat!(
        include_str!("../src/server.rs"),
        include_str!("../src/server/commands.rs"),
        include_str!("../src/server/visions_commands.rs")
    );
    let native = concat!(
        include_str!("../src/lib.rs"),
        include_str!("../src/notifications.rs")
    );
    let called: BTreeSet<_> = frontend
        .lines()
        .filter_map(|line| {
            let tail = line.split("invoke").nth(1)?;
            let tail = if tail.starts_with('<') {
                tail.split_once('>')?.1
            } else {
                tail
            };
            let tail = tail.strip_prefix('(')?;
            let quote = tail.chars().next()?;
            (quote == '\'' || quote == '"').then(|| tail[1..].split(quote).next().unwrap())
        })
        .filter(|name| *name != "check_for_updates")
        .chain(native.split(".call(").skip(1).filter_map(|part| {
            part.trim_start()
                .strip_prefix('"')
                .and_then(|tail| tail.split('"').next())
        }))
        // Advanced configuration, logs and usage queries have no desktop controls.
        .chain([
            "get_runtime_configuration",
            "save_runtime_configuration",
            "query_diagnostic_logs",
            "memory_try_pass",
            // Auto mode's reviewer dry run, for evals only.
            "auto_try_review",
            "get_context_usage",
            "get_thread_history_page",
            // CLI path to the global runtime.jsonc; the desktop uses open_runtime_config.
            "ensure_runtime_config",
            // The agent's create_goal tool and the CLI create goals; the desktop sends `/goal`.
            "create_goal",
            // The desktop watches the mcp.json copy and applies saves itself; the CLI applies.
            "apply_mcp_config",
            // Subagent state for the CLI; the desktop builds its card from chat events.
            "list_subagents",
            // The desktop's terminal lives until the project or the app goes; the CLI asks and closes.
            "terminal_state",
            "terminal_close",
            // A chat's saved originals and memory for `themis recall`/`themis memory`; the app
            // never lists memories, and the agent's tools do the same work in a run.
            "recall",
            "search_memory",
        ])
        .collect();
    let handled = arm_names(server);
    assert_eq!(
        called, handled,
        "server and desktop command contracts drifted"
    );
    assert!(server.contains("state.record_error(&request.method, error.clone())"));
}

/// Every `"name"` a match arm names before `=>`, one or several joined by `|`, also when the
/// arm runs over several lines.
fn arm_names(source: &str) -> BTreeSet<&str> {
    let mut names = BTreeSet::new();
    for (at, _) in source.match_indices("=>") {
        let mut before = source[..at].trim_end();
        while let Some(rest) = before.strip_suffix('"') {
            let Some(open) = rest.rfind('"') else { break };
            names.insert(&rest[open + 1..]);
            before = rest[..open].trim_end();
            match before.strip_suffix('|') {
                Some(rest) => before = rest.trim_end(),
                None => break,
            }
        }
    }
    names
}

/// The Visions methods the server routes (`visions_commands::METHODS`) are exactly those its
/// match arms answer, so a name in one but not the other cannot pass unnoticed.
#[test]
fn visions_methods_are_the_ones_their_arms_answer() {
    let source = include_str!("../src/server/visions_commands.rs");
    let list = source
        .split("const METHODS: &[&str] = &[")
        .nth(1)
        .and_then(|tail| tail.split("];").next())
        .expect("METHODS");
    let methods: BTreeSet<&str> = list
        .split(',')
        .filter_map(|item| item.trim().strip_prefix('"')?.strip_suffix('"'))
        .collect();
    assert_eq!(methods.len(), 13);
    assert_eq!(arm_names(source), methods);
}

#[test]
fn arm_names_read_arms_over_several_lines() {
    let source =
        "match m {\n    \"a\" | \"b\" => 1,\n    \"c\"\n    | \"d\" => 2,\n    x if y => 3,\n}";
    assert_eq!(arm_names(source), BTreeSet::from(["a", "b", "c", "d"]));
}
