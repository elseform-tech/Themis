use serde_json::{json, Value};
use std::path::Path;
use themis_desktop::{
    server::{Client, Server},
    state::AppState,
};

fn cli(dir: &Path, args: &[&str]) -> Value {
    let out = std::process::Command::new(env!("CARGO_BIN_EXE_themis"))
        .arg("--data-dir")
        .arg(dir)
        .args(args)
        .env_remove("OPENCODE_KEY")
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    serde_json::from_slice(&out.stdout).unwrap()
}
#[tokio::test(flavor = "multi_thread")]
async fn cli_plugin_crud_is_visible_to_app_and_preserves_pinned_skills() {
    let data = tempfile::tempdir().unwrap();
    let project = tempfile::tempdir().unwrap();
    let state = AppState::new_for_test(data.path().join("settings.json"));
    let server = Server::bind(state.clone(), data.path()).await.unwrap();
    let task = tokio::spawn(server.run());
    let created = cli(
        data.path(),
        &[
            "plugin",
            "create",
            "personal",
            "--scope",
            "local",
            "--project",
            project.path().to_str().unwrap(),
        ],
    );
    assert_eq!(created["spec"]["name"], "personal");
    let draft = project.path().join("review.json");
    std::fs::write(&draft,r#"{"id":"review","name":"Review","description":"Inspect changes","instructions":"Inspect ownership","allowedTools":[],"scripts":[]}"#).unwrap();
    cli(
        data.path(),
        &[
            "skill",
            "save",
            "personal",
            draft.to_str().unwrap(),
            "--scope",
            "local",
            "--project",
            project.path().to_str().unwrap(),
        ],
    );
    let skills = state
        .prompt_skills(Some(project.path().to_str().unwrap().into()))
        .await
        .unwrap();
    assert!(skills.iter().any(|s| s.name == "Review"));
    cli(
        data.path(),
        &[
            "plugin",
            "disable",
            "personal",
            "--scope",
            "local",
            "--project",
            project.path().to_str().unwrap(),
        ],
    );
    assert!(!state
        .prompt_skills(Some(project.path().to_str().unwrap().into()))
        .await
        .unwrap()
        .iter()
        .any(|s| s.name == "Review"));
    Client::new(data.path().into())
        .call("shutdown", json!({}))
        .await
        .unwrap();
    task.await.unwrap().unwrap();
}
