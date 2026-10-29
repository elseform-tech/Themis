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

fn import_mcp(servers: serde_json::Value) -> Result<ImportPreview, String> {
    let plugin = tempfile::tempdir().unwrap();
    let root = plugin.path();
    manifest(root, json!({"name": "fixture"}));
    write(
        root,
        ".mcp.json",
        &json!({ "mcpServers": servers }).to_string(),
    );
    let global = tempfile::tempdir().unwrap();
    PluginStore::new(global.path().into(), None)
        .inspect_path(root, Some("fixture"))
        .map_err(|error| format!("{error:#}"))
}

#[test]
fn install_time_settings_become_keys() {
    // Google's toolbox servers read `${user_config.bigquery_project}`.
    let preview = import_mcp(json!({"bigquery": {
        "command": "npx",
        "args": ["-y", "@toolbox-sdk/server@1.10.0", "--prebuilt", "bigquery", "--stdio"],
        "env": {"BIGQUERY_PROJECT": "${user_config.bigquery_project}", "BIGQUERY_LOCATION": "${user_config.bigquery_location}"}
    }}))
    .unwrap();
    let server = &preview.spec.mcp["bigquery"];
    assert_eq!(server.env["BIGQUERY_PROJECT"], "BIGQUERY_PROJECT");
    assert_eq!(
        server.key_names(),
        ["BIGQUERY_LOCATION", "BIGQUERY_PROJECT"]
    );
    assert_eq!(preview.report.status, "supported", "{:?}", preview.report);
}

#[test]
fn defaults_and_plain_settings_are_kept_but_secrets_must_be_keys() {
    let preview = import_mcp(json!({"grafana": {
        "command": "docker",
        "args": ["run", "--rm", "-i", "grafana/mcp-grafana"],
        "env": {
            "GRAFANA_URL": "${GRAFANA_URL:-http://localhost:3000}",
            "GRAFANA_TOKEN": "${env:GRAFANA_TOKEN}",
            "FASTMCP_LOG_LEVEL": "ERROR",
            "X_GOOG_PROXY_HEADERS": "{\"User-Agent\":\"datacloud.other\"}"
        }
    }}))
    .unwrap();
    let server = &preview.spec.mcp["grafana"];
    assert_eq!(server.env["GRAFANA_URL"], "GRAFANA_URL");
    assert_eq!(server.defaults["GRAFANA_URL"], "http://localhost:3000");
    assert_eq!(server.env["GRAFANA_TOKEN"], "GRAFANA_TOKEN");
    assert_eq!(server.env_values["FASTMCP_LOG_LEVEL"], "ERROR");
    assert_eq!(server.key_names(), ["GRAFANA_TOKEN", "GRAFANA_URL"]);

    for (name, value) in [
        ("API_KEY", "abc"),
        ("GITHUB_TOKEN", "x"),
        ("DB_PASSWORD", "hunter2"),
        ("ENDPOINT", "https://user:pass@example.com"),
        ("NOTE", "aZ8kQ2mX9pL4vB7nR3tY6wE1"),
        ("REGION", "[your region, e.g. us-east-1 — required]"),
    ] {
        let error =
            import_mcp(json!({"local": {"command": "run", "env": {name: value}}})).unwrap_err();
        assert!(
            error.contains("literal values are unsupported"),
            "{name}: {error}"
        );
    }
    let error = import_mcp(
        json!({"local": {"command": "run", "env": {"API_KEY": "${API_KEY:-sk-live-123}"}}}),
    )
    .unwrap_err();
    assert!(error.contains("literal values are unsupported"), "{error}");
}

