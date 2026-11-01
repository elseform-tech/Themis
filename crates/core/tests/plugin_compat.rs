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
fn files_over_one_mib_stay_out_of_the_registry_and_in_the_package() {
    let plugin = tempfile::tempdir().unwrap();
    let root = plugin.path();
    manifest(root, json!({"name": "fixture"}));
    write(root, "skills/review/SKILL.md", &skill_md("review"));
    write(root, "docs/big.json", &"x".repeat(1024 * 1024 + 1));
    let preview = inspect(root);
    assert_eq!(preview.report.status, "supported");
    assert!(!preview.spec.files.contains_key("docs/big.json"));
    assert_eq!(preview.spec.package.as_ref().unwrap().files, 3);
    assert!(
        preview.spec.notices.is_empty(),
        "{:?}",
        preview.spec.notices
    );
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
    assert_eq!(preview.spec.notices.len(), 2, "{:?}", preview.spec.notices);
    // The binary file isn't dropped: it is in the package, out of the registry.
    assert!(!preview.spec.files.contains_key("logo.png"));
    assert_eq!(preview.spec.package.as_ref().unwrap().files, 3);
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
fn a_whole_repository_keeps_every_file_out_of_the_registry() {
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
    // Past a package's own bounds it is still partial: see `size_tests` in marketplace.rs.
    assert_eq!(preview.report.status, "supported", "{:?}", preview.report);
    assert_eq!(skill_ids(&preview), ["deploy", "review"]);
    assert!(!preview
        .spec
        .files
        .keys()
        .any(|path| path.starts_with("src/") || path.starts_with("scripts/")));
    assert_eq!(preview.spec.package.as_ref().unwrap().files, 1005);
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
fn the_oauth_blocks_the_catalog_ships_are_kept_as_written() {
    // Wingspan's block as it is, and a preset client like one a provider registered for Themis.
    let preview = import_mcp(json!({
        "registered": {"type": "http", "url": "https://mcp.example/mcp",
            "oauth": {"clientId": "1601.8899", "callbackPort": 3118}},
        "wingspan": {"type": "http", "url": "https://api.wingspan.app/v3/mcp",
            "oauth": {"scopes": "offline_access",
                "authServerMetadataUrl": "https://api.wingspan.app/.well-known/oauth-authorization-server"}},
    }))
    .unwrap();
    assert_eq!(preview.report.status, "supported", "{:?}", preview.report);
    let oauth = |name: &str| preview.spec.mcp[name].oauth.clone().unwrap();
    assert_eq!(oauth("registered").client_id.as_deref(), Some("1601.8899"));
    assert_eq!(oauth("registered").callback_port, Some(3118));
    assert_eq!(oauth("wingspan").scopes.as_deref(), Some("offline_access"));
    assert!(oauth("wingspan").auth_server_metadata_url.is_some());
    // And `Edit as mcp.json` writes them back as they came.
    let written = themis_core::plugins::mcp_server_to_standard(&preview.spec.mcp["registered"]);
    assert_eq!(
        written["oauth"],
        json!({"clientId": "1601.8899", "callbackPort": 3118})
    );
    let again = themis_core::plugins::mcp_server_from_standard("registered", &written).unwrap();
    assert_eq!(again.oauth, Some(oauth("registered")));
}

#[test]
fn a_sign_in_set_up_for_claude_keeps_the_connection_out_and_says_so() {
    // The blocks of mongodb-atlas, render and slack, as they ship: signing in with them would
    // present Themis as Claude to the provider, so the rest of the plugin installs without them.
    for (name, url, oauth) in [
        (
            "mongodb-atlas",
            "https://mcp.mongodb.com/",
            json!({"clientId": "https://claude.ai/oauth/claude-code-client-metadata"}),
        ),
        (
            "render",
            "https://mcp.render.com/mcp",
            json!({"clientId": "claude"}),
        ),
        (
            "slack",
            "https://mcp.slack.com/mcp",
            json!({"clientId": "1601185624273.8899143856786", "callbackPort": 3118}),
        ),
    ] {
        let preview =
            import_mcp(json!({name: {"type": "http", "url": url, "oauth": oauth}})).unwrap();
        assert!(preview.spec.mcp.is_empty(), "{name} stayed in");
        let issue = &preview.spec.import_issues[0];
        assert_eq!(
            issue.field.as_deref(),
            Some(format!("mcpServers.{name}.oauth.clientId").as_str())
        );
        assert!(
            format!("{issue:?}").contains("set up for Claude"),
            "{issue:?}"
        );
        assert_ne!(preview.report.status, "supported", "{name}");
    }
}

#[test]
fn an_oauth_block_it_would_not_honour_is_named_or_refused() {
    // A field it doesn't know: the connection stays out and the field is named.
    let preview = import_mcp(json!({"a": {"type": "http", "url": "https://a.example/mcp",
        "oauth": {"clientId": "x", "pkce": false}}}))
    .unwrap();
    assert!(preview.spec.mcp.is_empty());
    assert_eq!(
        preview.spec.import_issues[0].field.as_deref(),
        Some("mcpServers.a.oauth.pkce")
    );
    // Nothing to connect to, or nothing that could be meant.
    for block in [
        json!({"command": "run", "oauth": {"clientId": "x"}}),
        json!({"url": "https://a.example/mcp", "oauth": "yes"}),
        json!({"url": "https://a.example/mcp", "oauth": {"callbackPort": 80}}),
        json!({"url": "https://a.example/mcp", "oauth": {"callbackPort": 70000}}),
        json!({"url": "https://a.example/mcp", "oauth": {"clientId": ""}}),
        json!({"url": "https://a.example/mcp", "oauth": {"clientId": 7}}),
        json!({"url": "https://a.example/mcp", "oauth": {"scopes": 7}}),
        json!({"url": "https://a.example/mcp", "oauth": {"authServerMetadataUrl": "http://a.example/m"}}),
        json!({"url": "https://a.example/mcp", "oauth": {"authServerMetadataUrl": "not an address"}}),
    ] {
        assert!(import_mcp(json!({"a": block.clone()})).is_err(), "{block}");
    }
    // Empty or off: discovery decides.
    for block in [json!({}), json!(false)] {
        let preview =
            import_mcp(json!({"a": {"url": "https://a.example/mcp", "oauth": block}})).unwrap();
        assert_eq!(preview.spec.mcp["a"].oauth, None);
    }
}

proptest::proptest! {
    #[test]
    fn any_oauth_block_imports_or_is_refused_cleanly(
        field in proptest::sample::select(vec!["clientId", "callbackPort", "scopes", "authServerMetadataUrl", "clientSecret", "other"]),
        text in "\\PC{0,40}",
        port in proptest::num::i64::ANY,
    ) {
        for value in [json!(text), json!(port), json!([text]), json!({"x": text})] {
            let result = import_mcp(json!({"a": {"url": "https://a.example/mcp", "oauth": {field: value}}}));
            if let Ok(preview) = result {
                if let Some(server) = preview.spec.mcp.get("a") {
                    proptest::prop_assert!(server.validate().is_ok());
                }
            }
        }
    }
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
        // In a plugin these become secrets the person fills in; see `mcp_literal_secrets.rs`.
        let preview = import_mcp(json!({"local": {"command": "run", "env": {name: value}}}))
            .unwrap_or_else(|error| panic!("{name}: {error}"));
        let server = &preview.spec.mcp["local"];
        assert_eq!(server.env[name], format!("FIXTURE_{name}"));
        assert!(server.env_values.is_empty());
        // ("x" and "abc" are in plenty of other words.)
        if value.len() >= 7 {
            assert!(!serde_json::to_string(&preview.spec)
                .unwrap()
                .contains(value));
        }
    }
    let preview = import_mcp(
        json!({"local": {"command": "run", "env": {"API_KEY": "${API_KEY:-sk-live-123}"}}}),
    )
    .unwrap();
    assert_eq!(preview.spec.mcp["local"].env["API_KEY"], "API_KEY");
    assert!(preview.spec.mcp["local"].defaults.is_empty());
    assert!(!serde_json::to_string(&preview.spec)
        .unwrap()
        .contains("sk-live-123"));
    // A value that mixes a reference with other text doesn't read as either.
    let error = import_mcp(json!({"local": {"command": "run", "env": {"API_KEY": "x${API_KEY}"}}}))
        .unwrap_err();
    assert!(
        error.contains("Unsupported environment reference"),
        "{error}"
    );
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

    // Credentials in a query stay refused.
    for server in [
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
    // A default for a part of the host is fine, as aws-agents-for-devsecops writes its region.
    let region = import_mcp(json!({"devops": {"type": "http",
        "url": "https://connect.aidevops.${REGION:-us-east-1}.api.aws/mcp"}}))
    .unwrap();
    assert_eq!(region.spec.mcp["devops"].defaults["REGION"], "us-east-1");
    // `not-setup` can't be a host: the value is needed, and no computer of that name is tried.
    assert!(!spec.mcp["datadog"].defaults.contains_key("DD_MCP_DOMAIN"));
    assert_eq!(spec.mcp["datadog"].key_names(), ["DD_MCP_DOMAIN"]);
    assert_eq!(spec.notices.len(), 2, "{:?}", spec.notices);

    // A preset OAuth client is kept, and a plugin's install settings are noted.
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
    assert_eq!(preview.report.status, "supported", "{:?}", preview.report);
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
            "SessionStart": [
                {"matcher": "startup|resume|clear|compact", "hooks": command("echo style")},
                {"matcher": "compact", "hooks": command("echo after-compact")}
            ],
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
    // An MCP tool is matched by its Claude name when the hook runs.
    assert!(hooks.contains(&(
        "BeforeTool".into(),
        Some("re:mcp__github__create_issue".into()),
        "audit".into()
    )));
    let notes = preview.spec.unsupported.join("\n");
    // Themis has no notebook tool: notebooks are edited with the tools this already covers.
    assert!(!notes.contains("NotebookEdit"), "{notes}");
    assert!(!notes.contains("MCP"), "{notes}");
    assert!(!notes.contains("Hook event UserPromptSubmit"));
    assert!(
        notes.contains("SessionStart: matcher compact has no Themis equivalent"),
        "{notes}"
    );
}

/// Events Themis has a moment for import under their own names, with a matcher kept for what it
/// can match; one that could only match what never happens here is listed, not kept to do nothing.
/// Events with no moment stay listed.
#[test]
fn lifecycle_events_import_only_where_their_moment_exists() {
    let plugin = tempfile::tempdir().unwrap();
    let root = plugin.path();
    let command = |text: &str| json!([{"type": "command", "command": text}]);
    write(
        root,
        ".claude-plugin/plugin.json",
        r#"{"name":"moments","description":"d"}"#,
    );
    write(
        root,
        "hooks/hooks.json",
        &json!({"hooks": {
            "SessionStart": [
                {"matcher": "startup|clear", "hooks": command("start")},
                {"matcher": "compact", "hooks": command("after-compact")},
            ],
            "SessionEnd": [
                {"hooks": command("end")},
                {"matcher": "clear|logout", "hooks": command("end-on-clear")},
                {"matcher": "clear|other", "hooks": command("end-other")},
            ],
            "UserPromptExpansion": [{"matcher": "^kit:review$", "hooks": command("expand")}],
            "UserPromptSubmit": [{"hooks": command("submit")}],
            "PermissionRequest": [{"matcher": "Bash", "hooks": command("asked")}],
            "PermissionDenied": [{"matcher": "Bash", "hooks": command("denied")}],
            "PostToolBatch": [{"hooks": command("batch")}],
            "PostCompact": [
                {"matcher": "auto", "hooks": command("compacted")},
                {"matcher": "manual", "hooks": command("compacted-by-hand")},
            ],
            "Notification": [
                {"matcher": "permission_prompt", "hooks": command("told")},
                {"matcher": "idle_prompt", "hooks": command("told-idle")},
            ],
            "TaskCreated": [{"hooks": command("created")}],
            "TaskCompleted": [{"hooks": command("completed")}],
            "ConfigChange": [{"matcher": "project_settings", "hooks": command("config")}],
            "InstructionsLoaded": [{"matcher": "session_start", "hooks": command("loaded")}],
            "FileChanged": [{"matcher": ".envrc", "hooks": command("file")}],
            "CwdChanged": [{"hooks": command("cwd")}],
            "TeammateIdle": [{"hooks": command("idle")}],
            "Elicitation": [{"hooks": command("elicit")}],
            "ElicitationResult": [{"hooks": command("elicited")}],
        }})
        .to_string(),
    );
    let preview = inspect(root);
    let find = |command: &str| {
        preview
            .spec
            .hooks
            .iter()
            .find(|hook| hook.command == command)
    };
    for (command, event, moment) in [
        ("start", "SessionStart", Some("startup|clear")),
        ("end", "SessionEnd", None),
        ("end-other", "SessionEnd", Some("clear|other")),
        ("expand", "UserPromptExpansion", Some("^kit:review$")),
        ("batch", "PostToolBatch", None),
        ("compacted", "PostCompact", Some("auto")),
        ("told", "Notification", Some("permission_prompt")),
        ("created", "TaskCreated", None),
        ("completed", "TaskCompleted", None),
        ("config", "ConfigChange", Some("project_settings")),
        ("loaded", "InstructionsLoaded", Some("session_start")),
    ] {
        let hook = find(command).unwrap_or_else(|| panic!("{command} wasn't imported"));
        assert_eq!(hook.event, event, "{command}");
        assert_eq!(hook.moment_matcher.as_deref(), moment, "{command}");
        assert!(hook.matcher.is_none(), "{command}");
    }
    for (command, event) in [
        ("asked", "PermissionRequest"),
        ("denied", "PermissionDenied"),
    ] {
        let hook = find(command).unwrap();
        assert_eq!(hook.event, event);
        assert_eq!(hook.matcher.as_deref(), Some("shell|background_task"));
    }
    assert_eq!(find("submit").unwrap().event, "PromptSubmit");
    assert!(!find("start").unwrap().exit_two_blocks);
    for never in [
        "after-compact",
        "end-on-clear",
        "compacted-by-hand",
        "told-idle",
        "file",
        "cwd",
        "idle",
        "elicit",
        "elicited",
    ] {
        assert!(find(never).is_none(), "{never} would never run");
    }
    let notes = preview.spec.unsupported.join("\n");
    for reason in [
        "SessionStart: matcher compact has no Themis equivalent",
        "SessionEnd: matcher clear|logout has no Themis equivalent",
        "PostCompact: matcher manual has no Themis equivalent",
        "Notification: matcher idle_prompt has no Themis equivalent",
        "Hook event FileChanged",
        "Hook event CwdChanged",
        "Hook event TeammateIdle",
        "Hook event Elicitation",
        "Hook event ElicitationResult",
    ] {
        assert!(notes.contains(reason), "{reason}: {notes}");
    }
    assert!(!notes.contains("Hook event SessionEnd"), "{notes}");
}

/// A hook whose worth is answering yes can't work here: hooks never grant. It stays out, and the
/// plugin says so. One that only watches, or only says no, isn't affected.
#[test]
fn a_permission_hook_that_approves_is_named_as_not_working() {
    let plugin = tempfile::tempdir().unwrap();
    let root = plugin.path();
    write(
        root,
        ".claude-plugin/plugin.json",
        r#"{"name":"approver","description":"d"}"#,
    );
    write(
        root,
        "hooks/approve.sh",
        r#"#!/bin/sh
echo '{"hookSpecificOutput":{"hookEventName":"PermissionRequest","decision":{"behavior":"allow"}}}'
"#,
    );
    write(root, "hooks/watch.sh", "#!/bin/sh\ncat >> seen.jsonl\n");
    write(
        root,
        "hooks/hooks.json",
        &json!({"hooks": {"PermissionRequest": [
            {"hooks": [{"type": "command", "command": "sh \"${CLAUDE_PLUGIN_ROOT}/hooks/approve.sh\""}]},
            {"hooks": [{"type": "command", "command": "sh \"${CLAUDE_PLUGIN_ROOT}/hooks/watch.sh\""}]},
            {"hooks": [{"type": "command", "command": "echo '{\"decision\":{\"behavior\":\"allow\"}}'"}]},
        ]}})
        .to_string(),
    );
    let preview = inspect(root);
    assert_eq!(preview.report.status, "partial", "{:?}", preview.report);
    let notes = preview.spec.unsupported.join("\n");
    assert_eq!(
        notes
            .matches("PermissionRequest: answers allow, and hooks never grant permissions")
            .count(),
        2,
        "{notes}"
    );
    assert_eq!(preview.spec.hooks.len(), 1, "{:?}", preview.spec.hooks);
    assert!(preview.spec.hooks[0].command.contains("watch.sh"));
}

/// One PreToolUse hook on `Bash` with `options`, imported: the hook if it came through, and the
/// notes either way.
fn import_hook(options: serde_json::Value) -> (Option<themis_core::plugins::hooks::Hook>, String) {
    let plugin = tempfile::tempdir().unwrap();
    let root = plugin.path();
    manifest(root, json!({"name": "fixture"}));
    let mut hook = json!({"type": "command", "command": "guard"});
    hook.as_object_mut()
        .unwrap()
        .extend(options.as_object().unwrap().clone());
    write(
        root,
        "hooks/hooks.json",
        &json!({"hooks": {"PreToolUse": [{"matcher": "Bash", "hooks": [hook]}]}}).to_string(),
    );
    let preview = inspect(root);
    (
        preview.spec.hooks.first().cloned(),
        preview.spec.unsupported.join("\n"),
    )
}

#[test]
fn options_that_change_nothing_leave_a_guard_as_it_was() {
    #[cfg(not(windows))]
    let options = json!({"statusMessage": "Checking…", "async": false, "asyncRewake": false,
        "commandWindows": "guard.cmd"});
    #[cfg(windows)]
    let options = json!({"statusMessage": "Checking…", "async": false, "asyncRewake": false});
    let (hook, notes) = import_hook(options);
    let hook = hook.expect("imported");
    assert_eq!(notes, "");
    assert_eq!(hook.command, "guard");
    assert!(hook.blocking, "a guard stays blocking");
    assert!(!hook.background);
    assert_eq!(hook.shell, None);
}

#[test]
fn shell_is_honoured_or_the_hook_stays_out() {
    let (hook, notes) = import_hook(json!({"shell": "bash"}));
    assert_eq!(hook.expect("imported").shell.as_deref(), Some("bash"));
    assert_eq!(notes, "");
    let (hook, notes) = import_hook(json!({"shell": "powershell"}));
    assert!(hook.is_none());
    assert!(notes.contains("PreToolUse: shell powershell"), "{notes}");
}

#[test]
fn async_runs_in_the_background_and_rewake_reports_back() {
    let (hook, notes) = import_hook(json!({"async": true}));
    let hook = hook.expect("imported");
    assert!(hook.background && !hook.blocking);
    assert!(hook.rewake.is_none());
    assert_eq!(notes, "");
    for options in [
        json!({"asyncRewake": true}),
        json!({"async": true, "asyncRewake": true, "rewakeMessage": "x", "rewakeSummary": "y"}),
    ] {
        let labelled = options.get("rewakeMessage").is_some();
        let (hook, notes) = import_hook(options);
        let hook = hook.expect("imported");
        assert_eq!(notes, "");
        assert!(hook.background && !hook.blocking && hook.exit_two_blocks);
        let rewake = hook.rewake.expect("reports back");
        assert_eq!(rewake.message.is_some(), labelled);
        assert_eq!(rewake.summary.is_some(), labelled);
        // Nobody waits for it, so it gets the longest a hook may have unless it asks for less.
        assert_eq!(
            hook.timeout_seconds,
            themis_core::plugins::hooks::MAX_TIMEOUT_SECONDS
        );
    }
    let (hook, _) = import_hook(json!({"asyncRewake": true, "timeout": 10}));
    assert_eq!(hook.unwrap().timeout_seconds, 10);
    // Written out but false: nothing changes.
    let (hook, _) = import_hook(json!({"asyncRewake": false, "rewakeMessage": "x"}));
    let hook = hook.unwrap();
    assert!(!hook.background && hook.rewake.is_none());
    // A long label is cut, and the hook still imports.
    let (hook, _) = import_hook(json!({"asyncRewake": true, "rewakeMessage": "é".repeat(4096)}));
    let hook = hook.unwrap();
    assert!(hook.validate().is_ok());
    assert!(
        hook.rewake.unwrap().message.unwrap().len()
            <= themis_core::plugins::hooks::REWAKE_LABEL_LIMIT
    );
    // A session hook's exit 2 stops nothing, but one that reports back reports.
    let plugin = tempfile::tempdir().unwrap();
    manifest(plugin.path(), json!({"name": "fixture"}));
    write(
        plugin.path(),
        "hooks/hooks.json",
        &json!({"hooks": {"SessionStart": [{"hooks": [{"type": "command", "command": "x", "asyncRewake": true}]}]}}).to_string(),
    );
    let session = inspect(plugin.path());
    assert!(session.spec.hooks[0].exit_two_blocks);
}

#[test]
fn if_names_a_command_or_the_hook_stays_out() {
    for rule in ["Bash(git push *)", "Bash(git push:*)", "Bash(git push)"] {
        let (hook, notes) = import_hook(json!({"if": rule}));
        assert_eq!(
            hook.expect("imported").command_prefix.unwrap(),
            ["git", "push"],
            "{rule}"
        );
        assert_eq!(notes, "");
    }
    // Patterns inside words, as security-guidance and claude-security write them.
    for rule in [
        "Bash(git*push*)",
        "Bash(py* *x*scripts/*.py *)",
        "Bash(gt*)",
    ] {
        let (hook, notes) = import_hook(json!({"if": rule}));
        let hook = hook.expect("imported");
        assert_eq!(hook.command_prefix, None);
        assert_eq!(
            hook.command_glob.as_deref(),
            rule.strip_prefix("Bash(").unwrap().strip_suffix(')')
        );
        assert_eq!(notes, "");
    }
    for rule in [
        "Bash(git?push)",
        "Read(*.env)",
        "Bash",
        "Bash()",
        "Bash(*)",
        "Bash(* *)",
    ] {
        let (hook, notes) = import_hook(json!({"if": rule}));
        assert!(hook.is_none(), "{rule}");
        assert!(notes.contains("PreToolUse: condition"), "{rule}: {notes}");
    }
}

#[test]
fn an_option_nobody_has_taught_it_keeps_the_hook_out_and_is_named() {
    let (hook, notes) = import_hook(json!({"once": true}));
    assert!(hook.is_none());
    assert!(notes.contains("PreToolUse: option once"), "{notes}");
}

#[cfg(unix)]
#[test]
fn args_reach_the_program_word_for_word() {
    let awkward = [
        "${CLAUDE_PLUGIN_ROOT}/a b.mjs",
        "two words",
        "$HOME `id` \"q\" 'r' \\",
        "plain",
    ];
    let (hook, notes) = import_hook(json!({"args": awkward}));
    assert_eq!(notes, "");
    // `command` is `guard`: define it as the shell function that prints its arguments.
    let script = format!(
        "guard() {{ for word in \"$@\"; do printf '%s\\n' \"$word\"; done; }}; {}",
        hook.unwrap().command
    );
    let output = std::process::Command::new("sh")
        .args(["-c", &script])
        .env("CLAUDE_PLUGIN_ROOT", "/plugin")
        .output()
        .unwrap();
    assert_eq!(
        String::from_utf8(output.stdout).unwrap(),
        "/plugin/a b.mjs\ntwo words\n$HOME `id` \"q\" 'r' \\\nplain\n"
    );
    let (hook, notes) = import_hook(json!({"args": ["a", 1]}));
    assert!(hook.is_none());
    assert!(notes.contains("PreToolUse: args"), "{notes}");
}

fn imported_matcher(matcher: &str) -> (Option<String>, String) {
    let plugin = tempfile::tempdir().unwrap();
    let root = plugin.path();
    manifest(root, json!({"name": "fixture"}));
    write(
        root,
        "hooks/hooks.json",
        &json!({"hooks": {"PreToolUse": [{"matcher": matcher, "hooks": [
            {"type": "command", "command": "guard"}
        ]}]}})
        .to_string(),
    );
    let preview = inspect(root);
    (
        preview
            .spec
            .hooks
            .first()
            .and_then(|hook| hook.matcher.clone()),
        preview.spec.unsupported.join("\n"),
    )
}

#[test]
fn matchers_map_to_themis_tools_and_mcp_regexes_or_stay_out() {
    for (matcher, mapped) in [
        ("Bash|Shell", "shell|background_task"),
        ("Write|NotebookEdit", "write_file|apply_patch"),
        ("mcp__.*__run_query", "re:mcp__.*__run_query"),
        ("__exec$", "re:__exec$"),
        (
            "^mcp__(?:plugin_stripe_stripe|stripe)__.*$",
            "re:^mcp__(?:plugin_stripe_stripe|stripe)__.*$",
        ),
        ("Bash|mcp__aws.*", "shell|background_task|re:mcp__aws.*"),
    ] {
        let (found, notes) = imported_matcher(matcher);
        assert_eq!(found.as_deref(), Some(mapped), "{matcher}");
        assert_eq!(notes, "", "{matcher}");
    }
    // Names Themis has no tool for are listed; what's left of the matcher still guards.
    let (found, notes) = imported_matcher("use_aws|mcp__aws.*");
    assert_eq!(found.as_deref(), Some("re:mcp__aws.*"));
    assert!(
        notes.contains("matcher use_aws has no Themis tool"),
        "{notes}"
    );
    // `Skill` isn't a tool, but its hooks run when a skill is taken up.
    let (found, notes) = imported_matcher("Skill|Read");
    assert_eq!(found.as_deref(), Some("Skill|read_file"));
    assert_eq!(notes, "");
    // A guard that can't be mapped stays out rather than matching nothing, or everything.
    for matcher in ["Bash.*", "(Edit|Write)", "mcp__(?=x)", "mcp__(", "[a-z]+"] {
        let (found, notes) = imported_matcher(matcher);
        assert_eq!(found, None, "{matcher}");
        assert!(notes.contains("no Themis tool"), "{matcher}: {notes}");
    }
}

#[test]
fn events_map_only_when_their_meaning_holds() {
    let plugin = tempfile::tempdir().unwrap();
    let root = plugin.path();
    manifest(root, json!({"name": "fixture"}));
    let one = json!([{"hooks": [{"type": "command", "command": "x"}]}]);
    write(
        root,
        "hooks/hooks.json",
        &json!({"hooks": {
            "StopFailure": one, "FileChanged": one, "CwdChanged": one,
            "TeammateIdle": one,
            "PreToolUse": [{"hooks": [{"type": "prompt", "prompt": "think"},
                                      {"type": "http", "url": "https://example.com"}]}]
        }})
        .to_string(),
    );
    let preview = inspect(root);
    let events: Vec<&str> = preview
        .spec
        .hooks
        .iter()
        .map(|h| h.event.as_str())
        .collect();
    assert_eq!(events, ["RunFailed"]);
    let notes = preview.spec.unsupported.join("\n");
    for event in ["FileChanged", "CwdChanged", "TeammateIdle"] {
        assert!(notes.contains(&format!("Hook event {event}")), "{notes}");
    }
    assert!(
        notes.contains("PreToolUse: prompt hook has no Themis equivalent"),
        "{notes}"
    );
    assert!(
        notes.contains("PreToolUse: http hook has no Themis equivalent"),
        "{notes}"
    );
}

proptest::proptest! {
    /// Whatever matcher a plugin writes: no panic, and what comes through passes validation, so
    /// a hook that imports can run. A matcher never turns into "any tool".
    #[test]
    fn any_matcher_imports_cleanly(matcher in "[ -~]{1,24}") {
        let (found, _) = imported_matcher(&matcher);
        if matcher.trim_matches('|').is_empty() || matcher == "*" || matcher == ".*" {
            return Ok(());
        }
        if let Some(found) = found {
            proptest::prop_assert!(!found.is_empty());
            let hook = themis_core::plugins::hooks::Hook {
                matcher: Some(found),
                ..serde_json::from_value(json!(
                    {"name":"a","event":"BeforeTool","command":"x","timeout_seconds":1}
                )).unwrap()
            };
            proptest::prop_assert!(hook.validate().is_ok());
        }
    }
}
