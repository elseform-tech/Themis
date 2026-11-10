use serde_json::json;
use themis_core::plugins::{PluginSpec, PluginStore};

#[test]
fn init_is_a_short_built_in_skill_described_by_its_frontmatter() {
    const SKILL: &str = include_str!("../builtins/init/SKILL.md");
    assert!(SKILL.len() < 2048, "{} bytes", SKILL.len());
    for needed in [
        "## Product",
        "## Project structure",
        "## Build, test and lint",
        "## Coding conventions",
        "## Security",
        "## How to work",
        "bounded reads",
        "keep the person's own rules",
        "CLAUDE.md",
        "`.env`",
        "1,500 tokens",
        "12 KB",
    ] {
        assert!(SKILL.contains(needed), "{needed}");
    }
    let global = tempfile::tempdir().unwrap();
    let store = PluginStore::new(global.path().to_path_buf(), None);
    let init = store
        .available_skills()
        .unwrap()
        .into_iter()
        .find(|skill| skill.id == "init")
        .expect("built-in init skill");
    assert_eq!(init.scope, "built-in");
    assert_eq!(init.name, "Init");
    // A Windows checkout may give the file CRLF line endings.
    assert!(SKILL
        .replace("\r\n", "\n")
        .contains(&format!("\ndescription: {}\n", init.description)));
    let (text, skills, _) = store.resolve_prompt("[[skill:init]]", &[]).unwrap();
    assert_eq!(text, "Init");
    assert_eq!(skills[0].instructions, SKILL);
}

#[test]
fn subagents_is_a_built_in_skill_that_lists_or_hands_over_the_task() {
    const SKILL: &str = include_str!("../builtins/subagents/SKILL.md");
    for needed in [
        "`themis plugin list --kind subagent`",
        "`read_only`",
        "`model`",
        "write_paths",
        "themis call get_subagent_settings",
        "max_parallel",
        "Settings › Subagents",
        "change nothing",
        "Always finish with one line that starts \"Try:\"",
        "spawn_subagents",
        "rather than doing it yourself",
        "check what they claim",
    ] {
        assert!(SKILL.contains(needed), "{needed}");
    }
    let global = tempfile::tempdir().unwrap();
    let store = PluginStore::new(global.path().to_path_buf(), None);
    let listed = store
        .available_skills()
        .unwrap()
        .into_iter()
        .find(|skill| skill.id == "subagents")
        .expect("built-in subagents skill");
    assert_eq!(listed.scope, "built-in");
    assert_eq!(listed.name, "Subagents");
    assert!(listed.description.contains("/subagents"));
    let (text, skills, _) = store
        .resolve_prompt("[[skill:subagents]] review the tests", &[])
        .unwrap();
    assert_eq!(text, "Subagents review the tests");
    assert_eq!(skills[0].instructions, SKILL);
}

