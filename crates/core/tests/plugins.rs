use serde_json::json;
use themis_core::plugins::{PluginSpec, PluginStore};

#[test]
fn plugin_revisions_preserve_saved_prompts_and_reject_stale_edits() {
    let global = tempfile::tempdir().unwrap();
    let project = tempfile::tempdir().unwrap();
    let store = PluginStore::new(
        global.path().to_path_buf(),
        Some(project.path().to_path_buf()),
    );
    let spec: PluginSpec = serde_json::from_value(json!({"name":"review","skills":[{"id":"rust","name":"Rust review","description":"Review Rust","instructions":"Check ownership","allowedTools":[],"scripts":[]}]})).unwrap();
    let first = store.save("local", spec.clone(), None).unwrap();
    let id = first.skill_id("rust");
    let mut changed = spec;
    changed.skills[0].instructions = "Check ownership and lifetimes".into();
    let second = store
        .save("local", changed.clone(), Some(&first.revision))
        .unwrap();
    assert!(store.save("local", changed, Some(&first.revision)).is_err());
    let (text, skills, _) = store
        .resolve_prompt(&format!("Use [[skill:{id}]] now"), &[])
        .unwrap();
    assert_eq!(text, "Use Rust review now");
    assert!(skills[0].instructions.contains("Check ownership"));
    assert!(!skills[0].instructions.contains("lifetimes"));
    assert_ne!(second.revision, first.revision);
    store.set_enabled("local", "review", false).unwrap();
    assert!(store
        .resolve_prompt(&format!("[[skill:{id}]]"), &[])
        .is_err());
}

#[test]
fn invalid_plugin_paths_and_missing_references_are_rejected() {
    let dir = tempfile::tempdir().unwrap();
    let store = PluginStore::new(dir.path().into(), None);
    let spec: PluginSpec = serde_json::from_value(json!({"name":"../escape"})).unwrap();
    assert!(store.save("global", spec, None).is_err());
    assert!(store.resolve_prompt("[[skill:missing]]", &[]).is_err());
    assert!(store.resolve_prompt("[[skill:unfinished", &[]).is_err());
    assert_eq!(
        store.marketplaces().unwrap()[0].name,
        "claude-plugins-official"
    );
}

#[tokio::test]
async fn marketplace_import_keeps_resources_and_disables_executable_components() {
    let global = tempfile::tempdir().unwrap();
    let repo = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(repo.path().join(".claude-plugin")).unwrap();
    std::fs::write(
        repo.path().join(".claude-plugin/marketplace.json"),
        r#"{"plugins":[{"name":"review","source":"./review"}]}"#,
    )
    .unwrap();
    std::fs::create_dir_all(repo.path().join("review/skills/rust")).unwrap();
    std::fs::write(
        repo.path().join("review/skills/rust/SKILL.md"),
        "---\nname: rust\ndescription: Review Rust\n---\nCheck ownership.",
    )
    .unwrap();
    std::fs::write(
        repo.path().join("review/.mcp.json"),
        r#"{"mcpServers":{"docs":{"url":"https://example.com/mcp"}}}"#,
    )
    .unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::create_dir_all(repo.path().join("review/scripts")).unwrap();
        let script = repo.path().join("review/scripts/check.sh");
        std::fs::write(&script, "#!/bin/sh\nprintf plugin-script-ok").unwrap();
        std::fs::set_permissions(script, std::fs::Permissions::from_mode(0o755)).unwrap();
    }
    let store = PluginStore::new(global.path().into(), None);
    store
        .add_marketplace("fixture", repo.path().to_str().unwrap())
        .unwrap();
    let imported = store.install("global", "fixture", "review").await.unwrap();
    assert_eq!(imported.spec.skills[0].instructions, "Check ownership.");
    assert!(!imported.spec.mcp["docs"].enabled);
    assert!(imported.spec.files.contains_key("skills/rust/SKILL.md"));
    assert!(store
        .save("global", imported.spec.clone(), Some(&imported.revision))
        .is_err());
    let project = tempfile::tempdir().unwrap();
    store
        .materialize(std::slice::from_ref(&imported), project.path())
        .unwrap();
    assert!(project
        .path()
        .join(".themis/plugin-files")
        .join(imported.skill_id("resources"))
        .join("skills/rust/SKILL.md")
        .exists());
    #[cfg(unix)]
    {
        assert!(imported
            .spec
            .executable_files
            .contains(&"scripts/check.sh".to_owned()));
        let script = project
            .path()
            .join(".themis/plugin-files")
            .join(imported.skill_id("resources"))
            .join("scripts/check.sh");
        let output = std::process::Command::new(script).output().unwrap();
        assert!(output.status.success());
        assert_eq!(output.stdout, b"plugin-script-ok");
    }
    std::fs::write(
        repo.path().join("review/skills/rust/SKILL.md"),
        "---\nname: rust\n---\nUpdated review.",
    )
    .unwrap();
    let updated = store.install("global", "fixture", "review").await.unwrap();
    assert_eq!(updated.spec.skills[0].instructions, "Updated review.");
    std::fs::write(
        repo.path().join("review/skills/rust/SKILL.md"),
        "---\nunclosed",
    )
    .unwrap();
    assert!(store.install("global", "fixture", "review").await.is_err());
    assert_eq!(store.list().unwrap()[0].revision, updated.revision);
}

