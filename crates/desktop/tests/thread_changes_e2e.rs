//! Thread creation and metadata changes made through the shared server are
//! announced on the subscription stream, so an open window follows them.

use std::time::Duration;

use serde_json::{json, Value};
use themis_desktop::server::{Client, Server};
use themis_desktop::state::AppState;

#[allow(dead_code)]
mod common;

/// `themis call <method> -` against the data dir's server, as a user would.
fn cli_call(data_dir: &std::path::Path, method: &str, args: Value) -> Value {
    use std::io::Write;
    let mut child = std::process::Command::new(env!("CARGO_BIN_EXE_themis"))
        .arg("--data-dir")
        .arg(data_dir)
        .env_remove("OPENCODE_KEY")
        .args(["call", method, "-"])
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .expect("run CLI");
    child
        .stdin
        .take()
        .unwrap()
        .write_all(args.to_string().as_bytes())
        .unwrap();
    let output = child.wait_with_output().unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    serde_json::from_slice(&output.stdout).expect("CLI JSON")
}

async fn next_change(stream: &mut tokio::net::TcpStream) -> Value {
    loop {
        let event = tokio::time::timeout(Duration::from_secs(20), Client::next_event(stream))
            .await
            .expect("event within 20s")
            .expect("event frame");
        if event["name"] == "thread-changed" {
            return event["payload"].clone();
        }
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn shared_server_announces_thread_creation_and_changes() {
    if !common::git_available() {
        return;
    }
    let (_repo, project) = common::init_repo();
    let dir = tempfile::tempdir().unwrap();
    let state = AppState::new_for_test(dir.path().join("settings.json"));
    let server = Server::bind(state, dir.path()).await.unwrap();
    let task = tokio::spawn(server.run());
    let client = Client::new(dir.path().to_path_buf());
    let mut events = client.subscribe().await.unwrap();
    let root = project.to_str().unwrap();

    let created = cli_call(
        dir.path(),
        "create_thread",
        json!({"projectRoot": root, "provider": "go", "model": "test-model"}),
    );
    let id = created["id"].as_str().unwrap().to_owned();
    let change = next_change(&mut events).await;
    assert_eq!(change["kind"], "created");
    assert_eq!(change["thread_id"], id);
    assert_eq!(change["thread"]["id"], id);
    assert!(change["project_root"].as_str().unwrap().ends_with("repo"));

    client
        .call("rename_thread", json!({"threadId": id, "title": "Renamed"}))
        .await
        .unwrap();
    let change = next_change(&mut events).await;
    assert_eq!(
        (change["kind"].clone(), change["thread"]["title"].clone()),
        (json!("updated"), json!("Renamed"))
    );

    client
        .call(
            "set_thread_approval_mode",
            json!({"threadId": id, "mode": "yolo"}),
        )
        .await
        .unwrap();
    let change = next_change(&mut events).await;
    assert_eq!(change["kind"], "updated");
    assert_eq!(change["thread"]["approval_mode"], "yolo");

    // A failed call announces nothing; the next success is the next event.
    assert!(client
        .call("rename_thread", json!({"threadId": id, "title": ""}))
        .await
        .is_err());
    client
        .call("discard_thread", json!({"threadId": id}))
        .await
        .unwrap();
    let change = next_change(&mut events).await;
    assert_eq!(change["kind"], "removed");
    assert_eq!(change["thread_id"], id);
    assert!(change.get("thread").is_none());
    task.abort();
}