#[test]
fn plugin_revisions_preserve_saved_prompts_and_reject_stale_edits() {
    let global = tempfile::tempdir().unwrap();
    let project = tempfile::tempdir().unwrap();
    let store = PluginStore::new(
        global.path().to_path_buf(),
        Some(project.path().to_path_buf()),
    );
    let spec: PluginSpec = serde_json::from_value(json!({"name":"review","skills":[{"id":"rust","name":"Rust review","description":"Review Rust","instructions":"Check ownership","allowedTools":[],"scripts":[]}]})).unwrap();
    let first = store.save("global", spec.clone(), None).unwrap();
    let id = first.skill_id("rust");
    let mut changed = spec;
    changed.skills[0].instructions = "Check ownership and lifetimes".into();
    let second = store
        .save("global", changed.clone(), Some(&first.revision))
        .unwrap();
    assert!(store
        .save("global", changed, Some(&first.revision))
        .is_err());
    let (text, skills, _) = store
        .resolve_prompt(&format!("Use [[skill:{id}]] now"), &[])
        .unwrap();
    assert_eq!(text, "Use Rust review now");
    assert!(skills[0].instructions.contains("Check ownership"));
    assert!(!skills[0].instructions.contains("lifetimes"));
    assert_ne!(second.revision, first.revision);
    store.set_enabled("global", "review", false).unwrap();
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

/// A person's install while a background check holds the marketplace waits for the check,
/// instead of failing with "try again when scanning finishes".
#[tokio::test]
async fn an_install_waits_for_a_background_check_instead_of_failing() {
    let global = tempfile::tempdir().unwrap();
    let repo = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(repo.path().join(".claude-plugin")).unwrap();
    std::fs::write(
        repo.path().join(".claude-plugin/marketplace.json"),
        r#"{"plugins":[{"name":"notes","source":"./notes"}]}"#,
    )
    .unwrap();
    std::fs::create_dir_all(repo.path().join("notes/skills/jot")).unwrap();
    std::fs::write(
        repo.path().join("notes/skills/jot/SKILL.md"),
        "---\nname: jot\ndescription: Jot\n---\nWrite it down.",
    )
    .unwrap();
    let store = PluginStore::new(global.path().into(), None);
    store
        .add_marketplace("fixture", repo.path().to_str().unwrap())
        .unwrap();
    // A background check holds the marketplace's lock for a moment.
    let directory = global.path().join("compatibility-cache/fixture");
    std::fs::create_dir_all(&directory).unwrap();
    let lock = std::fs::OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .open(directory.join("scan.lock"))
        .unwrap();
    lock.try_lock().unwrap();
    let release = tokio::spawn(async move {
        tokio::time::sleep(std::time::Duration::from_millis(400)).await;
        drop(lock);
    });
    let installed = store.install("global", "fixture", "notes").await.unwrap();
    assert_eq!(installed.spec.skills[0].instructions, "Write it down.");
    release.await.unwrap();
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
    let preview = store.preview("fixture", "review").await.unwrap();
    assert_eq!(preview.skills[0].instructions, "Check ownership.");
    assert!(preview.files.contains_key("skills/rust/SKILL.md"));
    assert!(!global.path().join("plugins/registry.json").exists());
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

async fn mcp_with_instructions(instructions: Option<serde_json::Value>) -> wiremock::MockServer {
    use wiremock::{matchers::body_partial_json, Mock, MockServer, ResponseTemplate};
    let server = MockServer::builder().start().await;
    let mut result = json!({"protocolVersion":"2025-03-26","capabilities":{},"serverInfo":{"name":"fixture","version":"1"}});
    if let Some(instructions) = instructions {
        result["instructions"] = instructions;
    }
    Mock::given(body_partial_json(json!({"method":"initialize"})))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_json(json!({"jsonrpc":"2.0","id":1,"result":result})),
        )
        .mount(&server)
        .await;
    Mock::given(body_partial_json(
        json!({"method":"notifications/initialized"}),
    ))
    .respond_with(ResponseTemplate::new(202))
    .mount(&server)
    .await;
    Mock::given(body_partial_json(json!({"method":"tools/list"})))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({"jsonrpc":"2.0","id":2,"result":{"tools":[{"name":"search","description":"Search docs","inputSchema":{"type":"object","properties":{}}}]}})))
        .mount(&server).await;
    server
}

#[tokio::test]
async fn mcp_instructions_are_optional_and_bounded_in_bytes() {
    use themis_core::{
        plugins::connections::{tools, McpServer},
        tools::AllowAllHook,
    };
    for instructions in [None, Some(String::new()), Some("é".repeat(16384))] {
        let server =
            mcp_with_instructions(instructions.clone().map(serde_json::Value::String)).await;
        let root = tempfile::tempdir().unwrap();
        let config = McpServer {
            url: Some(server.uri()),
            ..Default::default()
        };
        let (tools, guidance, _) = tools(
            "docs",
            &config,
            root.path(),
            std::sync::Arc::new(AllowAllHook),
        )
        .await
        .unwrap();
        assert_eq!(guidance, instructions);
        assert_eq!(tools.len(), 1);
    }
}

