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
fn large_skill_import_discovery_and_restart_preserve_complete_instructions() {
    let global = tempfile::tempdir().unwrap();
    let project = tempfile::tempdir().unwrap();
    let source = project.path().join(".agents/skills/review");
    let instructions = "é".repeat(128 * 1024);
    skill(&source, &instructions);
    let store = PluginStore::new(global.path().into(), Some(project.path().into()));
    let imported = store
        .import_path("global", &source, Some("large-review"))
        .unwrap();
    assert_eq!(imported.spec.skills[0].instructions, instructions);
    let restarted = PluginStore::new(global.path().into(), Some(project.path().into()));
    let loaded = restarted.list().unwrap();
    assert_eq!(
        loaded
            .iter()
            .find(|p| p.spec.name == "large-review")
            .unwrap()
            .spec
            .skills[0]
            .instructions,
        instructions
    );
    let discovered = loaded
        .iter()
        .find(|p| {
            p.scope == "local"
                && p.spec
                    .origin
                    .as_ref()
                    .is_some_and(|o| o.kind == "discovered")
        })
        .unwrap();
    assert_eq!(discovered.spec.skills[0].instructions, instructions);
}

#[test]
fn standalone_imports_are_distinct_from_real_plugin_packages() {
    let global = tempfile::tempdir().unwrap();
    let source = tempfile::tempdir().unwrap();
    skill(source.path(), "Review this file.");
    let store = PluginStore::new(global.path().into(), None);
    let standalone = store
        .import_path("global", source.path(), Some("standalone"))
        .unwrap();
    assert_eq!(
        serde_json::to_value(&standalone.spec).unwrap()["package_kind"],
        "skill"
    );
    let mut legacy = standalone.spec.clone();
    legacy.name = "legacy-skill".into();
    legacy.package_kind = None;
    store.save("global", legacy, None).unwrap();
    assert_eq!(
        store
            .list()
            .unwrap()
            .iter()
            .find(|p| p.spec.name == "legacy-skill")
            .unwrap()
            .spec
            .package_kind
            .as_deref(),
        Some("skill")
    );
    let mut invalid = standalone.spec.clone();
    invalid.name = "invalid-kind".into();
    invalid.package_kind = Some("unknown".into());
    assert!(store.save("global", invalid, None).is_err());
    fs::create_dir_all(source.path().join(".claude-plugin")).unwrap();
    fs::write(
        source.path().join(".claude-plugin/plugin.json"),
        r#"{"name":"bundle"}"#,
    )
    .unwrap();
    let bundle = store.import_path("global", source.path(), None).unwrap();
    assert_eq!(
        serde_json::to_value(&bundle.spec).unwrap()["package_kind"],
        "plugin"
    );
    assert_eq!(bundle.spec.skills.len(), 1);
    let mcp = store
        .import_mcp_json(
            "global",
            "filesystem",
            r#"{"mcpServers":{"files":{"command":"test-server"}}}"#,
        )
        .unwrap();
    assert_eq!(
        serde_json::to_value(&mcp.spec).unwrap()["package_kind"],
        "mcp"
    );
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
        let preview = store
            .preview_repository("", source.to_str().unwrap(), None, None)
            .await
            .unwrap();
        assert_eq!(preview.name, expected);
        let path = &preview.skill_paths["review"];
        assert_eq!(
            preview.files[path],
            fs::read_to_string(skill_path.join("SKILL.md")).unwrap()
        );
        assert!(!store
            .list()
            .unwrap()
            .iter()
            .any(|plugin| plugin.spec.name == expected));
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
    let creator = store
        .import_repository(
            "global",
            "upstream-creator",
            "https://github.com/anthropics/skills.git",
            Some("683bc88e56f3e09ba94f7055977f3d3aa499f202"),
            Some("skills/skill-creator"),
        )
        .await
        .unwrap();
    assert_eq!(creator.spec.skills[0].instructions.len(), 32805);
    println!("upstream skill-creator imported without truncation: 32805 bytes");
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
    for name in ["docx", "xlsx"] {
        let imported = store
            .import_repository(
                "global",
                &format!("upstream-{name}"),
                "https://github.com/anthropics/skills.git",
                Some("683bc88e56f3e09ba94f7055977f3d3aa499f202"),
                Some(&format!("skills/{name}")),
            )
            .await
            .unwrap();
        assert_eq!(imported.spec.package_kind.as_deref(), Some("skill"));
        assert_eq!(imported.spec.skills.len(), 1);
        assert_eq!(imported.spec.skills[0].id, name);
        println!(
            "anthropics/skills {name}: imported standalone skill and {} resources",
            imported.spec.files.len()
        );
    }

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
    let (tools, _instructions) = themis_core::plugins::connections::tools(
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

#[test]
fn agent_created_personal_skills_do_not_become_plugin_packages() {
    use themis_core::{plugins::PluginSpec, skills::Skill};
    let global = tempfile::tempdir().unwrap();
    let store = PluginStore::new(global.path().into(), None);
    let skill: Skill = serde_json::from_value(serde_json::json!({"id":"review","name":"Review","description":"Review changes","instructions":"Inspect the changes","allowedTools":[],"scripts":[]})).unwrap();
    let saved = store
        .save_skill("global", "personal", skill.clone(), None)
        .unwrap();
    assert_eq!(saved.spec.package_kind.as_deref(), Some("skill"));
    let updated = store
        .save_skill("global", "personal", skill.clone(), Some(&saved.revision))
        .unwrap();
    assert_eq!(updated.spec.package_kind.as_deref(), Some("skill"));
    let bundle = store
        .save(
            "global",
            PluginSpec {
                name: "real-bundle".into(),
                package_kind: Some("plugin".into()),
                ..Default::default()
            },
            None,
        )
        .unwrap();
    let bundle = store
        .save_skill(
            "global",
            "real-bundle",
            skill.clone(),
            Some(&bundle.revision),
        )
        .unwrap();
    assert_eq!(bundle.spec.package_kind.as_deref(), Some("plugin"));
    let legacy = store
        .save(
            "global",
            PluginSpec {
                name: "ambiguous-legacy".into(),
                skills: vec![skill.clone()],
                ..Default::default()
            },
            None,
        )
        .unwrap();
    store
        .save_skill("global", "ambiguous-legacy", skill, Some(&legacy.revision))
        .unwrap();
    assert_eq!(
        store
            .list()
            .unwrap()
            .iter()
            .find(|plugin| plugin.spec.name == "ambiguous-legacy")
            .unwrap()
            .spec
            .package_kind
            .as_deref(),
        Some("plugin")
    );
}

#[test]
fn skill_edits_refresh_captured_markdown_and_preserve_explicit_file_edits() {
    let global = tempfile::tempdir().unwrap();
    let source = tempfile::tempdir().unwrap();
    let original = "---\nname: review\ndescription: Original description\nlicense: Apache-2.0\ndisable-model-invocation: true\nmetadata:\n  owner: personal\n---\n\nOriginal instructions.\n";
    fs::write(source.path().join("SKILL.md"), original).unwrap();
    let store = PluginStore::new(global.path().into(), None);
    let imported = store
        .import_path("global", source.path(), Some("personal-review"))
        .unwrap();
    // Older imports captured the root file before skill_paths metadata existed.
    let mut legacy = imported.spec.clone();
    legacy.name = "legacy-review".into();
    legacy.skill_paths.clear();
    let legacy = store.save("global", legacy, None).unwrap();
    let mut legacy_skill = legacy.spec.skills[0].clone();
    legacy_skill.instructions = "Legacy updated instructions.".into();
    let legacy = store
        .save_skill(
            "global",
            "legacy-review",
            legacy_skill,
            Some(&legacy.revision),
        )
        .unwrap();
    assert!(legacy.spec.files["SKILL.md"].contains("Legacy updated instructions."));
    let unchanged = store
        .save_skill(
            "global",
            "personal-review",
            imported.spec.skills[0].clone(),
            Some(&imported.revision),
        )
        .unwrap();
    assert_eq!(unchanged.spec.files["SKILL.md"], original);
    let mut skill = unchanged.spec.skills[0].clone();
    skill.instructions = "Updated instructions.".into();
    skill.name = "Review: # safely".into();
    skill.description = "Updated: # description".into();
    let updated = store
        .save_skill(
            "global",
            "personal-review",
            skill,
            Some(&unchanged.revision),
        )
        .unwrap();
    let markdown = &updated.spec.files["SKILL.md"];
    assert!(markdown.contains("Updated instructions."), "{markdown}");
    assert!(!markdown.contains("Original instructions."));
    assert!(markdown.contains("Review: # safely"));
    assert!(markdown.contains("Updated: # description"));
    assert!(markdown.contains(
        "license: Apache-2.0\ndisable-model-invocation: true\nmetadata:\n  owner: personal\n"
    ));
    let exported = source.path().join("review-export");
    fs::create_dir(&exported).unwrap();
    fs::write(exported.join("SKILL.md"), markdown).unwrap();
    let parsed = store
        .import_path("global", &exported, Some("reimported-review"))
        .unwrap();
    assert_eq!(parsed.spec.skills[0].name, "Review: # safely");
    assert_eq!(parsed.spec.skills[0].description, "Updated: # description");
    assert_eq!(parsed.spec.manual_skills, vec!["review-export"]);
    let mut spec = updated.spec.clone();
    spec.skills[0].instructions = "Updated from whole-spec editor.".into();
    let edited = store.save("global", spec, Some(&updated.revision)).unwrap();
    assert!(edited.spec.files["SKILL.md"].contains("Updated from whole-spec editor."));
    let mut spec = edited.spec.clone();
    let explicit_file = "---\nname: review\n---\nUser-edited raw Markdown wins.\n";
    spec.files.insert("SKILL.md".into(), explicit_file.into());
    spec.skills[0].instructions = "Do not overwrite the explicit source edit.".into();
    let explicit = store.save("global", spec, Some(&edited.revision)).unwrap();
    assert_eq!(explicit.spec.files["SKILL.md"], explicit_file);
}

#[test]
fn mcp_and_hook_only_imports_do_not_fabricate_skills() {
    let global = tempfile::tempdir().unwrap();
    let source = tempfile::tempdir().unwrap();
    let store = PluginStore::new(global.path().into(), None);
    fs::write(
        source.path().join(".mcp.json"),
        r#"{"mcpServers":{"docs":{"url":"https://example.com/mcp"}}}"#,
    )
    .unwrap();
    let mcp = store
        .import_path("global", source.path(), Some("mcp-only"))
        .unwrap();
    assert!(
        mcp.spec.skills.is_empty(),
        "Only upstream SKILL.md files may create skills"
    );
    assert_eq!(mcp.spec.mcp.len(), 1);
    fs::remove_file(source.path().join(".mcp.json")).unwrap();
    fs::create_dir(source.path().join("hooks")).unwrap();
    fs::write(
        source.path().join("hooks/hooks.json"),
        r#"{"hooks":{"SessionStart":[{"hooks":[{"type":"command","command":"printf fixture"}]}]}}"#,
    )
    .unwrap();
    let hook = store
        .import_path("global", source.path(), Some("hook-only"))
        .unwrap();
    assert!(hook.spec.skills.is_empty());
    assert_eq!(hook.spec.hooks.len(), 1);
}

#[test]
fn legacy_generated_connect_is_removed_from_all_loaded_revisions_only() {
    let global = tempfile::tempdir().unwrap();
    let store = PluginStore::new(global.path().into(), None);
    let spec: PluginSpec = serde_json::from_value(json!({
        "name":"legacy", "origin":{"kind":"repository","location":"https://example.com/legacy.git"},
        "skills":[{"id":"connect","name":"Use legacy","description":"Use this plugin's connected tools and hooks","instructions":"Use the plugin tools to complete the task. Respect Themis approvals.","allowedTools":[],"scripts":[]}],
        "mcp":{"docs":{"url":"https://example.com/mcp","enabled":true}},
        "files":{".mcp.json":"{\"mcpServers\":{\"docs\":{\"url\":\"https://example.com/mcp\"}}}"},
        "disabled_skills":["connect"],"manual_skills":["connect"]
    })).unwrap();
    let saved = store.save("global", spec.clone(), None).unwrap();
    let path = global.path().join("plugins/registry.json");
    let before = fs::read(&path).unwrap();
    let loaded = store
        .list()
        .unwrap()
        .into_iter()
        .find(|plugin| plugin.spec.name == "legacy")
        .unwrap();
    assert!(loaded.spec.skills.is_empty());
    assert!(loaded.spec.disabled_skills.is_empty());
    assert!(loaded.spec.manual_skills.is_empty());
    assert_eq!(loaded.revision, saved.revision);
    assert_eq!(
        fs::read(&path).unwrap(),
        before,
        "Read-only load must not rewrite the store"
    );
    assert!(!store
        .available_skills()
        .unwrap()
        .iter()
        .any(|skill| skill.plugin == "legacy"));
    assert!(store
        .resolve_prompt(&format!("[[skill:{}]]", saved.skill_id("connect")), &[])
        .is_err());
    let (_, skills, plugins) = store.resolve_prompt("[[plugin:g--legacy]]", &[]).unwrap();
    assert!(skills.is_empty());
    assert_eq!(plugins[0].spec.mcp.len(), 1);
    store.set_enabled("global", "legacy", false).unwrap();
    let persisted: serde_json::Value = serde_json::from_slice(&fs::read(path).unwrap()).unwrap();
    assert_eq!(persisted["current"]["legacy"]["revision"], saved.revision);
    assert!(persisted["revisions"]
        .as_object()
        .unwrap()
        .values()
        .all(|plugin| plugin["spec"]["skills"].as_array().unwrap().is_empty()));

    for (name, mut genuine) in [
        ("personal", spec.clone()),
        ("authored", spec.clone()),
        ("custom", spec),
    ] {
        genuine.name = name.into();
        genuine.skills[0].name = format!("Use {name}");
        if name == "personal" {
            genuine.origin = None;
        }
        if name == "authored" {
            genuine.files.insert("skills/connect/SKILL.md".into(),"---\nname: connect\n---\nUse the plugin tools to complete the task. Respect Themis approvals.\n".into());
        }
        if name == "custom" {
            genuine.skills[0].instructions =
                "Use the real connection with a concrete workflow.".into();
        }
        let expected_skill = serde_json::to_value(&genuine.skills[0]).unwrap();
        let expected_files = genuine.files.clone();
        store.save("global", genuine, None).unwrap();
        let loaded = store
            .list()
            .unwrap()
            .into_iter()
            .find(|plugin| plugin.spec.name == name)
            .unwrap();
        assert_eq!(
            loaded.spec.skills.len(),
            1,
            "Preserve genuine {name} connect skills"
        );
        assert_eq!(
            serde_json::to_value(&loaded.spec.skills[0]).unwrap(),
            expected_skill
        );
        assert_eq!(loaded.spec.files, expected_files);
    }
}
