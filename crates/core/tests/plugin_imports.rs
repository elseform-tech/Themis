use serde_json::json;
use std::{fs, path::Path};
use themis_core::plugins::{PluginSpec, PluginStore};

fn skill(directory: &Path, instructions: &str) {
    fs::create_dir_all(directory).unwrap();
    fs::write(
        directory.join("SKILL.md"),
        format!("---\nname: review\ndescription: Review code\n---\n{instructions}"),
    )
    .unwrap();
}

#[test]
fn plugin_mentions_resolve_current_bundle_without_eager_skill_bodies() {
    let global = tempfile::tempdir().unwrap();
    let source = tempfile::tempdir().unwrap();
    skill(source.path(), "Never eagerly inject this body.");
    let store = PluginStore::new(global.path().into(), None);
    store
        .import_path("global", source.path(), Some("reviewer"))
        .unwrap();
    let (text, skills, plugins) = store
        .resolve_prompt(
            "Use [[plugin:g--reviewer]] and [[skill:manage-plugins]]",
            &[],
        )
        .unwrap();
    assert_eq!(text, "Use @reviewer and Manage plugins");
    assert_eq!(skills.len(), 1);
    assert_eq!(plugins.len(), 1);
    assert_eq!(plugins[0].spec.name, "reviewer");
    store.set_enabled("global", "reviewer", false).unwrap();
    assert!(store.resolve_prompt("[[plugin:g--reviewer]]", &[]).is_err());
    assert!(store.resolve_prompt("[[plugin:g--missing]]", &[]).is_err());
    assert!(store.resolve_prompt("[[plugin:invalid]]", &[]).is_err());
    assert!(store.resolve_prompt("[[plugin:g--reviewer", &[]).is_err());
}

#[test]
fn local_skill_import_stable_reference_and_component_control() {
    let global = tempfile::tempdir().unwrap();
    let source = tempfile::tempdir().unwrap();
    skill(source.path(), "Check ownership.");
    fs::write(source.path().join("SKILL.md"), "---\r\nname: review\r\ndescription: Review code\r\ndisable-model-invocation: true\r\n---\r\nCheck ownership.").unwrap();
    let store = PluginStore::new(global.path().into(), None);
    let first = store
        .import_path("global", source.path(), Some("reviewer"))
        .unwrap();
    assert_eq!(first.spec.manual_skills, ["review"]);
    assert!(first.spec.disabled_skills.is_empty());
    let stable = first.capability_id("review");
    let pinned = first.skill_id("review");
    let mut changed = first.spec.clone();
    changed.skills[0].instructions = "Check lifetimes.".into();
    let second = store
        .save("global", changed, Some(&first.revision))
        .unwrap();
    assert_eq!(second.capability_id("review"), stable);
    assert!(store
        .resolve_prompt(&format!("[[skill:{stable}]]"), &[])
        .unwrap()
        .1[0]
        .instructions
        .contains("lifetimes"));
    assert!(store
        .resolve_prompt(&format!("[[skill:{pinned}]]"), &[])
        .unwrap()
        .1[0]
        .instructions
        .contains("ownership"));
    store
        .set_component_enabled("global", "reviewer", "skill", "review", false)
        .unwrap();
    assert!(store
        .resolve_prompt(&format!("[[skill:{stable}]]"), &[])
        .is_err());
    assert!(store
        .resolve_prompt(&format!("[[skill:{pinned}]]"), &[])
        .is_err());
    store
        .set_component_enabled("global", "reviewer", "skill", "review", true)
        .unwrap();
    assert!(store
        .resolve_prompt(&format!("[[skill:{stable}]]"), &[])
        .is_ok());
    assert!(store
        .import_path("global", source.path(), Some("reviewer"))
        .is_err());
}

