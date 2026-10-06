//! Local import formats and lifecycle through the real shared server and CLI.
use serde_json::{json, Value};
use std::path::Path;
use themis_desktop::{
    server::{Client, Server},
    state::AppState,
};

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
#[tokio::test(flavor = "multi_thread")]
async fn imports_skill_folder_and_mcp_json_then_toggles_and_uninstalls() {
    let data = tempfile::tempdir().unwrap();
    let project = tempfile::tempdir().unwrap();
    let state = AppState::new_for_test(data.path().join("settings.json"));
    let server = Server::bind(state.clone(), data.path()).await.unwrap();
    let task = tokio::spawn(server.run());
    let folder = project.path().join("review");
    std::fs::create_dir(&folder).unwrap();
    std::fs::write(folder.join("SKILL.md"), "---\nname: review\ndescription: Inspect changes\n---\nInspect ownership and report evidence.\n").unwrap();
    let imported = cli(
        data.path(),
        &["skill", "import", folder.to_str().unwrap(), "reviews"],
    );
    assert_eq!(imported["spec"]["package_kind"], "skill");
    assert_eq!(imported["spec"]["skills"][0]["id"], "review");
    assert!(imported["spec"]["skills"][0]["instructions"]
        .as_str()
        .unwrap()
        .contains("Inspect ownership"));
    cli(data.path(), &["skill", "disable", "reviews", "review"]);
    assert!(!state
        .prompt_skills(None)
        .await
        .unwrap()
        .iter()
        .any(|s| s.name == "review"));
    cli(data.path(), &["skill", "enable", "reviews", "review"]);
    assert!(state
        .prompt_skills(None)
        .await
        .unwrap()
        .iter()
        .any(|s| s.name == "review"));
    let config = project.path().join("mcp.json");
    std::fs::write(
        &config,
        json!({"mcpServers":{"fixture":{"command":"printf","args":["fixture"]}}}).to_string(),
    )
    .unwrap();
    let connection = cli(
        data.path(),
        &["mcp", "import", config.to_str().unwrap(), "connections"],
    );
    assert_eq!(connection["spec"]["package_kind"], "mcp");
    assert_eq!(connection["spec"]["mcp"]["fixture"]["command"], "printf");
    assert!(connection["spec"]["skills"].as_array().unwrap().is_empty());
    cli(data.path(), &["mcp", "disable", "connections", "fixture"]);
    let shown = cli(data.path(), &["plugin", "show", "connections"]);
    assert_eq!(shown["spec"]["mcp"]["fixture"]["enabled"], false);
    let client = Client::new(data.path().into());
    std::fs::create_dir(project.path().join(".claude-plugin")).unwrap();
    std::fs::write(
        project.path().join(".claude-plugin/marketplace.json"),
        json!({"plugins":[{"name":"preview-only","source":"./review"}]}).to_string(),
    )
    .unwrap();
    client
        .call(
            "plugin_action",
            json!({"action":"add_marketplace","name":"preview-fixture","source":project.path()}),
        )
        .await
        .unwrap();
    let preview = cli(
        data.path(),
        &[
            "call",
            "plugin_action",
            r#"{"action":"preview","marketplace":"preview-fixture","name":"preview-only"}"#,
        ],
    );
    assert_eq!(preview["package_kind"], "plugin");
    assert_eq!(preview["skills"][0]["id"], "review");
    assert!(preview["files"]["SKILL.md"]
        .as_str()
        .unwrap()
        .contains("Inspect ownership"));
    assert!(!cli(data.path(), &["plugin", "list"])
        .as_array()
        .unwrap()
        .iter()
        .any(|plugin| plugin["spec"]["name"] == "preview-only"));
    cli(data.path(), &["mcp", "delete", "connections", "fixture"]);
    assert!(
        cli(data.path(), &["plugin", "show", "connections"])["spec"]["mcp"]
            .as_object()
            .unwrap()
            .is_empty()
    );
    let pasted = client.call("plugin_action", json!({"action":"import_json","content":json!({"spec":{"name":"pasted","skills":[]}}).to_string(),"scope":"global"})).await.unwrap();
    assert_eq!(pasted["spec"]["name"], "pasted");
    let malformed = client.call("plugin_action", json!({"action":"import_json","content":"{invalid","scope":"global","name":"connections"})).await;
    assert!(malformed.is_err());
    assert!(
        cli(data.path(), &["plugin", "show", "connections"])["spec"]["mcp"]
            .as_object()
            .unwrap()
            .is_empty()
    );
    cli(data.path(), &["plugin", "uninstall", "reviews"]);
    assert!(!state
        .prompt_skills(None)
        .await
        .unwrap()
        .iter()
        .any(|s| s.name == "review"));
    for args in [
        vec!["init", "-q"],
        vec!["add", "SKILL.md"],
        vec![
            "-c",
            "user.name=Fixture",
            "-c",
            "user.email=fixture@local",
            "commit",
            "-qm",
            "fixture",
        ],
    ] {
        let output = std::process::Command::new("git")
            .args(args)
            .current_dir(&folder)
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
    }
    let repository = cli(
        data.path(),
        &["skill", "import", folder.to_str().unwrap(), "--ref", "HEAD"],
    );
    assert_eq!(repository["spec"]["name"], "review");
    Client::new(data.path().into())
        .call("shutdown", json!({}))
        .await
        .unwrap();
    task.await.unwrap().unwrap();
}

