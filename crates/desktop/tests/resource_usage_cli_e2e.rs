//! Themis's own CPU and memory use through the production server and CLI: the
//! server samples its process tree, `themis call resource_usage` reports it by
//! group, and a metadata record lands in the diagnostics log.
use serde_json::{json, Value};
use std::path::Path;
use std::time::Duration;
use themis_desktop::{server::Server, state::AppState};

fn cli(directory: &Path, method: &str, args: Value) -> Value {
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
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    serde_json::from_slice(&output.stdout).unwrap()
}

#[tokio::test(flavor = "multi_thread")]
async fn the_server_reports_and_logs_its_own_resource_use() {
    let directory = tempfile::tempdir().unwrap();
    let state = AppState::new_for_test(directory.path().join("settings.json"));
    let server = Server::bind(state.clone(), directory.path()).await.unwrap();
    let host = tokio::spawn(server.run());
    // A program the server's process started, like a dev server: a tool.
    let mut tool = std::process::Command::new("sleep")
        .arg("30")
        .spawn()
        .unwrap();
    let data = directory.path().to_owned();
    let usage = tokio::task::spawn_blocking(move || {
        // The first sample is taken at start; CPU needs a second one.
        let deadline = std::time::Instant::now() + Duration::from_secs(20);
        loop {
            let usage = cli(&data, "resource_usage", json!({}));
            if usage["groups"][2]["processes"].as_u64().unwrap_or(0) >= 1 {
                return usage;
            }
            assert!(std::time::Instant::now() < deadline, "{usage}");
            std::thread::sleep(Duration::from_millis(500));
        }
    })
    .await
    .unwrap();
    let groups: Vec<&str> = usage["groups"]
        .as_array()
        .unwrap()
        .iter()
        .map(|group| group["group"].as_str().unwrap())
        .collect();
    assert_eq!(groups, ["app", "browser", "tools"]);
    assert!(
        usage["groups"][0]["processes"].as_u64().unwrap() >= 1,
        "{usage}"
    );
    assert!(
        usage["memory_bytes"].as_u64().unwrap() > 1_000_000,
        "{usage}"
    );
    assert!(usage["cores"].as_u64().unwrap() >= 1);
    assert!(usage["groups"][0].get("growing_bytes").is_none(), "{usage}");
    let log = std::fs::read_to_string(directory.path().join("logs/diagnostics-0.jsonl")).unwrap();
    let sample: Value = log
        .lines()
        .filter_map(|line| serde_json::from_str::<Value>(line).ok())
        .find(|record| record["service"] == "resources" && record["event"] == "sample")
        .expect("a resources sample in the diagnostics log");
    assert_eq!(sample["level"], "info");
    assert!(sample["details"]["stream"]["frames"].is_u64(), "{sample}");
    tool.kill().unwrap();
    let _ = tool.wait();
    host.abort();
}