#[test]
fn imports_mcp_shapes_without_credentials_or_silent_options() {
    let global = tempfile::tempdir().unwrap();
    let store = PluginStore::new(global.path().into(), None);
    let claude = store.import_mcp_json("global", "docs", &json!({"mcpServers":{"remote":{"type":"http","url":"https://example.com/mcp","headers":{"Authorization":"Bearer ${MCP_TOKEN}"}}}}).to_string()).unwrap();
    assert_eq!(
        claude.spec.mcp["remote"].bearer_env.as_deref(),
        Some("MCP_TOKEN")
    );
    assert!(!claude.spec.mcp["remote"].enabled);
    let opencode = store.import_mcp_json("global", "files", &json!({"mcp":{"local":{"type":"local","command":["node","server.js"],"environment":{"TOKEN":"{env:MCP_TOKEN}"}}}}).to_string()).unwrap();
    assert_eq!(opencode.spec.mcp["local"].args, ["server.js"]);
    assert_eq!(opencode.spec.mcp["local"].env["TOKEN"], "MCP_TOKEN");
    for invalid in [
        json!({"mcpServers":{"x":{"type":"sse","url":"https://example.com"}}}),
        json!({"mcpServers":{"x":{"url":"https://example.com","headers":{"Authorization":"Bearer literal-secret"}}}}),
        json!({"mcp":{"x":{"type":"remote","url":"https://example.com","oauth":true}}}),
        json!({"mcpServers":{"x":{"command":"node","env":{"TOKEN":"literal-secret"}}}}),
        json!({"mcpServers":{"x":{"command":"node","timeout":9000}}}),
    ] {
        assert!(store
            .import_mcp_json("global", "invalid", &invalid.to_string())
            .is_err());
    }
    assert!(!store
        .list()
        .unwrap()
        .iter()
        .any(|p| p.spec.name == "invalid"));
    let path = global.path().join("mcp.json");
    fs::write(
        &path,
        json!({"mcpServers":{"x":{"command":"node","args":[]}}}).to_string(),
    )
    .unwrap();
    assert_eq!(
        store
            .import_path("global", &path, Some("from-file"))
            .unwrap()
            .spec
            .mcp
            .len(),
        1
    );
}

#[test]
fn discovers_project_skills_read_only_and_persists_activation_override() {
    let global = tempfile::tempdir().unwrap();
    let project = tempfile::tempdir().unwrap();
    for directory in [
        ".agents/skills/review",
        ".codex/skills/review",
        ".claude/skills/review",
    ] {
        skill(&project.path().join(directory), "Review discovered source.");
    }
    let store = PluginStore::new(global.path().into(), Some(project.path().into()));
    let discovered: Vec<_> = store
        .discovered_plugins()
        .unwrap()
        .into_iter()
        .filter(|p| p.scope == "local")
        .collect();
    assert_eq!(discovered.len(), 3);
    assert!(discovered
        .iter()
        .all(|p| Path::new(&p.spec.skill_paths["review"]).is_absolute()));
    assert_ne!(
        discovered[0].capability_id("review"),
        discovered[1].capability_id("review")
    );
    assert!(!project.path().join(".themis").exists());
    let selected = &discovered[0];
    assert!(store
        .resolve_prompt(
            &format!("[[skill:{}]]", selected.capability_id("review")),
            &[]
        )
        .is_ok());
    fs::write(
        Path::new(&selected.spec.skill_paths["review"])
            .parent()
            .unwrap()
            .join("reference.md"),
        "New supporting resource",
    )
    .unwrap();
    let changed = store
        .discovered_plugins()
        .unwrap()
        .into_iter()
        .find(|plugin| plugin.scope == "local" && plugin.spec.name == selected.spec.name)
        .unwrap();
    assert_eq!(
        changed.capability_id("review"),
        selected.capability_id("review")
    );
    assert_ne!(changed.revision, selected.revision);
    store
        .set_component_enabled("local", &selected.spec.name, "skill", "review", false)
        .unwrap();
    let reopened = PluginStore::new(global.path().into(), Some(project.path().into()));
    assert!(
        !reopened
            .list()
            .unwrap()
            .iter()
            .find(|p| p.spec.name == selected.spec.name)
            .unwrap()
            .enabled
    );
    assert!(reopened
        .resolve_prompt(
            &format!("[[skill:{}]]", selected.capability_id("review")),
            &[]
        )
        .is_err());
    store
        .set_enabled("local", &selected.spec.name, true)
        .unwrap();
    assert!(store
        .resolve_prompt(
            &format!("[[skill:{}]]", selected.capability_id("review")),
            &[]
        )
        .is_ok());
}

