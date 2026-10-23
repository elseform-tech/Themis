//! Every frontend backend command must be handled by the local server.

use std::collections::BTreeSet;

#[test]
fn server_exposes_all_frontend_backend_commands() {
    let frontend = concat!(
        include_str!("../../../web/src/lib/tauri.ts"),
        include_str!("../../../web/src/lib/browser.ts")
    );
    let server = include_str!("../src/server.rs");
    let native = include_str!("../src/lib.rs");
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
            "get_context_usage",
            "get_thread_history_page",
            // CLI path to the global runtime.jsonc; the desktop uses open_runtime_config.
            "ensure_runtime_config",
            // The agent's set_goal tool and the CLI set goals; the desktop sends `/goal`.
            "set_goal",
            // The desktop watches the mcp.json copy and applies saves itself; the CLI applies.
            "apply_mcp_config",
        ])
        .collect();
    let handled: BTreeSet<_> = server
        .lines()
        .filter_map(|line| {
            let line = line.trim();
            line.strip_prefix('"')?
                .split('"')
                .next()
                .filter(|_| line.contains("=>"))
        })
        .collect();
    assert_eq!(
        called, handled,
        "server and desktop command contracts drifted"
    );
    assert!(server.contains("state.record_error(&request.method, error.clone())"));
}