#[test]
fn plain_headers_and_harmless_queries_are_accepted() {
    let preview = import_mcp(json!({
        "exa": {"type": "http", "url": "https://mcp.exa.ai/mcp?client=claude-code-plugin", "headers": {"x-exa-source": "claude-code-plugin"}},
        "postman": {"type": "http", "url": "https://mcp.postman.com/mcp", "headers": {"X-Plugin-Version": "0.3.0", "User-Agent": "postman-factory-plugin/0.3.0"}},
        "sourcegraph": {"type": "http", "url": "${SOURCEGRAPH_ENDPOINT}/.api/mcp/all", "headers": {"Authorization": "token ${SOURCEGRAPH_ACCESS_TOKEN}"}},
        "dynatrace": {"url": "${env:DT_ENVIRONMENT}/platform-reserved/mcp", "headers": {"Authorization": "Bearer ${env:DT_PLATFORM_TOKEN}", "Dt-External-Source": "dynatrace-for-ai/9.0.0"}}
    }))
    .unwrap();
    let spec = &preview.spec;
    assert_eq!(
        spec.mcp["exa"].header_values["x-exa-source"],
        "claude-code-plugin"
    );
    assert_eq!(
        spec.mcp["exa"].url.as_deref(),
        Some("https://mcp.exa.ai/mcp?client=claude-code-plugin")
    );
    assert_eq!(spec.mcp["postman"].header_values.len(), 2);
    assert_eq!(
        spec.mcp["sourcegraph"].key_names(),
        ["SOURCEGRAPH_ENDPOINT", "SOURCEGRAPH_ACCESS_TOKEN"]
    );
    assert_eq!(
        spec.mcp["dynatrace"].url.as_deref(),
        Some("${DT_ENVIRONMENT}/platform-reserved/mcp")
    );
    assert_eq!(
        spec.mcp["dynatrace"].bearer_env.as_deref(),
        Some("DT_PLATFORM_TOKEN")
    );

    // Credentials in a header or a query stay refused.
    for server in [
        json!({"type": "http", "url": "https://mediaflows.example.com/mcp", "headers": {"cld-api-key": "api_key"}}),
        json!({"type": "http", "url": "https://example.com/mcp?api_key=abc"}),
        json!({"type": "http", "url": "https://example.com/mcp?token=${TOKEN}"}),
        json!({"type": "http", "url": "http://example.com/mcp"}),
    ] {
        assert!(import_mcp(json!({ "remote": server })).is_err(), "{server}");
    }
}

#[test]
fn values_round_trip_through_the_standard_shape() {
    let preview = import_mcp(json!({
        "grafana": {"command": "docker", "env": {"GRAFANA_URL": "${GRAFANA_URL:-http://localhost:3000}", "LOG": "info"}},
        "exa": {"type": "http", "url": "${EXA_BASE}/mcp", "headers": {"x-exa-source": "plugin", "X-Key": "Key ${EXA_KEY}"}}
    }))
    .unwrap();
    for (name, server) in &preview.spec.mcp {
        let standard = themis_core::plugins::mcp_server_to_standard(server);
        let again = themis_core::plugins::mcp_server_from_standard(name, &standard).unwrap();
        assert_eq!(
            serde_json::to_value(server).unwrap(),
            serde_json::to_value(&again).unwrap(),
            "{name}: {standard}"
        );
    }
}

#[test]
fn the_plugin_folder_is_a_value_not_a_key() {
    let preview = import_mcp(json!({"okf": {
        "command": "uv",
        "args": ["run", "--script", "${CLAUDE_PLUGIN_ROOT}/scripts/mcp_server.py"],
        "env": {"CONFIG": "${CLAUDE_PLUGIN_ROOT}/config.json", "DATA": "${PLUGIN_ROOT}/data"}
    }}))
    .unwrap();
    let server = &preview.spec.mcp["okf"];
    assert!(server.key_names().is_empty(), "{:?}", server.key_names());
    assert_eq!(
        server.env_values["CONFIG"],
        "${CLAUDE_PLUGIN_ROOT}/config.json"
    );
    assert_eq!(server.env_values["DATA"], "${PLUGIN_ROOT}/data");
}