#[test]
fn component_removal_preserves_siblings_and_invalidates_saved_reference() {
    let global = tempfile::tempdir().unwrap();
    let store = PluginStore::new(global.path().into(), None);
    let spec: PluginSpec = serde_json::from_value(json!({"name":"bundle","skills":[
        {"id":"first","name":"First","description":"First","instructions":"First instructions","allowedTools":[],"scripts":[]},
        {"id":"second","name":"Second","description":"Second","instructions":"Second instructions","allowedTools":[],"scripts":[]}],
        "mcp":{"one":{"command":"node"},"two":{"command":"python"}},
        "hooks":[{"name":"one","event":"RunStart","command":"true"},{"name":"two","event":"RunEnd","command":"true"}]})).unwrap();
    let first = store.save("global", spec, None).unwrap();
    store
        .remove_component("global", "bundle", "skill", "first")
        .unwrap();
    store
        .remove_component("global", "bundle", "mcp", "one")
        .unwrap();
    store
        .remove_component("global", "bundle", "hook", "one")
        .unwrap();
    let current = store
        .list()
        .unwrap()
        .into_iter()
        .find(|p| p.spec.name == "bundle")
        .unwrap();
    assert_eq!(current.spec.skills.len(), 1);
    assert_eq!(current.spec.skills[0].id, "second");
    assert_eq!(current.spec.mcp.len(), 1);
    assert!(current.spec.mcp.contains_key("two"));
    assert_eq!(current.spec.hooks.len(), 1);
    assert_eq!(current.spec.hooks[0].name, "two");
    assert_ne!(first.revision, current.revision);
    assert!(store
        .resolve_prompt(&format!("[[skill:{}]]", first.skill_id("first")), &[])
        .is_err());
    assert!(store
        .resolve_prompt(&format!("[[skill:{}]]", first.skill_id("second")), &[])
        .is_ok());
}

#[test]
fn legacy_registry_without_new_metadata_still_loads() {
    let global = tempfile::tempdir().unwrap();
    fs::create_dir_all(global.path().join("plugins")).unwrap();
    fs::write(global.path().join("plugins/registry.json"), json!({"current":{"old":{"scope":"global","revision":"v1","enabled":true,"source":null,"spec":{"name":"old","skills":[]}}},"revisions":{}}).to_string()).unwrap();
    let store = PluginStore::new(global.path().into(), None);
    let old = store
        .list()
        .unwrap()
        .into_iter()
        .find(|p| p.spec.name == "old")
        .unwrap();
    assert!(old.spec.origin.is_none());
    assert!(old.spec.disabled_skills.is_empty());
    let native: PluginSpec = serde_json::from_value(json!({"name":"native"})).unwrap();
    assert!(store.save("global", native, None).is_ok());
}

