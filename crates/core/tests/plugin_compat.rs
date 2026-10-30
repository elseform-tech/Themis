//! Catalog compatibility fixes: each test mirrors a real plugin layout from the official catalog.
use serde_json::json;
use std::{fs, path::Path};
use themis_core::plugins::{ImportPreview, PluginStore};

fn write(root: &Path, path: &str, content: &str) {
    let path = root.join(path);
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(path, content).unwrap();
}

fn skill_md(name: &str) -> String {
    format!("---\nname: {name}\ndescription: {name} skill\n---\nUse {name}.")
}

fn manifest(root: &Path, value: serde_json::Value) {
    write(root, ".claude-plugin/plugin.json", &value.to_string());
}

fn inspect(root: &Path) -> ImportPreview {
    let global = tempfile::tempdir().unwrap();
    PluginStore::new(global.path().into(), None)
        .inspect_path(root, Some("fixture"))
        .unwrap()
}

fn skill_ids(preview: &ImportPreview) -> Vec<String> {
    let mut ids: Vec<_> = preview.spec.skills.iter().map(|s| s.id.clone()).collect();
    ids.sort();
    ids
}

#[cfg(unix)]
#[test]
fn symlinks_inside_the_plugin_are_followed() {
    let plugin = tempfile::tempdir().unwrap();
    let root = plugin.path();
    manifest(root, json!({"name": "fixture"}));
    write(root, "shared/review/SKILL.md", &skill_md("review"));
    write(root, "shared/notes.md", "Shared notes");
    fs::create_dir_all(root.join("skills")).unwrap();
    std::os::unix::fs::symlink("../shared/review", root.join("skills/review")).unwrap();
    std::os::unix::fs::symlink("../shared/notes.md", root.join("skills/notes.md")).unwrap();
    let preview = inspect(root);
    assert_eq!(preview.report.status, "supported", "{:?}", preview.report);
    assert_eq!(preview.spec.files["skills/notes.md"], "Shared notes");
    assert!(preview.spec.files.contains_key("skills/review/SKILL.md"));
}

#[cfg(unix)]
#[test]
fn symlinks_that_escape_or_loop_are_not_followed() {
    let plugin = tempfile::tempdir().unwrap();
    let outside = tempfile::tempdir().unwrap();
    let root = plugin.path();
    manifest(root, json!({"name": "fixture"}));
    write(root, "skills/review/SKILL.md", &skill_md("review"));
    write(outside.path(), "secret.txt", "do not read");
    std::os::unix::fs::symlink(outside.path().join("secret.txt"), root.join("secret.txt")).unwrap();
    std::os::unix::fs::symlink("..", root.join("skills/parent")).unwrap();
    std::os::unix::fs::symlink("loop-b", root.join("loop-a")).unwrap();
    std::os::unix::fs::symlink("loop-a", root.join("loop-b")).unwrap();
    let preview = inspect(root);
    assert!(!preview
        .spec
        .files
        .values()
        .any(|text| text.contains("do not read")));
    assert!(!preview
        .spec
        .files
        .keys()
        .any(|path| path.starts_with("skills/parent")));
    for link in ["secret.txt", "skills/parent", "loop-a", "loop-b"] {
        assert!(
            preview
                .spec
                .unsupported
                .contains(&format!("Link not followed: {link}")),
            "{link}: {:?}",
            preview.spec.unsupported
        );
    }
    assert_eq!(skill_ids(&preview), ["review"]);
}

#[test]
fn files_over_one_mib_are_skipped_with_a_notice() {
    let plugin = tempfile::tempdir().unwrap();
    let root = plugin.path();
    manifest(root, json!({"name": "fixture"}));
    write(root, "skills/review/SKILL.md", &skill_md("review"));
    write(root, "docs/big.json", &"x".repeat(1024 * 1024 + 1));
    let preview = inspect(root);
    assert_eq!(preview.report.status, "supported");
    assert!(!preview.spec.files.contains_key("docs/big.json"));
    assert!(preview
        .spec
        .notices
        .contains(&"File too large: docs/big.json".to_string()));
}

#[test]
fn harmless_notes_do_not_make_a_plugin_partial() {
    let plugin = tempfile::tempdir().unwrap();
    let root = plugin.path();
    manifest(root, json!({"name": "fixture"}));
    write(
        root,
        "skills/review/SKILL.md",
        "---\nname: review\ndescription: Review\nargument-hint: <file>\nallowed-tools: Read Grep\n---\nReview.",
    );
    fs::write(root.join("logo.png"), [0x89, b'P', b'N', b'G', 0xff, 0xfe]).unwrap();
    let preview = inspect(root);
    assert_eq!(preview.report.status, "supported", "{:?}", preview.report);
    assert!(
        preview.spec.unsupported.is_empty(),
        "{:?}",
        preview.spec.unsupported
    );
    assert_eq!(preview.spec.notices.len(), 3, "{:?}", preview.spec.notices);
    assert!(preview
        .report
        .components
        .iter()
        .any(|c| c.status == "notice"
            && c.reason.as_deref() == Some("Binary resource omitted: logo.png")));
}