#[test]
fn url_defaults_harmless_fields_and_install_settings() {
    let preview = import_mcp(json!({
        "logfire": {"type": "http", "url": "${LOGFIRE_MCP_URL:-https://logfire-us.pydantic.dev/mcp}"},
        "datadog": {"type": "http", "url": "https://${DD_MCP_DOMAIN:-not-setup}/v1/mcp?referrer_ide=claude-code-plugin"},
        "pricing": {"command": "uvx", "args": ["awslabs.aws-pricing-mcp-server@latest"], "description": "Prices", "_note": "x", "disabled": false, "timeout": 120000, "autoApprove": ["get_price"]}
    }))
    .unwrap();
    let spec = &preview.spec;
    assert_eq!(preview.report.status, "supported", "{:?}", preview.report);
    assert_eq!(
        spec.mcp["logfire"].url.as_deref(),
        Some("${LOGFIRE_MCP_URL}")
    );
    assert_eq!(
        spec.mcp["logfire"].defaults["LOGFIRE_MCP_URL"],
        "https://logfire-us.pydantic.dev/mcp"
    );
    assert_eq!(spec.mcp["datadog"].defaults["DD_MCP_DOMAIN"], "not-setup");
    assert_eq!(spec.notices.len(), 2, "{:?}", spec.notices);

    // A preset OAuth client (registered for another app) stays a real gap.
    let global = tempfile::tempdir().unwrap();
    let plugin = tempfile::tempdir().unwrap();
    let root = plugin.path();
    manifest(
        root,
        json!({"name": "fixture", "userConfig": {"project": {"type": "string"}}}),
    );
    write(root, "skills/chat/SKILL.md", &skill_md("chat"));
    write(
        root,
        ".mcp.json",
        &json!({"mcpServers": {"slack": {"type": "http", "url": "https://mcp.slack.com/mcp", "oauth": {"clientId": "123.456", "callbackPort": 3118}}}}).to_string(),
    );
    let preview = PluginStore::new(global.path().into(), None)
        .inspect_path(root, Some("fixture"))
        .unwrap();
    assert_eq!(preview.report.status, "partial", "{:?}", preview.report);
    assert!(preview
        .spec
        .notices
        .contains(&"Install settings (userConfig) are entered in Settings › Secrets".to_string()));
}

#[test]
fn commands_become_manual_skills_with_a_note() {
    // Mirrors commit-commands: frontmatter, shell expansion and arguments.
    let plugin = tempfile::tempdir().unwrap();
    let root = plugin.path();
    manifest(
        root,
        json!({"name": "fixture", "commands": ["./extra/deploy.md"]}),
    );
    write(
        root,
        "commands/commit.md",
        "---\nallowed-tools: Bash(git add:*)\ndescription: Create a git commit\nargument-hint: [message]\n---\n- Status: !`git status`\n- See @src/app.ts\nCommit with message $ARGUMENTS.",
    );
    write(
        root,
        "commands/git/clean.md",
        "Remove branches that are gone.",
    );
    write(
        root,
        "extra/deploy.md",
        "---\ndescription: Deploy\n---\nDeploy $1.",
    );
    write(root, "skills/commit/SKILL.md", &skill_md("commit"));
    let preview = inspect(root);
    assert_eq!(preview.report.status, "supported", "{:?}", preview.report);
    assert_eq!(
        skill_ids(&preview),
        ["commit", "commit-command", "deploy", "git-clean"]
    );
    let skill = |id: &str| preview.spec.skills.iter().find(|s| s.id == id).unwrap();
    let commit = skill("commit-command");
    assert_eq!(commit.name, "commit");
    assert_eq!(commit.description, "Create a git commit");
    assert!(commit
        .instructions
        .starts_with(themis_core::prompts::SKILL_COMMAND_NOTE));
    assert!(commit
        .instructions
        .ends_with("Commit with message $ARGUMENTS."));
    assert_eq!(
        skill("git-clean").description,
        "Remove branches that are gone."
    );
    let mut manual = preview.spec.manual_skills.clone();
    manual.sort();
    assert_eq!(manual, ["commit-command", "deploy", "git-clean"]);
    assert_eq!(
        preview.spec.skill_paths["git-clean"],
        "commands/git/clean.md"
    );
    assert!(!preview
        .spec
        .unsupported
        .iter()
        .any(|reason| reason.contains("commands")));
}