#[tokio::test]
async fn repository_import_infers_name_without_override() {
    let global = tempfile::tempdir().unwrap();
    let sources = tempfile::tempdir().unwrap();
    let store = PluginStore::new(global.path().into(), None);
    for (folder, expected) in [
        ("manifest-repo", "named-bundle"),
        ("standalone-repo", "review"),
        ("skill-bundle", "skill-bundle"),
    ] {
        let source = sources.path().join(folder);
        let skill_path = if folder == "skill-bundle" {
            source.join("skills/review")
        } else {
            source.clone()
        };
        skill(&skill_path, "Review imported Git source.");
        if folder == "manifest-repo" {
            fs::create_dir_all(source.join(".claude-plugin")).unwrap();
            fs::write(
                source.join(".claude-plugin/plugin.json"),
                r#"{"name":"named-bundle"}"#,
            )
            .unwrap();
        }
        for args in [
            vec!["init", "-q"],
            vec!["add", "."],
            vec![
                "-c",
                "user.name=Test",
                "-c",
                "user.email=test@example.com",
                "commit",
                "-qm",
                "fixture",
            ],
        ] {
            assert!(std::process::Command::new("git")
                .args(["-c", "core.hooksPath=/dev/null"])
                .args(args)
                .current_dir(&source)
                .status()
                .unwrap()
                .success());
        }
        let imported = store
            .import_repository("global", "", source.to_str().unwrap(), None, None)
            .await
            .unwrap();
        assert_eq!(imported.spec.name, expected);
        let marketplace = sources.path().join(format!("catalog-{folder}"));
        fs::create_dir_all(marketplace.join(".claude-plugin")).unwrap();
        fs::write(
            marketplace.join(".claude-plugin/marketplace.json"),
            json!({"plugins":[{"name":"preview-package","source":{"source":"url","url":source}}]})
                .to_string(),
        )
        .unwrap();
        store
            .add_marketplace(folder, marketplace.to_str().unwrap())
            .unwrap();
        assert!(!store
            .preview(folder, "preview-package")
            .await
            .unwrap()
            .skills
            .is_empty());
        assert!(fs::read_dir(global.path().join("marketplace-cache"))
            .unwrap()
            .all(|entry| !entry
                .unwrap()
                .file_name()
                .to_string_lossy()
                .starts_with("plugin-")));
        let overridden = store
            .import_repository(
                "global",
                &format!("override-{folder}"),
                source.to_str().unwrap(),
                None,
                None,
            )
            .await
            .unwrap();
        assert_eq!(overridden.spec.name, format!("override-{folder}"));
    }
}