#[tokio::test]
async fn mcp_rejects_malformed_and_oversized_instructions_before_listing_tools() {
    use themis_core::{
        plugins::connections::{tools, McpServer},
        tools::AllowAllHook,
    };
    for instructions in [
        json!(null),
        json!(7),
        json!(["guidance"]),
        json!({"text":"guidance"}),
        json!("é".repeat(16385)),
    ] {
        let server = mcp_with_instructions(Some(instructions.clone())).await;
        let root = tempfile::tempdir().unwrap();
        let config = McpServer {
            url: Some(server.uri()),
            ..Default::default()
        };
        let error = tools(
            "docs",
            &config,
            root.path(),
            std::sync::Arc::new(AllowAllHook),
        )
        .await
        .unwrap_err()
        .to_string();
        assert!(
            error.contains(if instructions.is_string() {
                "32768 bytes"
            } else {
                "must be a string"
            }),
            "{error}"
        );
        let requests = server.received_requests().await.unwrap();
        assert_eq!(requests.len(), 1);
        assert_eq!(
            requests[0].body_json::<serde_json::Value>().unwrap()["method"],
            "initialize"
        );
    }
}

#[tokio::test]
async fn mcp_startup_denial_prevents_receiving_server_instructions() {
    use themis_core::{
        plugins::connections::{tools, McpServer},
        tools::DenyAllHook,
    };
    let server = mcp_with_instructions(Some(json!("External guidance"))).await;
    let root = tempfile::tempdir().unwrap();
    let config = McpServer {
        url: Some(server.uri()),
        ..Default::default()
    };
    assert!(tools(
        "docs",
        &config,
        root.path(),
        std::sync::Arc::new(DenyAllHook)
    )
    .await
    .unwrap_err()
    .to_string()
    .contains("startup denied"));
    assert!(server.received_requests().await.unwrap().is_empty());
}