#[test]
fn hook_child_cli_cannot_start_a_nested_run() {
    let data = tempfile::tempdir().unwrap();
    let output = std::process::Command::new(env!("CARGO_BIN_EXE_themis"))
        .arg("--data-dir")
        .arg(data.path())
        .args(["thread", "send", "unused", "fixture"])
        .env("THEMIS_HOOK_ACTIVE", "1")
        .output()
        .unwrap();
    assert!(!output.status.success());
    assert!(
        String::from_utf8_lossy(&output.stderr).contains("Hooks cannot start nested agent runs")
    );
    assert!(!data.path().join("server.json").exists());
}

#[tokio::test(flavor = "multi_thread")]
async fn repository_preview_preserves_full_skill_without_installing_then_imports() {
    let data = tempfile::tempdir().unwrap();
    let repository = tempfile::tempdir().unwrap();
    let skill_dir = repository.path().join("skills/review");
    std::fs::create_dir_all(skill_dir.join("references")).unwrap();
    let markdown = "---\nname: review\ndescription: Review source changes\n---\n# Review workflow\n\nKeep this complete Markdown, including frontmatter and Unicode: résumé.\nRead [supporting notes](references/notes.md) when relevant.\n";
    std::fs::write(skill_dir.join("SKILL.md"), markdown).unwrap();
    std::fs::write(
        skill_dir.join("references/notes.md"),
        "Preserve this supporting resource.\n",
    )
    .unwrap();
    for args in [
        vec!["init", "-q"],
        vec!["add", "."],
        vec![
            "-c",
            "user.name=Fixture",
            "-c",
            "user.email=fixture@local",
            "-c",
            "commit.gpgsign=false",
            "-c",
            "core.hooksPath=/dev/null",
            "commit",
            "-qm",
            "fixture",
        ],
    ] {
        let output = std::process::Command::new("git")
            .args(args)
            .current_dir(repository.path())
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
    }
    let revision = std::process::Command::new("git")
        .args(["rev-parse", "HEAD"])
        .current_dir(repository.path())
        .output()
        .unwrap();
    assert!(revision.status.success());
    let reference = String::from_utf8(revision.stdout)
        .unwrap()
        .trim()
        .to_owned();
    let state = AppState::new_for_test(data.path().join("settings.json"));
    let server = Server::bind(state, data.path()).await.unwrap();
    let serving = tokio::spawn(server.run());
    let before = cli(data.path(), &["plugin", "list"]);
    let mut args = json!({"action":"preview_repository","name":"review-preview","url":repository.path(),"reference":reference,"subdirectory":"skills/review","scope":"global"});
    let preview = cli(data.path(), &["call", "plugin_action", &args.to_string()]);
    assert_eq!(preview["files"]["SKILL.md"], markdown);
    assert_eq!(
        preview["files"]["references/notes.md"],
        "Preserve this supporting resource.\n"
    );
    assert_eq!(preview["skills"][0]["id"], "review");
    assert_eq!(preview["skill_paths"]["review"], "SKILL.md");
    assert_eq!(preview["origin"]["reference"], reference);
    assert_eq!(
        cli(data.path(), &["plugin", "list"]),
        before,
        "Preview must leave the registry unchanged"
    );
    args["action"] = "import_repository".into();
    let installed = cli(data.path(), &["call", "plugin_action", &args.to_string()]);
    assert_eq!(installed["spec"]["files"]["SKILL.md"], markdown);
    assert_eq!(
        installed["spec"]["files"]["references/notes.md"],
        preview["files"]["references/notes.md"]
    );
    assert_eq!(installed["spec"]["name"], "review-preview");
    assert_eq!(installed["spec"]["package_kind"], "skill");
    assert_eq!(installed["spec"]["skill_paths"]["review"], "SKILL.md");
    assert!(cli(data.path(), &["plugin", "list"])
        .as_array()
        .unwrap()
        .iter()
        .any(|plugin| plugin["revision"] == installed["revision"]));
    Client::new(data.path().into())
        .call("shutdown", json!({}))
        .await
        .unwrap();
    serving.await.unwrap().unwrap();
}