#[test]
fn copies_for_other_tools_and_nested_reference_skills_do_not_collide() {
    // netlify/context-and-tools ships the same skills for other tools; vercel/vercel-plugin keeps
    // an `upstream/SKILL.md` inside a skill folder as reference material.
    let plugin = tempfile::tempdir().unwrap();
    let root = plugin.path();
    manifest(root, json!({"name": "fixture"}));
    for prefix in ["", "codex/", "agent-plugin/"] {
        write(
            root,
            &format!("{prefix}skills/netlify-forms/SKILL.md"),
            &skill_md("netlify-forms"),
        );
    }
    write(root, "skills/ai-sdk/SKILL.md", &skill_md("ai-sdk"));
    write(root, "skills/ai-sdk/upstream/SKILL.md", &skill_md("ai-sdk"));
    let preview = inspect(root);
    assert_eq!(preview.report.status, "supported", "{:?}", preview.report);
    assert_eq!(skill_ids(&preview), ["ai-sdk", "netlify-forms"]);
    assert_eq!(preview.spec.skill_paths["ai-sdk"], "skills/ai-sdk/SKILL.md");
    assert!(preview
        .spec
        .files
        .contains_key("skills/ai-sdk/upstream/SKILL.md"));
}

#[test]
fn remaining_collisions_get_ids_from_their_folders() {
    let plugin = tempfile::tempdir().unwrap();
    let root = plugin.path();
    manifest(
        root,
        json!({"name": "fixture", "skills": ["./skills/", "./extra/"]}),
    );
    write(root, "skills/review/SKILL.md", &skill_md("review"));
    write(root, "extra/review/SKILL.md", &skill_md("review"));
    let preview = inspect(root);
    assert_eq!(preview.report.status, "supported", "{:?}", preview.report);
    assert_eq!(skill_ids(&preview), ["extra-review", "skills-review"]);
}

#[test]
fn layouts_without_collisions_keep_their_current_skills() {
    let plugin = tempfile::tempdir().unwrap();
    let root = plugin.path();
    manifest(root, json!({"name": "fixture"}));
    write(root, "guides/deploy/SKILL.md", &skill_md("deploy"));
    write(root, "skills/review/SKILL.md", &skill_md("review"));
    let preview = inspect(root);
    assert_eq!(skill_ids(&preview), ["deploy", "review"]);
}

#[test]
fn declared_skill_paths_are_read_instead_of_skipped() {
    let plugin = tempfile::tempdir().unwrap();
    let root = plugin.path();
    manifest(
        root,
        json!({"name": "fixture", "skills": ["./skills/review", "./more"]}),
    );
    write(root, "skills/review/SKILL.md", &skill_md("review"));
    write(root, "more/deploy/SKILL.md", &skill_md("deploy"));
    let preview = inspect(root);
    assert_eq!(preview.report.status, "supported", "{:?}", preview.report);
    assert_eq!(skill_ids(&preview), ["deploy", "review"]);

    manifest(root, json!({"name": "fixture", "skills": "./missing/"}));
    let preview = inspect(root);
    assert!(preview
        .spec
        .unsupported
        .contains(&"Declared skills path not found: ./missing/".to_string()));

    manifest(root, json!({"name": "fixture", "skills": "../elsewhere"}));
    let preview = inspect(root);
    assert!(preview
        .spec
        .unsupported
        .contains(&"Declared skills path is outside the plugin: ../elsewhere".to_string()));
}

#[test]
fn mcp_and_hook_configuration_files_named_in_the_manifest_are_imported() {
    let plugin = tempfile::tempdir().unwrap();
    let root = plugin.path();
    manifest(
        root,
        json!({"name": "fixture", "mcpServers": "./.mcp.json", "hooks": "./claude-hooks/hooks.json"}),
    );
    write(
        root,
        ".mcp.json",
        &json!({"mcpServers": {"docs": {"command": "docs-server"}}}).to_string(),
    );
    write(
        root,
        "claude-hooks/hooks.json",
        &json!({"hooks": {"PreToolUse": [{"hooks": [{"type": "command", "command": "echo ok"}]}]}})
            .to_string(),
    );
    let preview = inspect(root);
    assert_eq!(preview.report.status, "supported", "{:?}", preview.report);
    assert_eq!(preview.spec.mcp.keys().collect::<Vec<_>>(), ["docs"]);
    assert_eq!(preview.spec.hooks.len(), 1);

    manifest(root, json!({"name": "fixture", "mcpServers": "./mcp.json"}));
    fs::rename(root.join(".mcp.json"), root.join("mcp.json")).unwrap();
    let preview = inspect(root);
    assert_eq!(preview.spec.mcp.keys().collect::<Vec<_>>(), ["docs"]);
}

