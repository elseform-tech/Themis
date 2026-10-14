//! Windows counterpart of the process-group background task tests: stopping a
//! task (`taskkill /T /F`) must end a grandchild that is still in its tree.
#![cfg(windows)]

use std::time::{Duration, Instant};
use themis_core::tools::background::{BackgroundRegistry, StartSpec, TaskState, STOP_WAIT};

/// Number of `ping.exe` processes whose command line carries `-n <marker> `.
fn ping_count(marker: &str) -> usize {
    let script = format!(
        "@(Get-CimInstance Win32_Process -Filter \"Name='ping.exe'\" | \
         Where-Object {{ $_.CommandLine -like '*-n {marker} *' }}).Count"
    );
    let out = std::process::Command::new("powershell")
        .args(["-NoProfile", "-NonInteractive", "-Command", &script])
        .output()
        .expect("powershell must be available");
    String::from_utf8_lossy(&out.stdout)
        .trim()
        .parse()
        .unwrap_or_else(|_| panic!("unexpected powershell output: {out:?}"))
}

fn wait_until(what: &str, secs: u64, mut condition: impl FnMut() -> bool) {
    let deadline = Instant::now() + Duration::from_secs(secs);
    while !condition() {
        assert!(Instant::now() < deadline, "timed out waiting for {what}");
        std::thread::sleep(Duration::from_millis(200));
    }
}

#[test]
fn stopping_a_background_task_kills_its_grandchild_on_windows() {
    let dir = tempfile::tempdir().unwrap();
    let registry = BackgroundRegistry::in_memory();
    let marker = "3102";
    let started = registry
        .start(StartSpec {
            thread_id: "t1".into(),
            program: "cmd".into(),
            args: vec!["/c".into(), format!("ping -n {marker} 127.0.0.1")],
            workdir: dir.path().to_path_buf(),
            timeout_secs: 0,
        })
        .unwrap();
    assert_eq!(started.record.state, TaskState::Running);
    wait_until("grandchild start", 10, || ping_count(marker) > 0);
    let stopped = registry.stop("t1", &started.record.id, STOP_WAIT).unwrap();
    assert_eq!(stopped.record.state, TaskState::Stopped);
    wait_until("grandchild gone", 15, || ping_count(marker) == 0);
    assert_eq!(registry.running_count(), 0);
}
