//! Literal credentials in a plugin's MCP configuration become named secrets and are discarded,
//! in each form they come in, and in the plugins of the official catalog that hold them (shapes
//! copied from those plugins, values replaced).

use serde_json::{json, Value};
use std::{fs, path::Path};
use themis_core::plugins::PluginStore;

const REAL: &str = "sk-live-Zq8vK2mX9pL4vB7nR3tY";

fn write(root: &Path, relative: &str, content: &str) {
    let path = root.join(relative);
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(path, content).unwrap();
}

fn plugin(root: &Path, name: &str, servers: &Value) {
    write(
        root,
        ".claude-plugin/plugin.json",
        &json!({"name": name, "version": "1.0.0"}).to_string(),
    );
    write(
        root,
        ".mcp.json",
        &json!({ "mcpServers": servers }).to_string(),
    );
}

fn no_file_holds(root: &Path, literal: &str) {
    let mut stack = vec![root.to_path_buf()];
    while let Some(folder) = stack.pop() {
        for entry in fs::read_dir(&folder).unwrap().flatten() {
            let path = entry.path();
            if path.is_dir() {
                stack.push(path);
            } else {
                let bytes = fs::read(&path).unwrap();
                assert!(
                    !String::from_utf8_lossy(&bytes).contains(literal),
                    "{}",
                    path.display()
                );
            }
        }
    }
}

/// Imports the folder into a fresh store, returning the data folder with the plugin saved.
fn imported(root: &Path) -> (tempfile::TempDir, themis_core::plugins::Plugin) {
    let data = tempfile::tempdir().unwrap();
    let store = PluginStore::new(data.path().into(), None);
    let plugin = store.import_path("global", root, None).unwrap();
    (data, plugin)
}

#[test]
fn every_form_of_a_literal_becomes_a_reference_and_is_gone() {
    let folder = tempfile::tempdir().unwrap();
    plugin(
        folder.path(),
        "acme",
        &json!({
            "remote": {"type": "http", "url": "https://acme.example/mcp", "headers": {
                "Authorization": format!("Bearer {REAL}-a"),
                "X-Api-Key": format!("Token {REAL}-b"),
                "X-Default": format!("${{ACME_DEFAULTED_KEY:-{REAL}-c}}"),
                "X-Source": "acme-plugin"
            }, "oauth": {"clientSecret": format!("{REAL}-d")}},
            "local": {"command": "run", "env": {
                "API_KEY": format!("{REAL}-e"),
                "OTHER_KEY": format!("${{OTHER_KEY:-{REAL}-f}}"),
                "LOG_LEVEL": "ERROR"
            }},
        }),
    );
    write(
        folder.path(),
        "docs/setup.md",
        &format!("export K={REAL}-e"),
    );
    let (data, installed) = imported(folder.path());
    let (remote, local) = (&installed.spec.mcp["remote"], &installed.spec.mcp["local"]);
    assert_eq!(remote.bearer_env.as_deref(), Some("ACME_AUTHORIZATION"));
    assert_eq!(remote.headers["X-Api-Key"], "Token ${ACME_X_API_KEY}");
    assert_eq!(remote.headers["X-Default"], "${ACME_DEFAULTED_KEY}");
    assert!(remote.defaults.is_empty(), "{:?}", remote.defaults);
    assert_eq!(remote.header_values["X-Source"], "acme-plugin");
    assert_eq!(
        remote.oauth.as_ref().unwrap().client_secret_env.as_deref(),
        Some("ACME_CLIENTSECRET")
    );
    assert_eq!(local.env["API_KEY"], "ACME_API_KEY");
    assert_eq!(local.env["OTHER_KEY"], "OTHER_KEY");
    assert_eq!(local.env_values["LOG_LEVEL"], "ERROR");
    assert!(local.defaults.is_empty(), "{:?}", local.defaults);

    // The registry, the check and any kept files: the value isn't anywhere.
    no_file_holds(data.path(), REAL);
    let spec = serde_json::to_string(&installed.spec).unwrap();
    assert!(!spec.contains(REAL));
    // The plugin's copies read the references, and the setup note lost the value.
    let config: Value = serde_json::from_str(&installed.spec.files[".mcp.json"]).unwrap();
    assert_eq!(
        config["mcpServers"]["local"]["env"]["API_KEY"],
        "${ACME_API_KEY}"
    );
    assert!(installed.spec.files["docs/setup.md"].contains("${ACME_API_KEY}"));
    // Said in names only.
    let notices = installed.spec.notices.join("\n");
    assert!(notices.contains("ACME_API_KEY"), "{notices}");
    assert!(!notices.contains(REAL));
    // And it counts as what it is: the plugin works once the secrets are added.
    let data = tempfile::tempdir().unwrap();
    let preview = PluginStore::new(data.path().into(), None)
        .inspect_path(folder.path(), None)
        .unwrap();
    assert_eq!(preview.report.status, "supported", "{:?}", preview.report);
    assert!(!serde_json::to_string(&preview.report)
        .unwrap()
        .contains(REAL));
    assert!(!format!("{:?}", preview.spec).contains(REAL));
}

