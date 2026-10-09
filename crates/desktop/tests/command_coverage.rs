//! Every frontend backend command must be handled by the local server.

use std::collections::BTreeSet;

#[test]
fn server_exposes_all_frontend_backend_commands() {
    let frontend = include_str!("../../../web/src/lib/tauri.ts");
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
        // Advanced configuration and logs intentionally have no desktop controls.
        .chain([
            "get_runtime_configuration",
            "save_runtime_configuration",
            "query_diagnostic_logs",
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
