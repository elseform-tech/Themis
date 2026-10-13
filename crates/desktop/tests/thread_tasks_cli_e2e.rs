//! Per-chat task feed and automation pause through the real shared server CLI.
use serde_json::{json, Value};
use std::path::Path;
use themis_desktop::{server::Server, state::AppState, types::ProviderKind};

fn call(data: &Path, command: &str, args: Value) -> std::process::Output {
    std::process::Command::new(env!("CARGO_BIN_EXE_themis"))
        .arg("--data-dir")
        .arg(data)
        .args(["call", command, &args.to_string()])
        .env_remove("OPENCODE_KEY")
        .output()
        .expect("CLI")
}
fn success(data: &Path, command: &str, args: Value) -> Value {
    let output = call(data, command, args);
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    serde_json::from_slice(&output.stdout).expect("JSON")
}

#[tokio::test(flavor = "multi_thread")]
async fn thread_tasks_feed_lists_chat_automations_and_pause_persists() {
    let data = tempfile::tempdir().unwrap();
    let project = tempfile::tempdir().unwrap();
    let state = AppState::new_for_test(data.path().join("settings.json"));
    let thread = state
        .create_thread(
            project.path().to_string_lossy().into(),
            ProviderKind::Go,
            Some("fixture".to_owned()),
        )
        .await
        .unwrap();
    let server = Server::bind(state.clone(), data.path()).await.unwrap();
    let task = tokio::spawn(server.run());

    let empty = success(
        data.path(),
        "get_thread_tasks",
        json!({"threadId": thread.id}),
    );
    assert_eq!(
        empty["run"],
        json!({"running": false, "awaiting_approval": false})
    );
    assert_eq!(empty["automations"], json!([]));
    assert_eq!(empty["recent"], json!([]));

    let input = json!({
        "name":"Morning review", "project_root":project.path(), "target_thread_id": thread.id,
        "provider":"go", "model":"", "skill_ids":[], "interval_mins":0,
        "task":"Review recent changes", "enabled":true,
        "schedule":{"repeat":"daily", "time":"09:00", "timezone":"Europe/Berlin", "weekday":0}
    });
    let created = success(data.path(), "create_automation", json!({"input": input}));
    let other = json!({
        "name":"Elsewhere", "project_root":project.path(), "provider":"go", "model":"fixture",
        "skill_ids":[], "interval_mins":30, "task":"Other", "enabled":true
    });
    success(data.path(), "create_automation", json!({"input": other}));

    let tasks = success(
        data.path(),
        "get_thread_tasks",
        json!({"threadId": thread.id}),
    );
    let listed = tasks["automations"].as_array().unwrap();
    assert_eq!(listed.len(), 1, "only this chat's automations: {tasks}");
    assert_eq!(listed[0]["id"], created["id"]);
    assert_eq!(listed[0]["enabled"], true);
    assert_eq!(listed[0]["origin"], "user");
    assert_eq!(listed[0]["next_run_at"], created["next_run_at"]);
    assert_eq!(listed[0]["running"], false);

    success(
        data.path(),
        "set_automation_enabled",
        json!({"automationId": created["id"], "enabled": false}),
    );
    let paused = success(
        data.path(),
        "get_thread_tasks",
        json!({"threadId": thread.id}),
    );
    assert_eq!(paused["automations"][0]["enabled"], false);

    let missing = call(data.path(), "get_thread_tasks", json!({"threadId": "nope"}));
    assert!(!missing.status.success());

    let stop = std::process::Command::new(env!("CARGO_BIN_EXE_themis"))
        .arg("--data-dir")
        .arg(data.path())
        .args(["server", "stop"])
        .output()
        .unwrap();
    assert!(stop.status.success());
    task.await.unwrap().unwrap();
}