#[test]
fn discover_groups_come_from_the_bundled_map_then_the_category() {
    use themis_core::plugins::catalog_groups;
    let catalog = json!({"plugins": [
        {"name": "code-review", "category": "productivity"},
        {"name": "not-in-the-map", "category": "database"},
        {"name": "uncategorised"}
    ]});
    let official = catalog_groups("claude-plugins-official", &catalog);
    assert_eq!(official["code-review"], "Coding");
    assert_eq!(official["not-in-the-map"], "Data & databases");
    assert_eq!(official["uncategorised"], "Other");
    let local = catalog_groups(
        "local",
        &json!({"plugins": [{"name": "code-review", "category": "security"}]}),
    );
    assert_eq!(local["code-review"], "Security");

    let bundled: std::collections::BTreeMap<String, std::collections::BTreeMap<String, String>> =
        serde_json::from_str(include_str!("../builtins/plugin-groups.json")).unwrap();
    let groups = [
        "Coding",
        "Cloud & deploy",
        "Data & databases",
        "Monitoring",
        "Security",
        "Work apps",
        "Design",
        "Other",
    ];
    for (name, group) in &bundled["claude-plugins-official"] {
        assert!(groups.contains(&group.as_str()), "{name}: {group}");
    }
}

#[test]
fn prompt_and_failure_hooks_import_and_claude_tool_names_map_to_themis_tools() {
    // Mirrors catalog plugins that guard Bash, react to edits and add prompt context.
    let plugin = tempfile::tempdir().unwrap();
    let root = plugin.path();
    manifest(root, json!({"name": "fixture"}));
    let command = |text: &str| json!([{"type": "command", "command": text}]);
    write(
        root,
        "hooks/hooks.json",
        &json!({"hooks": {
            "UserPromptSubmit": [{"hooks": command("echo context")}],
            "PostToolUseFailure": [{"matcher": "Bash", "hooks": command("echo failed")}],
            "PreToolUse": [
                {"matcher": "Bash", "hooks": command("guard")},
                {"matcher": "Edit|Write|NotebookEdit", "hooks": command("lint")},
                {"matcher": "mcp__github__create_issue", "hooks": command("audit")}
            ]
        }})
        .to_string(),
    );
    let preview = inspect(root);
    let hooks: Vec<(String, Option<String>, String)> = preview
        .spec
        .hooks
        .iter()
        .map(|hook| {
            (
                hook.event.clone(),
                hook.matcher.clone(),
                hook.command.clone(),
            )
        })
        .collect();
    assert!(
        hooks.contains(&("PromptSubmit".into(), None, "echo context".into())),
        "{hooks:?}"
    );
    assert!(hooks.contains(&(
        "ToolCallFailed".into(),
        Some("shell|background_task".into()),
        "echo failed".into()
    )));
    assert!(hooks.contains(&(
        "BeforeTool".into(),
        Some("shell|background_task".into()),
        "guard".into()
    )));
    assert!(hooks.contains(&(
        "BeforeTool".into(),
        Some("apply_patch|write_file".into()),
        "lint".into()
    )));
    // A matcher with no Themis tool is not imported; the reason is listed.
    assert!(!hooks.iter().any(|(_, _, command)| command == "audit"));
    let notes = preview.spec.unsupported.join("\n");
    assert!(
        notes.contains("PreToolUse: matcher NotebookEdit has no Themis tool"),
        "{notes}"
    );
    assert!(notes.contains("PreToolUse: matcher mcp__github__create_issue has no Themis tool"));
    assert!(!notes.contains("Hook event UserPromptSubmit"));
}
