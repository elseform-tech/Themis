//! Watch fields, limits and removal reasons through the real shared server CLI.
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
async fn watch_fields_validation_and_removal_reason_through_themis_call() {
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

    let expires = (chrono::Utc::now() + chrono::Duration::hours(1)).to_rfc3339();
    let watch = |extra: Value| {
        let mut input = json!({
            "name":"Watch status", "project_root":project.path(), "target_thread_id": thread.id,
            "provider":"go", "model":"", "skill_ids":[], "interval_mins":2,
            "task":"Report status.txt", "enabled":true,
            "stop_when":"status.txt says DONE", "expires_at": expires,
            "max_runs": 30, "check_command":["cat","status.txt"]
        });
        for (key, value) in extra.as_object().unwrap() {
            input[key] = value.clone();
        }
        json!({"input": input})
    };

    // Limits are validated by the shared server, not just the tool.
    for (extra, message) in [
        (json!({"expires_at": "2001-01-01T00:00:00Z"}), "future"),
        (json!({"max_runs": 0}), "max_runs"),
        (json!({"check_command": [" "]}), "check_command"),
    ] {
        let output = call(data.path(), "create_automation", watch(extra));
        assert!(!output.status.success());
        assert!(
            String::from_utf8_lossy(&output.stderr).contains(message),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
    }

    let created = success(data.path(), "create_automation", watch(json!({})));
    assert_eq!(created["interval_mins"], 2);
    assert_eq!(created["stop_when"], "status.txt says DONE");
    assert_eq!(created["max_runs"], 30);
    assert_eq!(created["check_command"], json!(["cat", "status.txt"]));
    assert!(
        created.get("origin").is_none(),
        "created through the CLI = user"
    );

    let tasks = success(
        data.path(),
        "get_thread_tasks",
        json!({"threadId": thread.id}),
    );
    let listed = &tasks["automations"][0];
    assert_eq!(listed["stop_when"], "status.txt says DONE");
    assert_eq!(listed["expires_at"], created["expires_at"]);
    assert_eq!(tasks["removed"], json!([]));

    success(
        data.path(),
        "delete_automation",
        json!({"automationId": created["id"]}),
    );
    let after = success(
        data.path(),
        "get_thread_tasks",
        json!({"threadId": thread.id}),
    );
    assert_eq!(after["automations"], json!([]));
    assert_eq!(after["removed"][0]["reason"], "deleted");
    assert_eq!(after["removed"][0]["name"], "Watch status");
    assert_eq!(after["removed"][0]["stop_when"], "status.txt says DONE");

    let stop = std::process::Command::new(env!("CARGO_BIN_EXE_themis"))
        .arg("--data-dir")
        .arg(data.path())
        .args(["server", "stop"])
        .output()
        .unwrap();
    assert!(stop.status.success());
    task.await.unwrap().unwrap();
}
