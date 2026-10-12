//! Deleting a project through the shared app server and CLI moves its folder
//! to the Trash and forgets its chats, while refusing unsafe targets.
use serde_json::{json, Value};
use std::path::Path;
use themis_desktop::{
    server::{Client, Server},
    state::AppState,
};

fn cli(dir: &Path, command: &str, args: Value) -> Value {
    let output = std::process::Command::new(env!("CARGO_BIN_EXE_themis"))
        .arg("--data-dir")
        .arg(dir)
        .args(["call", command, &args.to_string()])
        .env_remove("OPENCODE_KEY")
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    serde_json::from_slice(&output.stdout).unwrap_or(Value::Null)
}

#[tokio::test(flavor = "multi_thread")]
async fn cli_delete_project_moves_it_to_trash_and_forgets_its_chats() {
    let data = tempfile::tempdir().unwrap();
    let outside = tempfile::tempdir().unwrap();
    let state = AppState::new_for_test(data.path().join("settings.json"));
    let server = Server::bind(state, data.path()).await.unwrap();
    let serving = tokio::spawn(server.run());
    let client = Client::new(data.path().into());

    let project = cli(data.path(), "create_project", json!({"name": "Scratch"}));
    let root = project["root"].as_str().unwrap().to_owned();
    std::fs::write(Path::new(&root).join("notes.txt"), "keep me").unwrap();
    let thread = cli(
        data.path(),
        "create_thread",
        json!({"projectRoot": root, "provider": "go", "model": "muse-spark-1.3-contributor"}),
    );
    assert_eq!(
        cli(data.path(), "list_threads", json!({"projectRoot": root}))
            .as_array()
            .unwrap()
            .len(),
        1
    );

    // Unsafe targets are refused and left untouched.
    let default = cli(data.path(), "get_default_project", json!({}));
    for (path, reason) in [
        (default["root"].as_str().unwrap(), "built-in"),
        (outside.path().to_str().unwrap(), "projects folder"),
    ] {
        let error = client
            .call("delete_project", json!({"path": path}))
            .await
            .unwrap_err();
        assert!(error.to_string().contains(reason), "{error}");
        assert!(Path::new(path).exists());
    }

    cli(data.path(), "delete_project", json!({"path": root}));
    assert!(
        !Path::new(&root).exists(),
        "the folder leaves the projects folder"
    );
    assert_eq!(
        std::fs::read_to_string(data.path().join("trash/Scratch/notes.txt")).unwrap(),
        "keep me",
        "files are recoverable from the Trash"
    );
    let error = client
        .call("get_thread", json!({"threadId": thread["id"]}))
        .await
        .unwrap_err();
    assert!(error.to_string().contains("unknown thread"), "{error}");
    let settings = cli(data.path(), "get_settings", json!({}));
    assert!(!settings["recent_roots"]
        .as_array()
        .unwrap()
        .iter()
        .any(|recent| recent == &json!(root)));

    client.call("shutdown", json!({})).await.unwrap();
    serving.await.unwrap().unwrap();
}
