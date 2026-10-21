//! Failed commands through the production server and CLI: each one leaves a
//! `command_failed` record in the diagnostics log with the method and a fixed
//! category, never the error message (it can name paths or hold user text).
use serde_json::{json, Value};
use std::path::Path;
use themis_desktop::{server::Server, state::AppState};

/// Runs `themis call`, expecting it to fail, and returns its error output.
fn failing_cli(directory: &Path, method: &str, args: Value) -> String {
    let output = std::process::Command::new(env!("CARGO_BIN_EXE_themis"))
        .args([
            "--data-dir",
            directory.to_str().unwrap(),
            "call",
            method,
            &args.to_string(),
        ])
        .env_remove("OPENCODE_KEY")
        .output()
        .unwrap();
    assert!(!output.status.success(), "{method} should fail");
    String::from_utf8_lossy(&output.stderr).into_owned()
}

#[tokio::test(flavor = "multi_thread")]
async fn failed_cli_commands_log_their_category_without_the_message() {
    let directory = tempfile::tempdir().unwrap();
    let state = AppState::new_for_test(directory.path().join("settings.json"));
    let server = Server::bind(state.clone(), directory.path()).await.unwrap();
    let host = tokio::spawn(server.run());
    let data = directory.path().to_owned();
    let errors = tokio::task::spawn_blocking(move || {
        [
            // A window from a newer build asking an older server.
            failing_cli(&data, "resource_usage_next", json!({})),
            failing_cli(&data, "get_thread", json!({})),
            failing_cli(
                &data,
                "open_project",
                json!({"path": "/private-client-name/secret-project"}),
            ),
        ]
    })
    .await
    .unwrap();
    assert!(errors[0].contains("unknown server method"), "{}", errors[0]);

    let log = std::fs::read_to_string(directory.path().join("logs/diagnostics-0.jsonl")).unwrap();
    let failed: Vec<Value> = log
        .lines()
        .filter_map(|line| serde_json::from_str::<Value>(line).ok())
        .filter(|record| record["event"] == "command_failed")
        .collect();
    let seen: Vec<(&str, &str)> = failed
        .iter()
        .map(|record| {
            (
                record["details"]["method"].as_str().unwrap(),
                record["details"]["category"].as_str().unwrap(),
            )
        })
        .collect();
    assert_eq!(seen[0], ("resource_usage_next", "unknown_method"));
    assert_eq!(seen[1], ("get_thread", "invalid_request"));
    assert_eq!(seen[2], ("open_project", "not_found"));
    // The path the person typed stays out of the log.
    assert!(!log.contains("secret-project"), "{log}");

    host.abort();
}