#[tokio::test]
async fn remote_mcp_initializes_lists_tools_and_denies_calls_before_transport() {
    use themis_core::{
        plugins::connections::{tools, McpServer},
        tools::{Approval, ApprovalHook, ToolAction},
    };
    use wiremock::{
        matchers::{body_partial_json, method},
        Mock, MockServer, ResponseTemplate,
    };
    let server = MockServer::start().await;
    Mock::given(method("POST")).and(body_partial_json(json!({"method":"initialize"}))).respond_with(ResponseTemplate::new(200).set_body_json(json!({"jsonrpc":"2.0","id":1,"result":{"protocolVersion":"2025-03-26","capabilities":{},"serverInfo":{"name":"fixture","version":"1"}}}))).mount(&server).await;
    Mock::given(body_partial_json(
        json!({"method":"notifications/initialized"}),
    ))
    .respond_with(ResponseTemplate::new(202))
    .mount(&server)
    .await;
    Mock::given(body_partial_json(json!({"method":"tools/list"}))).respond_with(ResponseTemplate::new(200).set_body_json(json!({"jsonrpc":"2.0","id":2,"result":{"tools":[{"name":"search","description":"Search docs","inputSchema":{"type":"object","properties":{}}}]}}))).mount(&server).await;
    let dir = tempfile::tempdir().unwrap();
    let config = McpServer {
        url: Some(server.uri()),
        enabled: true,
        ..Default::default()
    };
    struct StartupOnly;
    impl ApprovalHook for StartupOnly {
        fn approve(&self, action: &ToolAction) -> Approval {
            if action.tool.starts_with("mcp_start_") {
                Approval::AllowOnce
            } else {
                Approval::Deny
            }
        }
    }
    let tools = tools(
        "docs",
        &config,
        dir.path(),
        std::sync::Arc::new(StartupOnly),
    )
    .await
    .unwrap();
    assert!(tools[0].name().starts_with("mcp_"));
    assert!(tools[0].execute(json!({})).await.is_err());
    assert!(!server
        .received_requests()
        .await
        .unwrap()
        .iter()
        .any(|r| r.body_json::<serde_json::Value>().unwrap()["method"] == "tools/call"));
}

#[tokio::test]
async fn hooks_require_approval_and_can_block_before_events() {
    use themis_core::{
        plugins::{hooks, Hook},
        tools::{AllowAllHook, ApprovalHook, DenyAllHook},
    };
    let root = tempfile::tempdir().unwrap();
    let hook = Hook {
        name: "guard".into(),
        event: "BeforeToolCall".into(),
        command: if cfg!(windows) {
            "echo {\"block\":true}"
        } else {
            "printf '{\"block\":true}'"
        }
        .into(),
        enabled: true,
        timeout_seconds: 1,
        blocking: true,
        plugin_root: None,
        runtime_identity: None,
    };
    let deny: std::sync::Arc<dyn ApprovalHook> = std::sync::Arc::new(DenyAllHook);
    assert!(hooks::run(&hook, root.path(), &json!({}), &deny)
        .await
        .unwrap_err()
        .to_string()
        .contains("denied"));
    let allow: std::sync::Arc<dyn ApprovalHook> = std::sync::Arc::new(AllowAllHook);
    let error = hooks::run(
        &hook,
        root.path(),
        &json!({"input": "x".repeat(1024 * 1024)}),
        &allow,
    )
    .await
    .unwrap_err()
    .to_string();
    assert!(error.contains("blocked"), "{error}");
    let slow = Hook {
        command: if cfg!(windows) {
            "ping -n 4 127.0.0.1 >nul"
        } else {
            "sleep 3"
        }
        .into(),
        ..hook
    };
    assert!(hooks::run(&slow, root.path(), &json!({}), &allow)
        .await
        .unwrap_err()
        .to_string()
        .contains("timed out"));
}

