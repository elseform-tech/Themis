//! CLI integration tests: `themis` binary against a wiremock LLM.
//!
//! No real network. The binary under test comes from `CARGO_BIN_EXE_themis`.

mod common;

use std::process::Command;

use serde_json::json;
use wiremock::MockServer;

#[tokio::test]
async fn cli_run_writes_file_and_exits_zero() {
    let server = MockServer::start().await;
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().join("proj");
    std::fs::create_dir(&root).unwrap();
    std::fs::write(root.join("hello.txt"), "before\n").unwrap();

    common::mount_script(
        &server,
        vec![
            common::tool_call_body(
                "call_1",
                "write_file",
                json!({"file_path": "hello.txt", "content": "via cli\n", "append": false}),
            ),
            common::final_text_body("all done"),
        ],
    )
    .await;

    let output = Command::new(env!("CARGO_BIN_EXE_themis"))
        .arg("run")
        .arg("replace hello.txt with the cli greeting")
        .arg("--project")
        .arg(&root)
        .arg("--provider")
        .arg("go")
        .arg("--model")
        .arg("test-model")
        .arg("--base-url")
        .arg(server.uri())
        .arg("--api-key")
        .arg("test-key")
        .arg("--yes")
        .arg("--max-turns")
        .arg("5")
        .output()
        .unwrap();

    assert!(
        output.status.success(),
        "exit: {:?}\nstdout: {}\nstderr: {}",
        output.status.code(),
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(
        std::fs::read_to_string(root.join("hello.txt")).unwrap(),
        "via cli\n"
    );
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("finished: all done"), "stdout: {stdout}");
}

#[tokio::test]
async fn cli_skills_file_applies_and_materializes() {
    let server = MockServer::start().await;
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().join("proj");
    std::fs::create_dir(&root).unwrap();
    std::fs::write(root.join("hello.txt"), "before\n").unwrap();

    common::mount_script(
        &server,
        vec![
            common::tool_call_body(
                "call_1",
                "write_file",
                json!({"file_path": "hello.txt", "content": "via skill\n", "append": false}),
            ),
            common::final_text_body("skilled done"),
        ],
    )
    .await;

    let skills_path = tmp.path().join("skills.json");
    std::fs::write(
        &skills_path,
        serde_json::to_string(&serde_json::json!([{
            "id": "writer",
            "name": "Writer",
            "description": "writes files",
            "instructions": "Always be concise.",
            "allowedTools": ["write_file", "read_file"],
            "scripts": [{"name": "greet.sh", "content": "echo hi"}],
        }]))
        .unwrap(),
    )
    .unwrap();

    let output = Command::new(env!("CARGO_BIN_EXE_themis"))
        .arg("run")
        .arg("replace hello.txt with the skilled greeting")
        .arg("--project")
        .arg(&root)
        .arg("--provider")
        .arg("go")
        .arg("--model")
        .arg("test-model")
        .arg("--base-url")
        .arg(server.uri())
        .arg("--api-key")
        .arg("test-key")
        .arg("--yes")
        .arg("--max-turns")
        .arg("5")
        .arg("--skills-file")
        .arg(&skills_path)
        .output()
        .unwrap();

    assert!(
        output.status.success(),
        "exit: {:?}\nstdout: {}\nstderr: {}",
        output.status.code(),
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(
        std::fs::read_to_string(root.join("hello.txt")).unwrap(),
        "via skill\n"
    );
    assert_eq!(
        std::fs::read_to_string(
            root.join(".themis")
                .join("skills")
                .join("writer")
                .join("greet.sh")
        )
        .unwrap(),
        "echo hi"
    );
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        stdout.contains("finished: skilled done"),
        "stdout: {stdout}"
    );
}

#[tokio::test]
async fn cli_skills_file_missing_or_invalid_errors() {
    let tmp = tempfile::tempdir().unwrap();

    let output = Command::new(env!("CARGO_BIN_EXE_themis"))
        .arg("run")
        .arg("do nothing")
        .arg("--project")
        .arg(tmp.path())
        .arg("--provider")
        .arg("go")
        .arg("--model")
        .arg("test-model")
        .arg("--api-key")
        .arg("test-key")
        .arg("--yes")
        .arg("--skills-file")
        .arg(tmp.path().join("nonexistent.json"))
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(2));
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("--skills-file"), "stderr: {stderr}");

    let bad = tmp.path().join("bad.json");
    std::fs::write(&bad, "{not valid json").unwrap();
    let output = Command::new(env!("CARGO_BIN_EXE_themis"))
        .arg("run")
        .arg("do nothing")
        .arg("--project")
        .arg(tmp.path())
        .arg("--provider")
        .arg("go")
        .arg("--model")
        .arg("test-model")
        .arg("--api-key")
        .arg("test-key")
        .arg("--yes")
        .arg("--skills-file")
        .arg(&bad)
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(2));
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("invalid --skills-file"), "stderr: {stderr}");
}

#[test]
fn cli_help_exits_zero() {
    let output = Command::new(env!("CARGO_BIN_EXE_themis"))
        .arg("--help")
        .output()
        .unwrap();
    assert!(output.status.success());
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("themis run"), "stdout: {stdout}");
    assert!(stdout.contains("--skills-file"), "stdout: {stdout}");
}
