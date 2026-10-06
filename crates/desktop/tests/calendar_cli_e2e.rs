//! Calendar schedule persistence and validation through the real shared server CLI.
use serde_json::{json, Value};
use std::path::Path;
use themis_desktop::{server::Server, state::AppState};

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
async fn calendar_cli_round_trip_restart_and_rejected_update() {
    let data = tempfile::tempdir().unwrap();
    let project = tempfile::tempdir().unwrap();
    let state = AppState::new_for_test(data.path().join("settings.json"));
    let server = Server::bind(state.clone(), data.path()).await.unwrap();
    let task = tokio::spawn(server.run());
    let input = json!({
        "name":"Morning review", "project_root":project.path(), "provider":"go",
        "model":"fixture", "skill_ids":[], "interval_mins":0,
        "task":"Review recent changes", "enabled":true,
        "schedule":{"repeat":"weekdays", "time":"09:00", "timezone":"Europe/Berlin", "weekday":0}
    });
    let created = success(data.path(), "create_automation", json!({"input":input}));
    assert_eq!(created["interval_mins"], 60);
    let id = created["id"].as_str().unwrap();
    let next =
        chrono::DateTime::parse_from_rfc3339(created["next_run_at"].as_str().unwrap()).unwrap();
    assert!(next > chrono::Utc::now());
    let local = next.with_timezone(&chrono_tz::Europe::Berlin);
    assert_eq!(local.format("%H:%M").to_string(), "09:00");
    use chrono::Datelike;
    assert!(local.weekday().num_days_from_monday() < 5);

    let mut invalid = input.clone();
    invalid["schedule"]["timezone"] = json!("Unknown/Zone");
    let rejected = call(
        data.path(),
        "update_automation",
        json!({"automationId":id,"input":invalid}),
    );
    assert!(!rejected.status.success());
    assert!(String::from_utf8_lossy(&rejected.stderr).contains("time zone"));
    assert_eq!(
        success(data.path(), "list_automations", json!({}))[0],
        created
    );

    let mut legacy = input.clone();
    legacy.as_object_mut().unwrap().remove("schedule");
    legacy["name"] = json!("Legacy interval");
    legacy["interval_mins"] = json!(17);
    let legacy_created = success(data.path(), "create_automation", json!({"input":legacy}));
    assert!(legacy_created.get("schedule").is_none());
    // Stop the server and rebuild AppState from persisted files.
    let output = std::process::Command::new(env!("CARGO_BIN_EXE_themis"))
        .arg("--data-dir")
        .arg(data.path())
        .args(["server", "stop"])
        .output()
        .unwrap();
    assert!(output.status.success());
    task.await.unwrap().unwrap();
    drop(state);
    let rebooted = AppState::new_for_test(data.path().join("settings.json"));
    let automations = rebooted.list_automations().await;
    let calendar = automations
        .iter()
        .find(|automation| automation.id == id)
        .unwrap();
    assert_eq!(serde_json::to_value(calendar).unwrap(), created);
    let legacy = automations
        .iter()
        .find(|automation| automation.id == legacy_created["id"].as_str().unwrap())
        .unwrap();
    assert_eq!(legacy.interval_mins, 17);
    assert!(legacy.schedule.is_none());
}
