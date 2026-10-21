//! A browser Themis started never outlives it: when the Themis process is
//! killed outright (a crash or force quit, so no cleanup code runs), its
//! Chromium and helpers exit too. Skipped (with a message) when no Chromium is
//! installed.

use std::time::{Duration, Instant};

use themis_core::browser::launch::{find_browser, BrowserProcess};

const HOST: &str = "THEMIS_TEST_BROWSER_HOST";
const NAME: &str = "a_browser_exits_when_themis_is_killed";

/// Whether the browser is still running: on Unix any process of its group
/// (helpers included), on Windows the browser process itself.
#[cfg(unix)]
fn running(pid: i32) -> bool {
    // SAFETY: signal 0 to the process group only checks that it exists.
    unsafe { libc::killpg(pid, 0) == 0 }
}

#[cfg(windows)]
fn running(pid: i32) -> bool {
    let list = std::process::Command::new("tasklist")
        .args(["/FI", &format!("PID eq {pid}"), "/NH"])
        .output()
        .unwrap();
    String::from_utf8_lossy(&list.stdout).contains(&pid.to_string())
}

#[tokio::test(flavor = "multi_thread")]
async fn a_browser_exits_when_themis_is_killed() {
    let Some(executable) = find_browser(None, None) else {
        eprintln!("skipping: no Chromium browser installed");
        return;
    };
    if let Some(profile) = std::env::var_os(HOST) {
        // The stand-in for Themis: start a browser, write its pid, then wait
        // to be killed.
        let profile = std::path::Path::new(&profile);
        let process = BrowserProcess::launch(&executable, &profile.join("profile"), true)
            .await
            .unwrap();
        std::fs::write(profile.join("pid"), process.pid().unwrap().to_string()).unwrap();
        tokio::time::sleep(Duration::from_secs(120)).await;
        return;
    }
    let directory = tempfile::tempdir().unwrap();
    let mut host = std::process::Command::new(std::env::current_exe().unwrap())
        .args([NAME, "--exact", "--nocapture", "--test-threads=1"])
        .env(HOST, directory.path())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn()
        .unwrap();
    let started = Instant::now();
    let pid: i32 = loop {
        if let Some(pid) = std::fs::read_to_string(directory.path().join("pid"))
            .ok()
            .and_then(|text| text.parse().ok())
        {
            break pid;
        }
        assert!(
            started.elapsed() < Duration::from_secs(30),
            "the host never started its browser"
        );
        std::thread::sleep(Duration::from_millis(100));
    };
    assert!(running(pid));
    // SIGKILL or TerminateProcess: no Drop, no shutdown hook.
    host.kill().unwrap();
    let _ = host.wait();
    let deadline = Instant::now() + Duration::from_secs(15);
    while running(pid) {
        assert!(
            Instant::now() < deadline,
            "the browser ({pid}) is still running after Themis was killed"
        );
        std::thread::sleep(Duration::from_millis(200));
    }
}
