//! A stale `server.json` whose port another Themis server now holds (it answers `unauthorized`
//! to the old token) does not stall `themis server start` or exhaust local ports: the CLI
//! starts this folder's own server promptly and leaves the other one alone.

#![cfg(unix)]

use std::path::Path;
use std::time::{Duration, Instant};

use serde_json::Value;
use themis_desktop::server::{Client, Server};
use themis_desktop::state::AppState;

/// Well under the five seconds a client waits for a busy server, plus a slow CI machine's
/// startup; before the fix the wait alone spun for the full five seconds twice.
const START_WITHIN: Duration = Duration::from_secs(8);

fn themis(scratch: &Path, data: &Path, words: &[&str]) -> std::process::Output {
    std::process::Command::new(env!("CARGO_BIN_EXE_themis"))
        .arg("--data-dir")
        .arg(data)
        .args(words)
        .env("HOME", scratch)
        .env("THEMIS_SEARXNG_DIR", scratch.join("no-searxng"))
        .env_remove("OPENCODE_KEY")
        .output()
        .unwrap()
}

/// Stops the CLI-started server even when an assertion fails first.
struct Stop<'a>(&'a Path, &'a Path);

impl Drop for Stop<'_> {
    fn drop(&mut self) {
        let _ = themis(self.0, self.1, &["server", "stop"]);
        let _ = std::process::Command::new("pkill")
            .arg("-f")
            .arg(self.1.to_string_lossy().as_ref())
            .status();
    }
}

fn port_of(data: &Path) -> u64 {
    let text = std::fs::read_to_string(data.join("server.json")).unwrap();
    serde_json::from_str::<Value>(&text).unwrap()["port"]
        .as_u64()
        .unwrap()
}

#[tokio::test(flavor = "multi_thread")]
async fn server_start_replaces_a_stale_discovery_whose_port_refuses_the_token() {
    let scratch = tempfile::tempdir().unwrap();
    // Another folder's server, holding the port the stale file names.
    let other = scratch.path().join("other");
    let state = AppState::new_for_test(other.join("settings.json"));
    let server = Server::bind(state, &other).await.unwrap();
    let serving = tokio::spawn(server.run());
    let other_port = port_of(&other);
    let data = scratch.path().join("data");
    std::fs::create_dir_all(&data).unwrap();
    std::fs::write(
        data.join("server.json"),
        format!(r#"{{"port":{other_port},"token":"stale"}}"#),
    )
    .unwrap();
    let _stop = Stop(scratch.path(), &data);

    let started = Instant::now();
    let output = tokio::task::spawn_blocking({
        let (scratch, data) = (scratch.path().to_path_buf(), data.clone());
        move || themis(&scratch, &data, &["server", "start"])
    })
    .await
    .unwrap();
    let took = started.elapsed();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(took < START_WITHIN, "server start took {took:?}");
    assert_ne!(port_of(&data), other_port, "the folder got its own server");
    // The other folder's server was not taken over or stopped.
    let other_client = Client::new(other.clone());
    other_client.call("ping", Value::Null).await.unwrap();
    assert!(!serving.is_finished());
    other_client.call("shutdown", Value::Null).await.unwrap();
}