#[test]
fn a_server_declared_in_both_the_manifest_and_mcp_json_is_imported_once() {
    // CodSpeed and Leadfeeder define the same server inline in plugin.json and in .mcp.json.
    let plugin = tempfile::tempdir().unwrap();
    let root = plugin.path();
    let server = json!({"type": "http", "url": "https://mcp.example.com/mcp"});
    manifest(
        root,
        json!({"name": "fixture", "mcpServers": {"Example": server}}),
    );
    write(
        root,
        ".mcp.json",
        &json!({"mcpServers": {"Example": server}}).to_string(),
    );
    let preview = inspect(root);
    assert_eq!(preview.spec.mcp.len(), 1);

    write(
        root,
        ".mcp.json",
        &json!({"mcpServers": {"Example": {"type": "http", "url": "https://other.example.com/mcp"}}})
            .to_string(),
    );
    let global = tempfile::tempdir().unwrap();
    let error = PluginStore::new(global.path().into(), None)
        .inspect_path(root, Some("fixture"))
        .unwrap_err();
    assert!(error
        .to_string()
        .contains("Duplicate MCP connection 'Example'"));
}

#[test]
fn a_whole_repository_over_the_limits_imports_only_the_plugin_folders() {
    let plugin = tempfile::tempdir().unwrap();
    let root = plugin.path();
    manifest(root, json!({"name": "fixture", "skills": "./extra/skills"}));
    write(root, "skills/review/SKILL.md", &skill_md("review"));
    write(root, "extra/skills/deploy/SKILL.md", &skill_md("deploy"));
    write(root, "scripts/check.sh", "echo ok");
    for index in 0..1001 {
        write(root, &format!("src/module_{index}.py"), "pass");
    }
    let preview = inspect(root);
    assert_eq!(preview.report.status, "partial", "{:?}", preview.report);
    assert_eq!(skill_ids(&preview), ["deploy", "review"]);
    assert!(preview.spec.files.contains_key("scripts/check.sh"));
    assert!(!preview
        .spec
        .files
        .keys()
        .any(|path| path.starts_with("src/")));
    assert!(preview
        .spec
        .unsupported
        .contains(&"Large repository: only the plugin's own folders were imported".to_string()));
}

mod layouts {
    use super::*;
    use proptest::prelude::*;

    fn folder() -> impl Strategy<Value = String> {
        prop::collection::vec(
            prop::sample::select(vec!["skills", "codex", "extra", "a", "b", "upstream"]),
            1..4,
        )
        .prop_map(|parts| parts.join("/"))
    }

    proptest! {
        #![proptest_config(ProptestConfig::with_cases(48))]

        /// Any mix of skill folders and names imports with unique, safe ids.
        #[test]
        fn skill_ids_are_unique_for_any_layout(
            skills in prop::collection::vec((folder(), prop::sample::select(vec!["review", "deploy", "forms"])), 1..8),
            declared in prop::collection::vec(folder(), 0..3),
        ) {
            let plugin = tempfile::tempdir().unwrap();
            let root = plugin.path();
            let declared: Vec<String> = declared.iter().map(|path| format!("./{path}")).collect();
            manifest(root, json!({"name": "fixture", "skills": declared}));
            for (index, (folder, name)) in skills.iter().enumerate() {
                write(root, &format!("{folder}/{name}{index}/SKILL.md"), &skill_md(name));
            }
            let global = tempfile::tempdir().unwrap();
            let preview = PluginStore::new(global.path().into(), None)
                .inspect_path(root, Some("fixture"))
                .expect("import");
            let ids = skill_ids(&preview);
            let mut unique = ids.clone();
            unique.dedup();
            prop_assert_eq!(&ids, &unique);
            prop_assert!(!ids.is_empty());
        }
    }

    #[cfg(unix)]
    proptest! {
        #![proptest_config(ProptestConfig::with_cases(32))]

        /// Links never bring in content from outside the plugin, whatever they point at.
        #[test]
        fn links_never_read_outside_the_plugin(
            targets in prop::collection::vec(prop::sample::select(vec!["..", "../..", "skills", "skills/review", "skills/review/SKILL.md", "missing", "/", "loop"]), 1..6),
        ) {
            let plugin = tempfile::tempdir().unwrap();
            let outside = tempfile::tempdir().unwrap();
            let root = plugin.path().join("plugin");
            fs::create_dir_all(&root).unwrap();
            write(plugin.path(), "secret.txt", "OUTSIDE");
            write(outside.path(), "secret.txt", "OUTSIDE");
            manifest(&root, json!({"name": "fixture"}));
            write(&root, "skills/review/SKILL.md", &skill_md("review"));
            for (index, target) in targets.iter().enumerate() {
                let target = if *target == "/" { outside.path().to_str().unwrap() } else { target };
                std::os::unix::fs::symlink(target, root.join(format!("link{index}"))).unwrap();
            }
            let global = tempfile::tempdir().unwrap();
            let preview = PluginStore::new(global.path().into(), None)
                .inspect_path(&root, Some("fixture"))
                .expect("import");
            prop_assert!(!preview.spec.files.values().any(|text| text.contains("OUTSIDE")));
        }
    }
}