#[test]
fn a_package_too_big_for_the_registry_has_its_kept_copy_rewritten() {
    let folder = tempfile::tempdir().unwrap();
    plugin(
        folder.path(),
        "bigco",
        &json!({"local": {"command": "run", "env": {"API_KEY": REAL}}}),
    );
    // A binary file keeps the package on disk rather than in the registry.
    fs::write(folder.path().join("tool.bin"), [0xff, 0x00, 0xfe, 0x01]).unwrap();
    write(folder.path(), "scripts/use.sh", &format!("curl -H {REAL}"));
    let (data, installed) = imported(folder.path());
    let package = installed.spec.package.clone().expect("kept on disk");
    no_file_holds(data.path(), REAL);
    let kept = data.path().join("plugins/packages").join(&package.hash);
    let config: Value =
        serde_json::from_str(&fs::read_to_string(kept.join(".mcp.json")).unwrap()).unwrap();
    assert_eq!(
        config["mcpServers"]["local"]["env"]["API_KEY"],
        "${BIGCO_API_KEY}"
    );
    assert!(fs::read_to_string(kept.join("scripts/use.sh"))
        .unwrap()
        .contains("${BIGCO_API_KEY}"));
    // On the way nothing was written outside the store: not the rewritten text, not the original.
    let scratch: Vec<_> = fs::read_dir(std::env::temp_dir())
        .unwrap()
        .flatten()
        .filter(|entry| {
            entry
                .file_name()
                .to_string_lossy()
                .starts_with("themis-scrub")
        })
        .collect();
    assert!(scratch.is_empty(), "{scratch:?}");
    // The same folder gives the same package, so a check and an install agree.
    let (_again, second) = imported(folder.path());
    assert_eq!(second.spec.package.unwrap().hash, package.hash);
    // The source folder is the person's.
    assert!(fs::read_to_string(folder.path().join(".mcp.json"))
        .unwrap()
        .contains(REAL));
}

#[test]
fn two_values_for_one_field_never_share_a_secret() {
    let folder = tempfile::tempdir().unwrap();
    plugin(
        folder.path(),
        "twin",
        &json!({
            "one": {"command": "run", "env": {"API_KEY": "first-value-1"}},
            "two": {"command": "run", "env": {"API_KEY": "second-value-2"}},
            "three": {"command": "run", "env": {"API_KEY": "first-value-1"}},
        }),
    );
    let (_data, installed) = imported(folder.path());
    let name = |server: &str| installed.spec.mcp[server].env["API_KEY"].clone();
    assert_eq!(name("one"), "TWIN_API_KEY");
    assert_eq!(
        name("three"),
        name("one"),
        "the same value, the same secret"
    );
    assert_ne!(name("two"), name("one"));
    assert_eq!(name("two"), "TWIN_TWO_API_KEY");
}

#[test]
fn stand_ins_ask_for_the_real_value_and_everyday_words_elsewhere_are_left_alone() {
    let folder = tempfile::tempdir().unwrap();
    // The shape of Cloudinary's mediaflows server and the AWS placeholders of databases-on-aws.
    plugin(
        folder.path(),
        "cloudy",
        &json!({
            "flows": {"type": "http", "url": "https://x.example/mcp", "headers": {
                "cld-cloud-name": "cloud_name", "cld-api-key": "api_key", "cld-secret": "api_secret"
            }},
            "cloudwatch": {"command": "uvx", "env": {
                "FASTMCP_LOG_LEVEL": "ERROR",
                "AWS_REGION": "[your dsql cluster region, e.g. us-east-1 — must be a region]",
                "AWS_PROFILE": "[your aws profile name, e.g. default]"
            }},
        }),
    );
    write(
        folder.path(),
        "skills/use/SKILL.md",
        "---\nname: use\ndescription: d\n---\nPass api_key and cloud_name to the call.",
    );
    let (_data, installed) = imported(folder.path());
    let flows = &installed.spec.mcp["flows"];
    assert_eq!(flows.headers["cld-api-key"], "${CLOUDY_CLD_API_KEY}");
    assert_eq!(flows.headers["cld-secret"], "${CLOUDY_CLD_SECRET}");
    assert_eq!(flows.headers["cld-cloud-name"], "${CLOUDY_CLD_CLOUD_NAME}");
    assert!(flows.header_values.is_empty());
    let cloudwatch = &installed.spec.mcp["cloudwatch"];
    assert_eq!(cloudwatch.env["AWS_REGION"], "CLOUDY_AWS_REGION");
    assert_eq!(cloudwatch.env["AWS_PROFILE"], "CLOUDY_AWS_PROFILE");
    assert_eq!(cloudwatch.env_values["FASTMCP_LOG_LEVEL"], "ERROR");
    // A skill's mention of `api_key` is documentation, not the value.
    assert!(installed.spec.skills[0].instructions.contains("api_key"));
    assert!(installed.spec.files["skills/use/SKILL.md"].contains("Pass api_key and cloud_name"));
}

