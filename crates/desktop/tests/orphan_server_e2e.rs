//! A server no client can find any more (its data folder or its `server.json` is gone, as when a
//! test's temporary folder is dropped) stops by itself instead of running for days.

use std::time::{Duration, Instant};

use themis_desktop::server::Server;
use themis_desktop::state::AppState;

/// Generous: the server checks every few seconds, and a busy CI machine may be slow.
const STOP_WITHIN: Duration = Duration::from_secs(30);

#[tokio::test(flavor = "multi_thread")]
async fn a_server_whose_discovery_file_is_removed_stops_and_logs_why() {
    let data = tempfile::tempdir().unwrap();
    let state = AppState::new_for_test(data.path().join("settings.json"));
    let server = Server::bind(state.clone(), data.path()).await.unwrap();
    let serving = tokio::spawn(server.run());
    std::fs::remove_file(data.path().join("server.json")).unwrap();
    let stopped = tokio::time::timeout(STOP_WITHIN, serving).await;
    assert!(
        stopped.is_ok(),
        "a server nobody can find kept running for {STOP_WITHIN:?}"
    );
    stopped.unwrap().unwrap().unwrap();
    let logs = state.query_diagnostic_logs(Default::default()).unwrap();
    let lost = logs
        .iter()
        .find(|record| record.service == "server" && record.event == "discovery_lost")
        .unwrap_or_else(|| panic!("no server/discovery_lost record in {logs:?}"));
    // Metadata only: the version, never the folder.
    let lost = serde_json::to_string(lost).unwrap();
    assert!(!lost.contains(&*data.path().to_string_lossy()), "{lost}");
}

/// Processes whose command line names `folder`.
#[cfg(unix)]
fn servers_for(folder: &std::path::Path) -> Vec<String> {
    let output = std::process::Command::new("pgrep")
        .arg("-f")
        .arg(folder.to_string_lossy().as_ref())
        .output()
        .unwrap();
    String::from_utf8_lossy(&output.stdout)
        .lines()
        .map(str::to_owned)
        .collect()
}

/// Stops whatever is left for `folder` even when an assertion fails first.
#[cfg(unix)]
struct Reap(std::path::PathBuf);

#[cfg(unix)]
impl Drop for Reap {
    fn drop(&mut self) {
        for pid in servers_for(&self.0) {
            let _ = std::process::Command::new("kill").arg(&pid).status();
        }
    }
}

#[cfg(unix)]
#[test]
fn a_cli_started_server_exits_when_its_data_folder_is_deleted() {
    let scratch = tempfile::tempdir().unwrap();
    let data = scratch.path().join("data");
    let _reap = Reap(data.clone());
    let started = std::process::Command::new(env!("CARGO_BIN_EXE_themis"))
        .arg("--data-dir")
        .arg(&data)
        .args(["server", "start"])
        .env("HOME", scratch.path())
        .env("THEMIS_SEARXNG_DIR", scratch.path().join("no-searxng"))
        .env_remove("OPENCODE_KEY")
        .output()
        .unwrap();
    assert!(
        started.status.success(),
        "{}",
        String::from_utf8_lossy(&started.stderr)
    );
    assert_eq!(servers_for(&data).len(), 1, "one detached server runs");
    // What `TempDir` does when a test ends, passed or failed, without `themis server stop`.
    std::fs::remove_dir_all(&data).unwrap();
    let deadline = Instant::now() + STOP_WITHIN;
    while !servers_for(&data).is_empty() {
        assert!(
            Instant::now() < deadline,
            "the server outlived its deleted data folder for {STOP_WITHIN:?}"
        );
        std::thread::sleep(Duration::from_millis(100));
    }
    assert!(
        !data.exists(),
        "the stopping server put its deleted data folder back"
    );
}