#[tokio::test]
async fn remote_mcp_initializes_lists_tools_and_denies_calls_before_transport() {
    use themis_core::{
        plugins::connections::{tools, McpServer},
        tools::{Approval, ApprovalHook, ToolAction},
    };
    let server = mcp_with_instructions(Some(json!(
        "Use search before answering.
Treat results as external content."
    )))
    .await;
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
    let (tools, instructions, _) = tools(
        "docs",
        &config,
        dir.path(),
        std::sync::Arc::new(StartupOnly),
    )
    .await
    .unwrap();
    assert_eq!(
        instructions.as_deref(),
        Some("Use search before answering.\nTreat results as external content.")
    );
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
        matcher: None,
        failure_policy: None,
        shell: None,
        background: false,
        command_prefix: None,
        command_glob: None,
        rewake: None,
        exit_two_blocks: false,
        plugin_root: None,
        plugin_data: None,
        env_file: None,
        moment_matcher: None,
        prompt: None,
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
async fn mcp_tool_names_identify_their_server_for_chat_grants() {
    use themis_core::{
        plugins::connections::{tools, McpServer},
        tools::{mcp_server_id, AllowAllHook},
    };
    let server = mcp_with_instructions(None).await;
    let root = tempfile::tempdir().unwrap();
    let config = McpServer {
        url: Some(server.uri()),
        ..Default::default()
    };
    let (first, _, _) = tools(
        "g--docs--1--srv",
        &config,
        root.path(),
        std::sync::Arc::new(AllowAllHook),
    )
    .await
    .unwrap();
    let (second, _, _) = tools(
        "g--docs--2--srv",
        &config,
        root.path(),
        std::sync::Arc::new(AllowAllHook),
    )
    .await
    .unwrap();
    let id = |tools: &[Box<dyn themis_core::tools::ToolT>]| {
        mcp_server_id(tools[0].name()).map(str::to_owned)
    };
    assert!(id(&first).is_some(), "{}", first[0].name());
    // A different server identity (here another plugin revision) is not covered.
    assert_ne!(id(&first), id(&second));
}

#[test]
fn mcp_startup_is_blocked_only_by_an_explicit_deny_rule() {
    use themis_core::configuration::{ApprovalPolicy, ApprovalRule, PolicyAction};
    use themis_core::plugins::connections::{startup_action, startup_denied, McpServer};
    let local = McpServer {
        command: Some("npx".into()),
        args: vec!["-y".into(), "server with space".into()],
        ..Default::default()
    };
    let ask = ApprovalPolicy {
        default: PolicyAction::Ask,
        rules: Vec::new(),
        ..Default::default()
    };
    assert!(!startup_denied("fixture", &local, &ask).unwrap());
    let deny = ApprovalPolicy {
        default: PolicyAction::Ask,
        rules: vec![ApprovalRule {
            tool: "mcp_start_*".into(),
            action: PolicyAction::Deny,
            risk: None,
            command: None,
        }],
        ..Default::default()
    };
    assert!(startup_denied("fixture", &local, &deny).unwrap());
    // The startup action shows its command as structured tokens, not a joined string.
    let action = startup_action("fixture", &local).unwrap();
    assert_eq!(
        action.display.unwrap().argv,
        ["npx", "-y", "server with space"]
    );
    assert!(!action.summary.contains("server with space"));
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
        matcher: None,
        failure_policy: None,
        shell: None,
        background: false,
        command_prefix: None,
        command_glob: None,
        rewake: None,
        exit_two_blocks: false,
        plugin_root: Some(path),
        plugin_data: None,
        env_file: None,
        moment_matcher: None,
        prompt: None,
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

#[test]
fn inspection_reports_incompatible_mcp_without_installing_or_losing_skills() {
    let global = tempfile::tempdir().unwrap();
    let package = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(package.path().join("skills/review")).unwrap();
    std::fs::write(
        package.path().join("skills/review/SKILL.md"),
        "---\nname: review\ndescription: Review\n---\nCheck code.",
    )
    .unwrap();
    std::fs::write(package.path().join(".mcp.json"), r#"{"mcpServers":{"working":{"url":"https://example.com/mcp"},"legacy":{"type":"websocket","url":"https://example.com/events"}}}"#).unwrap();
    let store = PluginStore::new(global.path().into(), None);
    assert!(store
        .import_path("global", package.path(), Some("fixture"))
        .is_err());
    let preview = store.inspect_path(package.path(), Some("fixture")).unwrap();
    assert_eq!(preview.report.status, "partial");
    let issue = preview
        .report
        .components
        .iter()
        .find(|c| c.name == "legacy")
        .unwrap();
    assert_eq!(issue.status, "unsupported");
    assert_eq!(issue.field.as_deref(), Some("mcpServers.legacy.type"));
    assert_eq!(preview.spec.skills.len(), 1);
    assert!(!global.path().join("plugins/registry.json").exists());
    let installed = store
        .import_path_with_options("global", package.path(), Some("fixture"), true)
        .unwrap();
    assert_eq!(installed.spec.skills.len(), 1);
    assert!(!installed.spec.mcp.contains_key("legacy"));
    assert!(!installed.spec.mcp["working"].enabled);
}

#[test]
fn partial_mcp_import_rejects_unsafe_credentials_and_malformed_configuration() {
    let global = tempfile::tempdir().unwrap();
    let store = PluginStore::new(global.path().into(), None);
    for input in [
        r#"{"mcpServers":{"docs":{"url":"https://user:secret@example.com/mcp"}}}"#,
        r#"{"mcpServers":{"docs":{"command":"tool","env":{"TOKEN":"secret"}}}}"#,
        r#"{"mcpServers":{"docs":{"command":"tool","args":42}}}"#,
        r#"{"mcpServers":{"docs":{"command":"tool","enabled":"yes"}}}"#,
        r#"{"mcpServers":{"docs":{"command":"tool","type":5}}}"#,
        r#"{"mcpServers":{"docs":{"url":"https://user:secret@example.com/mcp","timeout":5000}}}"#,
        r#"{"mcpServers":{"docs":{"command":"tool","env":{"TOKEN":"secret"},"timeout":5000}}}"#,
    ] {
        assert!(store
            .import_mcp_json_with_options("global", "fixture", input, true)
            .is_err());
    }
    assert!(!global.path().join("plugins/registry.json").exists());
}

#[test]
fn opencode_mcp_inspection_names_fields_and_preserves_disabled_supported_servers() {
    let global = tempfile::tempdir().unwrap();
    let store = PluginStore::new(global.path().into(), None);
    let input = r#"{"$schema":"https://opencode.ai/config.json","mcp":{"local":{"type":"local","command":["node","server.js"],"environment":{"TOKEN":"{env:TEST_TOKEN}"},"enabled":true},"slow":{"type":"remote","url":"https://example.com/mcp","timeout":9000},"oauth":{"type":"remote","url":"https://example.com/mcp","oauth":true}}}"#;
    let preview = store.inspect_mcp_json("fixture", input).unwrap();
    assert_eq!(preview.spec.mcp["local"].command.as_deref(), Some("node"));
    assert_eq!(preview.spec.mcp["local"].args, ["server.js"]);
    assert_eq!(preview.spec.mcp["local"].env["TOKEN"], "TEST_TOKEN");
    assert!(!preview.spec.mcp["local"].enabled);
    let fields: Vec<_> = preview
        .spec
        .import_issues
        .iter()
        .filter_map(|i| i.field.as_deref())
        .collect();
    assert!(fields.contains(&"mcp.slow.timeout"));
    // OAuth servers import: the sign-in happens in Integrations afterwards.
    assert!(!fields.contains(&"mcp.oauth.oauth"));
    assert_eq!(
        preview.spec.mcp["oauth"].url.as_deref(),
        Some("https://example.com/mcp")
    );
    assert!(store.import_mcp_json("global", "fixture", input).is_err());
    let installed = store
        .import_mcp_json_with_options("global", "fixture", input, true)
        .unwrap();
    assert_eq!(installed.spec.mcp.len(), 2);
    assert_eq!(store.list().unwrap()[0].spec.import_issues.len(), 1);
}

#[test]
fn native_bundle_inspection_validates_before_reporting_compatibility() {
    let invalid: PluginSpec = serde_json::from_value(json!({"name":"../escape"})).unwrap();
    assert!(themis_core::plugins::ImportPreview::from_spec(invalid).is_err());
    let global = tempfile::tempdir().unwrap();
    let package = global.path().join("bundle.json");
    std::fs::write(&package, r#"{"name":"fixture","import_issues":[{"kind":"mcp","name":"docs","status":"unsupported","field":"mcp.docs.oauth","reason":"Unsupported OAuth","remedy":"Use supported authentication"}]}"#).unwrap();
    let store = PluginStore::new(global.path().into(), None);
    assert!(store.import_path("global", &package, None).is_err());
    assert_eq!(
        store.inspect_path(&package, None).unwrap().report.status,
        "unsupported"
    );
}

#[test]
fn unsupported_only_imports_cannot_create_empty_installed_packages() {
    let global = tempfile::tempdir().unwrap();
    let store = PluginStore::new(global.path().into(), None);
    let input =
        r#"{"mcpServers":{"legacy":{"type":"websocket","url":"https://example.com/events"}}}"#;
    assert_eq!(
        store
            .inspect_mcp_json("legacy", input)
            .unwrap()
            .report
            .status,
        "unsupported"
    );
    assert!(store
        .import_mcp_json_with_options("global", "legacy", input, true)
        .unwrap_err()
        .to_string()
        .contains("No supported capabilities"));
    assert!(!global.path().join("plugins/registry.json").exists());
}

#[tokio::test]
async fn mcp_application_error_keeps_session_available_for_next_run() {
    use themis_core::{
        plugins::connections::{McpServer, Session},
        tools::AllowAllHook,
    };
    use wiremock::{matchers::body_partial_json, Mock, ResponseTemplate};
    let server = mcp_with_instructions(None).await;
    Mock::given(body_partial_json(json!({"method":"tools/call"})))
        .respond_with(|request: &wiremock::Request| {
            let body: serde_json::Value = request.body_json().unwrap();
            ResponseTemplate::new(200).set_body_json(if body["params"]["arguments"]["reject"] == true {
                json!({"jsonrpc":"2.0","id":body["id"],"error":{"code":-32602,"message":"Invalid arguments"}})
            } else if body["params"]["arguments"]["throw"] == true {
                json!({"jsonrpc":"2.0","id":body["id"],"result":{"isError":true,"content":[{"type":"text","text":"Error: ReferenceError: planets is not defined"}]}})
            } else { json!({"jsonrpc":"2.0","id":body["id"],"result":{"content":[]}}) })
        }).mount(&server).await;
    let root = tempfile::tempdir().unwrap();
    let session = Session::connect(
        &McpServer {
            url: Some(server.uri()),
            ..Default::default()
        },
        root.path(),
    )
    .await
    .unwrap();
    let (tools, _, _) = session
        .tools("docs", std::sync::Arc::new(AllowAllHook))
        .unwrap();
    assert!(tools[0].execute(json!({"reject":true})).await.is_err());
    assert!(session.is_connected());
    // A tool-level error reaches the agent with the server's own explanation.
    let error = tools[0]
        .execute(json!({"throw":true}))
        .await
        .unwrap_err()
        .to_string();
    assert!(
        error.contains("ReferenceError: planets is not defined"),
        "{error}"
    );
    let (next_run, _, _) = session
        .tools("docs", std::sync::Arc::new(AllowAllHook))
        .unwrap();
    assert!(next_run[0].execute(json!({})).await.is_ok());
}

#[cfg(unix)]
#[test]
fn materialization_preserves_open_resource_readers() {
    use std::io::Read;
    let data = tempfile::tempdir().unwrap();
    let root = tempfile::tempdir().unwrap();
    let store = PluginStore::new(data.path().into(), None);
    let mut plugin = store
        .save(
            "global",
            serde_json::from_value(json!({"name":"resources"})).unwrap(),
            None,
        )
        .unwrap();
    plugin
        .spec
        .files
        .insert("server.py".into(), "print('complete')".into());
    store.materialize(&[plugin.clone()], root.path()).unwrap();
    let path = root
        .path()
        .join(".themis/plugin-files")
        .join(plugin.skill_id("resources"))
        .join("server.py");
    let original = std::fs::metadata(&path).unwrap().modified().unwrap();
    store.materialize(&[plugin.clone()], root.path()).unwrap();
    assert_eq!(
        std::fs::metadata(&path).unwrap().modified().unwrap(),
        original
    );
    use std::os::unix::fs::PermissionsExt;
    plugin.spec.executable_files.push("server.py".into());
    store.materialize(&[plugin.clone()], root.path()).unwrap();
    assert_eq!(
        std::fs::metadata(&path).unwrap().permissions().mode() & 0o777,
        0o700
    );
    let mut reader = std::fs::File::open(&path).unwrap();
    plugin
        .spec
        .files
        .insert("server.py".into(), "print('replacement')".into());
    store.materialize(&[plugin], root.path()).unwrap();
    let mut contents = String::new();
    reader.read_to_string(&mut contents).unwrap();
    assert_eq!(contents, "print('complete')");
    assert_eq!(
        std::fs::read_to_string(path).unwrap(),
        "print('replacement')"
    );
}

/// An agent's store never leaves a hook or a command-based MCP server switched on, in any mode:
/// saves and imports keep new or changed ones off whatever the definition says (an unchanged one
/// keeps its switch), and switching one on is refused. A connection by URL keeps the consent
/// rule, and the person's own saves and switches are unchanged.
#[test]
fn an_agent_never_leaves_a_hook_or_command_server_switched_on() {
    let global = tempfile::tempdir().unwrap();
    let person = PluginStore::new(global.path().to_path_buf(), None);
    let agent = person.clone().for_agent(true);
    let hook = |command: &str| json!({"name":"guard","event":"BeforeTool","command":command,"enabled":true,"timeout_seconds":5});
    let spec = |command: &str, server: &str| -> PluginSpec {
        serde_json::from_value(json!({
            "name": "demo",
            "hooks": [hook(command)],
            "mcp": {"docs": {"command": server, "enabled": true}},
        }))
        .unwrap()
    };
    let switches = |name: &str| {
        let plugin = person
            .list()
            .unwrap()
            .into_iter()
            .find(|plugin| plugin.spec.name == name)
            .unwrap();
        let server = plugin.spec.mcp.get("docs").is_some_and(|docs| docs.enabled);
        (plugin.spec.hooks[0].enabled, server, plugin.revision)
    };
    // New: off, though the definition asked for on.
    agent.save("global", spec("echo one", "srv"), None).unwrap();
    let (hook_on, server_on, _) = switches("demo");
    assert!(!hook_on && !server_on);
    // The agent may not switch them on; the person may.
    let refused = themis_core::prompts::AGENT_SWITCH_ON_REFUSED;
    for kind in ["hook", "mcp"] {
        let id = if kind == "hook" { "guard" } else { "docs" };
        let error = agent
            .set_component_enabled("global", "demo", kind, id, true)
            .unwrap_err();
        assert_eq!(error.to_string(), refused, "{kind}");
    }
    let (hook_on, server_on, _) = switches("demo");
    assert!(!hook_on && !server_on, "nothing was switched on");
    person
        .set_component_enabled("global", "demo", "hook", "guard", true)
        .unwrap();
    person
        .set_component_enabled("global", "demo", "mcp", "docs", true)
        .unwrap();
    // Already on: switching it on again changes nothing and passes; switching off passes.
    agent
        .set_component_enabled("global", "demo", "hook", "guard", true)
        .unwrap();
    // A plugin that is off with a hook on inside may not be switched on by the agent.
    person.set_enabled("global", "demo", false).unwrap();
    let error = agent.set_enabled("global", "demo", true).unwrap_err();
    assert_eq!(error.to_string(), refused);
    person.set_enabled("global", "demo", true).unwrap();
    // The agent saving them unchanged keeps them on.
    let (.., revision) = switches("demo");
    agent
        .save("global", spec("echo one", "srv"), Some(&revision))
        .unwrap();
    let (hook_on, server_on, revision) = switches("demo");
    assert!(hook_on && server_on, "unchanged parts keep their switch");
    // Changed: off again.
    agent
        .save("global", spec("echo two", "other"), Some(&revision))
        .unwrap();
    let (hook_on, server_on, revision) = switches("demo");
    assert!(!hook_on && !server_on, "changed parts are switched off");
    // An import goes through the same save.
    let file = global.path().join("import.json");
    let imported = json!({"name": "imported", "hooks": [hook("echo x")]});
    std::fs::write(&file, serde_json::to_vec(&imported).unwrap()).unwrap();
    agent.import_path("global", &file, None).unwrap();
    assert!(!switches("imported").0);
    // The person's own save keeps what they wrote.
    person
        .save("global", spec("echo three", "srv"), Some(&revision))
        .unwrap();
    let (hook_on, server_on, _) = switches("demo");
    assert!(hook_on && server_on);
    // A connection by URL: the agent may leave it on (it still waits for the person's consent).
    let remote: PluginSpec = serde_json::from_value(
        json!({"name": "remote", "mcp": {"docs": {"url": "https://example.com/mcp", "enabled": false}}}),
    )
    .unwrap();
    agent.save("global", remote, None).unwrap();
    agent
        .set_component_enabled("global", "remote", "mcp", "docs", true)
        .unwrap();
    let remote = person
        .list()
        .unwrap()
        .into_iter()
        .find(|plugin| plugin.spec.name == "remote")
        .unwrap();
    assert!(remote.spec.mcp["docs"].enabled);
}
