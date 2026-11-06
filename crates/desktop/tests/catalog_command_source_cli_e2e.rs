//! A catalog entry whose source is a command (Apple's `xcode` plugin is found by running
//! `xcrun`), through the real shared server and CLI. A fake `xcrun` first on PATH stands in for
//! Xcode, so the allowlisted command itself runs.
#![cfg(unix)]

use serde_json::{json, Value};
use std::os::unix::fs::PermissionsExt;
use std::path::Path;
use themis_desktop::{server::Server, state::AppState};

fn cli(data: &Path, args: &[&str]) -> Value {
    let output = std::process::Command::new(env!("CARGO_BIN_EXE_themis"))
        .arg("--data-dir")
        .arg(data)
        .args(args)
        .env_remove("OPENCODE_KEY")
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    serde_json::from_slice(&output.stdout).unwrap()
}

fn write(path: &Path, text: &str) {
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(path, text).unwrap();
}

#[tokio::test(flavor = "multi_thread")]
async fn an_allowlisted_command_source_installs_and_any_other_command_never_runs() {
    let data = tempfile::tempdir().unwrap();
    let scratch = tempfile::tempdir().unwrap();
    // The plugin as Xcode ships it, somewhere outside any catalog.
    let plugin = scratch.path().join("Xcode.app/plugin");
    write(
        &plugin.join(".claude-plugin/plugin.json"),
        &json!({"name": "xcode", "version": "1.0.0", "description": "Xcode tools"}).to_string(),
    );
    write(
        &plugin.join("skills/build/SKILL.md"),
        "---\nname: build\ndescription: Build a scheme\n---\nRun the scheme's build.",
    );
    // A stand-in for xcrun that answers only the allowlisted question.
    let bin = scratch.path().join("bin");
    write(
        &bin.join("xcrun"),
        &format!(
            "#!/bin/sh\n[ \"$*\" = \"agent plugin path --plugin-format claude\" ] || exit 9\nprintf '%s\\n' '{}'\n",
            plugin.display()
        ),
    );
    std::fs::set_permissions(bin.join("xcrun"), std::fs::Permissions::from_mode(0o755)).unwrap();
    let path = std::env::var("PATH").unwrap_or_default();
    // This file holds one test, so changing the process's PATH affects nothing else.
    std::env::set_var("PATH", format!("{}:{path}", bin.display()));

    let marker = scratch.path().join("ran");
    let catalog = scratch.path().join("catalog");
    write(
        &catalog.join(".claude-plugin/marketplace.json"),
        &json!({"plugins": [
            {"name": "xcode", "source": {"source": "command",
                "command": "xcrun agent plugin path --plugin-format claude", "mode": "link"}},
            {"name": "sneaky", "source": {"source": "command",
                "command": format!("touch {}", marker.display())}}
        ]})
        .to_string(),
    );

    let state = AppState::new_for_test(data.path().join("settings.json"));
    let task = tokio::spawn(Server::bind(state, data.path()).await.unwrap().run());
    cli(
        data.path(),
        &[
            "plugin",
            "marketplace",
            "add",
            catalog.to_str().unwrap(),
            "local",
        ],
    );
    let report = cli(data.path(), &["plugin", "marketplace", "check", "local"]);
    assert_eq!(
        report["statuses"],
        json!({"failed": 1, "supported": 1}),
        "{report}"
    );
    let reasons = report["reasons"].to_string();
    assert!(reasons.contains("not one Themis runs"), "{report}");
    assert!(
        !marker.exists(),
        "a command off the allowlist must never run"
    );

    let installed = cli(data.path(), &["plugin", "install", "xcode@local"]);
    assert_eq!(installed["spec"]["name"], "xcode");
    assert_eq!(installed["spec"]["skills"][0]["id"], "build");
    assert!(!marker.exists());
    task.abort();
}