#[tokio::test]
#[ignore = "Downloads real pinned public repositories; run explicitly for release acceptance"]
async fn real_public_plugin_and_skill_import_acceptance() {
    let global = tempfile::tempdir().unwrap();
    let store = PluginStore::new(global.path().into(), None);
    let superpowers = store
        .import_repository(
            "global",
            "superpowers",
            "https://github.com/obra/superpowers.git",
            Some("8ca22dba9a94f28898bbce59f2537ff4d87c747d"),
            None,
        )
        .await
        .unwrap();
    assert!(superpowers
        .spec
        .skills
        .iter()
        .any(|s| s.id == "using-superpowers"));
    assert!(superpowers.spec.hooks.iter().all(|h| !h.enabled));
    println!("superpowers @ 8ca22dba9a94f28898bbce59f2537ff4d87c747d: {} skills, {} disabled hooks, {} compatibility notices", superpowers.spec.skills.len(), superpowers.spec.hooks.len(), superpowers.spec.unsupported.len());
    let oversized = store
        .import_repository(
            "global",
            "oversized-creator",
            "https://github.com/anthropics/skills.git",
            Some("683bc88e56f3e09ba94f7055977f3d3aa499f202"),
            Some("skills/skill-creator"),
        )
        .await
        .unwrap_err();
    assert!(oversized.to_string().contains("32768-byte cap"));
    println!("upstream skill-creator rejected explicitly: {oversized}");
    let pdf = store
        .import_repository(
            "global",
            "upstream-pdf",
            "https://github.com/anthropics/skills.git",
            Some("683bc88e56f3e09ba94f7055977f3d3aa499f202"),
            Some("skills/pdf"),
        )
        .await
        .unwrap();
    assert!(pdf.spec.skills.iter().any(|s| s.id == "pdf"));
    assert!(pdf.spec.files.contains_key("SKILL.md"));
    println!("anthropics/skills @ 683bc88e56f3e09ba94f7055977f3d3aa499f202 pdf: {} skills, {} text resources, {} compatibility notices", pdf.spec.skills.len(), pdf.spec.files.len(), pdf.spec.unsupported.len());
    let official = tempfile::tempdir().unwrap();
    let checkout = official.path().join("catalog");
    for (args, directory) in [
        (
            vec![
                "clone",
                "-q",
                "--depth=1",
                "https://github.com/anthropics/claude-plugins-official.git",
                checkout.to_str().unwrap(),
            ],
            None,
        ),
        (
            vec![
                "fetch",
                "-q",
                "--depth=1",
                "origin",
                "d4226d062928f8d9505dbdeadd10217d23361052",
            ],
            Some(checkout.as_path()),
        ),
        (
            vec!["checkout", "-q", "--detach", "FETCH_HEAD"],
            Some(checkout.as_path()),
        ),
    ] {
        let mut command = std::process::Command::new("git");
        command
            .args(["-c", "core.hooksPath=/dev/null"])
            .args(args)
            .env("GIT_TERMINAL_PROMPT", "0");
        if let Some(directory) = directory {
            command.current_dir(directory);
        }
        assert!(command.status().unwrap().success());
    }
    store
        .add_marketplace("official-pinned", checkout.to_str().unwrap())
        .unwrap();
    assert!(
        store.catalog("official-pinned", false).await.unwrap()["plugins"]
            .as_array()
            .unwrap()
            .len()
            > 10
    );
    let dev = store
        .install("global", "official-pinned", "plugin-dev")
        .await
        .unwrap();
    assert!(!dev.spec.skills.is_empty());
    println!("official catalog @ d4226d062928f8d9505dbdeadd10217d23361052 plugin-dev: {} skills, {} compatibility notices", dev.spec.skills.len(), dev.spec.unsupported.len());
    #[cfg(unix)]
    {
        use themis_core::plugins::{hooks, Hook};
        let work = tempfile::tempdir().unwrap();
        store
            .materialize(std::slice::from_ref(&superpowers), work.path())
            .unwrap();
        let resource_root = work
            .path()
            .join(".themis/plugin-files")
            .join(superpowers.skill_id("resources"));
        // Reviewed pinned upstream script only reads its bundled skill and prints JSON.
        // Explicit native adapter; Claude SessionStart matcher/output semantics aren't imported.
        let hook = Hook {
            name: "upstream-superpowers".into(),
            event: "RunStart".into(),
            command: "bash \"$CLAUDE_PLUGIN_ROOT/hooks/session-start\"".into(),
            enabled: true,
            matcher: None,
            failure_policy: None,
            timeout_seconds: 10,
            blocking: true,
            plugin_root: Some(resource_root),
            runtime_identity: None,
        };
        let approvals: std::sync::Arc<dyn themis_core::tools::ApprovalHook> =
            std::sync::Arc::new(themis_core::tools::AllowAllHook);
        let output = hooks::run(&hook, work.path(), &json!({"event":"RunStart"}), &approvals)
            .await
            .unwrap();
        assert!(output["hookSpecificOutput"]["additionalContext"]
            .as_str()
            .unwrap()
            .contains("using-superpowers"));
        println!("real Superpowers hook: explicit native RunStart adapter executed; upstream JSON parsed (Claude context injection semantics remain unsupported)");
    }
    let work = tempfile::tempdir().unwrap();
    let marker = work.path().join("marker.txt");
    fs::write(&marker, "themis-real-mcp-acceptance").unwrap();
    let package = "@modelcontextprotocol/server-filesystem@2026.8.31";
    let mcp = store.import_mcp_json("global", "real-filesystem", &json!({"mcpServers":{"filesystem":{"command":"npx","args":["-y",package,work.path().to_str().unwrap()]}}}).to_string()).unwrap();
    assert!(!mcp.spec.mcp["filesystem"].enabled);
    store
        .set_component_enabled("global", "real-filesystem", "mcp", "filesystem", true)
        .unwrap();
    let mcp = store
        .list()
        .unwrap()
        .into_iter()
        .find(|p| p.spec.name == "real-filesystem")
        .unwrap();
    let tools = themis_core::plugins::connections::tools(
        "filesystem",
        &mcp.spec.mcp["filesystem"],
        work.path(),
        std::sync::Arc::new(themis_core::tools::AllowAllHook),
    )
    .await
    .unwrap();
    let read = tools
        .iter()
        .find(|tool| tool.name().ends_with("_read_file"))
        .expect("Official filesystem server must expose read_file");
    let result = read.execute(json!({"path":marker})).await.unwrap();
    assert!(result.to_string().contains("themis-real-mcp-acceptance"));
    println!(
        "real {package}: MCP initialization passed, {} tools discovered, read_file marker passed",
        tools.len()
    );
}