#[tokio::test]
async fn mcp_startup_denial_prevents_process_execution() {
    use themis_core::{
        plugins::connections::{tools, McpServer},
        tools::DenyAllHook,
    };
    let dir = tempfile::tempdir().unwrap();
    let server = McpServer {
        command: Some("sh".into()),
        args: vec!["-c".into(), "touch forbidden".into()],
        enabled: true,
        ..Default::default()
    };
    assert!(tools(
        "fixture",
        &server,
        dir.path(),
        std::sync::Arc::new(DenyAllHook)
    )
    .await
    .unwrap_err()
    .to_string()
    .contains("startup denied"));
    assert!(!dir.path().join("forbidden").exists());
}

#[cfg(unix)]
#[tokio::test]
async fn hook_root_is_data_and_timeout_kills_descendants() {
    use themis_core::{
        plugins::{hooks, Hook},
        tools::{AllowAllHook, ApprovalHook},
    };
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("root $(touch injected)");
    std::fs::create_dir(&path).unwrap();
    let hook = Hook {
        name: "root".into(),
        event: "RunStart".into(),
        command: "test -d \"$CLAUDE_PLUGIN_ROOT\" && printf '{\"ok\":true}'".into(),
        enabled: true,
        timeout_seconds: 1,
        blocking: true,
        plugin_root: Some(path),
        runtime_identity: Some("g--personal--rev--root".into()),
    };
    let approvals: std::sync::Arc<dyn ApprovalHook> = std::sync::Arc::new(AllowAllHook);
    assert_eq!(
        hooks::run(
            &hook,
            dir.path(),
            &json!({"input": "x".repeat(1024 * 1024)}),
            &approvals
        )
        .await
        .unwrap()["ok"],
        true
    );
    assert!(!dir.path().join("injected").exists());
    let hook = Hook {
        command: "(sleep 2; touch survived) & wait".into(),
        ..hook
    };
    assert!(hooks::run(&hook, dir.path(), &json!({}), &approvals)
        .await
        .is_err());
    tokio::time::sleep(std::time::Duration::from_secs(2)).await;
    assert!(!dir.path().join("survived").exists());
}

#[test]
fn oversized_registry_update_preserves_readable_previous_revision() {
    let dir = tempfile::tempdir().unwrap();
    let store = PluginStore::new(dir.path().into(), None);
    let mut spec = PluginSpec {
        name: "resources".into(),
        ..Default::default()
    };
    spec.files
        .insert("data.txt".into(), "x".repeat(7 * 1024 * 1024));
    let mut current = store.save("global", spec.clone(), None).unwrap();
    let mut rejected = false;
    for _ in 0..5 {
        match store.save("global", spec.clone(), Some(&current.revision)) {
            Ok(next) => current = next,
            Err(error) => {
                assert!(error.to_string().contains("32 MiB"));
                rejected = true;
                break;
            }
        }
    }
    assert!(rejected);
    assert_eq!(store.list().unwrap()[0].revision, current.revision);
}

#[test]
fn duplicate_hook_names_cannot_share_an_approval_identity() {
    let dir = tempfile::tempdir().unwrap();
    let store = PluginStore::new(dir.path().into(), None);
    let hook = json!({"name":"guard","event":"RunStart","command":"true"});
    let spec = serde_json::from_value(json!({"name":"hooks","hooks":[hook.clone(),hook]})).unwrap();
    assert!(store
        .save("global", spec, None)
        .unwrap_err()
        .to_string()
        .contains("Duplicate hook"));
}