#[test]
fn an_address_left_empty_is_a_setting_the_person_fills_in() {
    let folder = tempfile::tempdir().unwrap();
    // As Informatica's plugin ships.
    plugin(
        folder.path(),
        "informatica-for-claude-platform",
        &json!({"informatica-catalog-discovery": {"type": "http", "url": ""}}),
    );
    let (_data, installed) = imported(folder.path());
    let server = &installed.spec.mcp["informatica-catalog-discovery"];
    assert_eq!(
        server.key_names(),
        ["INFORMATICA_FOR_CLAUDE_PLATFORM_INFORMATICA_CATALOG_DISCOVERY_URL"]
    );
    assert_eq!(server.filled_url(), None, "nothing to connect to yet");
}

#[test]
fn optional_keys_and_a_needed_domain_as_datadog_ships_them() {
    let folder = tempfile::tempdir().unwrap();
    plugin(
        folder.path(),
        "datadog",
        &json!({"mcp": {
            "type": "http",
            "url": "https://${DD_MCP_DOMAIN:-not-setup}/v1/mcp?referrer_ide=claude-code-plugin&plugin_version=0.7.17",
            "headers": {
                "DD_API_KEY": "${DD_API_KEY:-}",
                "DD_APPLICATION_KEY": "${DD_APPLICATION_KEY:-}",
                "X-Datadog-MCP-Toolsets": "${DD_MCP_TOOLSETS:-}"
            }
        }}),
    );
    let (_data, installed) = imported(folder.path());
    let server = &installed.spec.mcp["mcp"];
    assert_eq!(server.defaults["DD_API_KEY"], "");
    assert!(!server.defaults.contains_key("DD_MCP_DOMAIN"));
    // Unset here (no credential store, no such variable): it may sign in.
    assert!(!server.has_credentials());
    assert!(server.validate().is_ok());
}

#[test]
fn what_a_person_pastes_or_a_catalog_entry_says_is_still_refused() {
    let data = tempfile::tempdir().unwrap();
    let store = PluginStore::new(data.path().into(), None);
    // Typed or pasted: they hear about it at once rather than having it dropped.
    let error = store
        .import_mcp_json(
            "global",
            "pasted",
            &json!({"mcpServers": {"x": {"command": "run", "env": {"API_KEY": REAL}}}}).to_string(),
        )
        .unwrap_err();
    assert!(format!("{error:#}").contains("literal values are unsupported"));
    assert!(!format!("{error:#}").contains(REAL));
    assert!(themis_core::plugins::mcp_server_from_standard(
        "x",
        &json!({"url": "https://x.example/mcp", "headers": {"cld-api-key": REAL}})
    )
    .is_err());
    no_file_holds(data.path(), REAL);
}

proptest::proptest! {
    #[test]
    fn any_literal_value_imports_without_keeping_it(
        field in "[A-Za-z][A-Za-z0-9_-]{0,20}",
        value in "[ -~]{8,60}",
    ) {
        // Only values that can't be mistaken for something else in the configuration's own JSON.
        proptest::prop_assume!(!value.contains("${") && !value.contains("{env:"));
        let secret = format!("zz{value}zz");
        let folder = tempfile::tempdir().unwrap();
        plugin(folder.path(), "fuzz", &json!({"x": {"url": "https://x.example/mcp", "headers": {
            format!("{field}-key"): secret,
        }}}));
        let data = tempfile::tempdir().unwrap();
        if let Ok(installed) = PluginStore::new(data.path().into(), None).import_path("global", folder.path(), None) {
            let text = serde_json::to_string(&installed.spec).unwrap();
            proptest::prop_assert!(!text.contains(&serde_json::to_string(&secret).unwrap()));
            for server in installed.spec.mcp.values() {
                proptest::prop_assert!(server.validate().is_ok());
            }
        }
    }
}
