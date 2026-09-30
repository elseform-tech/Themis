//! Doctor reads existing stores without boot recovery, migrations, or provider calls.
use serde_json::Value;
use themis_desktop::transcript::TranscriptStore;

fn doctor(directory: &std::path::Path) -> (bool, Value) {
    let output = std::process::Command::new(env!("CARGO_BIN_EXE_themis"))
        .arg("--data-dir")
        .arg(directory)
        .args(["doctor", "--json", "--deep"])
        .env_remove("OPENCODE_KEY")
        .output()
        .unwrap();
    (
        output.status.success(),
        serde_json::from_slice(&output.stdout).unwrap(),
    )
}

#[test]
fn offline_database_checks_detect_corruption_and_preserve_version() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("sessions.sqlite3");
    let store = TranscriptStore::open(&path).unwrap();
    store
        .append_user("thread", "run", "private conversation content")
        .unwrap();
    drop(store);
    let (ok, report) = doctor(dir.path());
    assert!(ok, "{report}");
    assert!(!report.to_string().contains("private conversation content"));
    let connection = rusqlite::Connection::open(&path).unwrap();
    connection.execute("DELETE FROM context_turns", []).unwrap();
    let (ok, report) = doctor(dir.path());
    assert!(!ok);
    assert!(report.to_string().contains("Persisted context differs"));
    connection.execute_batch("PRAGMA user_version=0").unwrap();
    let (ok, report) = doctor(dir.path());
    assert!(!ok);
    assert!(report.to_string().contains("Unsupported schema version 0"));
    let version: i64 = connection
        .query_row("PRAGMA user_version", [], |row| row.get(0))
        .unwrap();
    assert_eq!(version, 0, "doctor must not migrate");
    assert!(!dir.path().join("server.lock").exists());
}

#[tokio::test(flavor = "multi_thread")]
async fn online_doctor_uses_live_store_and_reports_invalid_history_without_contents() {
    use themis_desktop::{server::Server, state::AppState};
    let dir = tempfile::tempdir().unwrap();
    let state = AppState::new_for_test(dir.path().join("settings.json"));
    let server = Server::bind(state, dir.path()).await.unwrap();
    let task = tokio::spawn(server.run());
    let connection = rusqlite::Connection::open(dir.path().join("sessions.sqlite3")).unwrap();
    connection
        .execute(
            "INSERT INTO history(thread_id,item) VALUES ('t','secret-invalid-json')",
            [],
        )
        .unwrap();
    let (ok, report) = doctor(dir.path());
    assert!(!ok, "{report}");
    assert!(report["checks"]
        .as_array()
        .unwrap()
        .iter()
        .any(|check| check["name"] == "server" && check["status"] == "pass"));
    assert!(report.to_string().contains("Invalid history JSON"));
    assert!(!report.to_string().contains("secret-invalid-json"));
    task.abort();
}

#[test]
fn doctor_detects_missing_and_stale_checkpoints() {
    use themis_desktop::types::{ThreadEvent, ThreadEventEnvelope};
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("sessions.sqlite3");
    let store = TranscriptStore::open(&path).unwrap();
    for summary in ["first", "latest"] {
        store
            .append_event(&ThreadEventEnvelope {
                thread_id: "t".to_owned(),
                run_id: "r".to_owned(),
                event: ThreadEvent::ContextCheckpoint {
                    summary: summary.to_owned(),
                },
            })
            .unwrap();
    }
    assert!(doctor(dir.path()).0);
    let connection = rusqlite::Connection::open(path).unwrap();
    connection
        .execute(
            "UPDATE context_checkpoints SET through_seq=1,summary='first'",
            [],
        )
        .unwrap();
    assert!(!doctor(dir.path()).0);
    connection
        .execute("DELETE FROM context_checkpoints", [])
        .unwrap();
    assert!(!doctor(dir.path()).0);
}
