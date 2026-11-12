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
    // Match arms: every `"name"` before `=>`, one or several joined by `|`.
    let handled: BTreeSet<_> = server
        .lines()
        .filter_map(|line| {
            let (pattern, _) = line.trim().split_once("=>")?;
            pattern.starts_with('"').then_some(pattern)
        })
        .flat_map(|pattern| pattern.split('|'))
        .filter_map(|name| name.trim().strip_prefix('"')?.split('"').next())
        .collect();
    assert_eq!(
        called, handled,
        "server and desktop command contracts drifted"
    );
    assert!(server.contains("state.record_error(&request.method, error.clone())"));
}
