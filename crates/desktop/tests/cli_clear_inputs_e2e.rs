//! Clear inputs on the `themis` CLI path (agent CLI plan, step 3), through the real app server
//! and the `themis` binary with an isolated data folder: each definition's exact shape in
//! `--help` and in errors, defaults for empty lists, every problem in one error, marketplace
//! names found whichever field carries them, and a single plugin's preview checking only it.
use serde_json::{json, Value};
use std::path::Path;
use themis_desktop::{server::Server, state::AppState};

/// Runs `themis --data-dir DATA ARGS`: success, stdout and stderr.
fn themis(data: &Path, args: &[&str]) -> (bool, String, String) {
    let output = std::process::Command::new(env!("CARGO_BIN_EXE_themis"))
        .arg("--data-dir")
        .arg(data)
        .args(args)
        .env_remove("OPENCODE_KEY")
        .env_remove("THEMIS_AGENT_TOKEN")
        .env_remove("THEMIS_AGENT_PORT")
        .output()
        .unwrap();
    (
        output.status.success(),
        String::from_utf8_lossy(&output.stdout).into_owned(),
        String::from_utf8_lossy(&output.stderr).into_owned(),
    )
}

fn ok(data: &Path, args: &[&str]) -> Value {
    let (success, stdout, stderr) = themis(data, args);
    assert!(success, "themis {args:?} failed: {stderr}");
    serde_json::from_str(&stdout).unwrap_or(Value::Null)
}

fn fails(data: &Path, args: &[&str]) -> String {
    let (success, stdout, stderr) = themis(data, args);
    assert!(!success, "themis {args:?} should fail; printed {stdout}");
    stderr
}

fn write(dir: &Path, name: &str, value: &Value) -> String {
    let path = dir.join(name);
    std::fs::write(&path, value.to_string()).unwrap();
    path.to_string_lossy().into_owned()
}

async fn server(data: &Path) -> tokio::task::JoinHandle<()> {
    let state = AppState::new_for_test(data.join("settings.json"));
    let server = Server::bind(state, data).await.unwrap();
    tokio::spawn(async move {
        let _ = server.run().await;
    })
}

/// `themis skill|mcp|hook --help` prints the exact definition: every field, which are required,
/// and an example, so nobody learns the fields one error at a time.
#[test]
fn help_shows_every_field_of_each_definition() {
    let data = tempfile::tempdir().unwrap();
    let cases: [(&str, &[&str]); 3] = [
        (
            "skill",
            &[
                "id",
                "name",
                "description",
                "instructions",
                "allowedTools",
                "scripts",
                "content",
            ],
        ),
        (
            "hook",
            &[
                "event",
                "command",
                "prompt",
                "matcher",
                "timeout_seconds",
                "blocking",
                "BeforeTool",
            ],
        ),
        (
            "mcp",
            &[
                "command",
                "args",
                "url",
                "env",
                "bearer_env",
                "headers",
                "tool_timeout_secs",
            ],
        ),
    ];
    for (command, fields) in cases {
        let (success, help, stderr) = themis(data.path(), &[command, "--help"]);
        assert!(success, "{stderr}");
        for field in fields {
            assert!(
                help.contains(field),
                "`{command} --help` lacks {field}:\n{help}"
            );
        }
        assert!(
            help.contains("required") && help.contains("optional"),
            "{help}"
        );
        assert!(help.contains("Example"), "{help}");
    }
    let (_, plugin_help, _) = themis(data.path(), &["plugin", "--help"]);
    assert!(plugin_help.contains("preview NAME@MARKET"), "{plugin_help}");
}

/// A skill without `allowedTools` or `scripts` saves, and into a plugin that does not exist yet.
#[tokio::test(flavor = "multi_thread")]
async fn a_skill_without_lists_saves_even_into_a_new_plugin() {
    let data = tempfile::tempdir().unwrap();
    let task = server(data.path()).await;
    let file = write(
        data.path(),
        "skill.json",
        &json!({"id": "notes", "name": "Notes", "description": "Takes notes",
            "instructions": "Write short notes."}),
    );
    ok(data.path(), &["plugin", "create", "mine"]);
    let saved = ok(data.path(), &["skill", "save", "mine", &file]);
    assert_eq!(saved["spec"]["skills"][0]["id"], "notes", "{saved}");
    let skills = ok(data.path(), &["skill", "list", "mine"]);
    assert_eq!(skills[0]["allowedTools"], json!([]));
    assert_eq!(skills[0]["scripts"], json!([]));
    // `skill verify` takes the same draft and agrees with the store.
    let verified = ok(data.path(), &["skill", "verify", &file]);
    assert_eq!(verified["valid"], true);
    // A plugin that does not exist yet is created by the save.
    let created = ok(data.path(), &["skill", "save", "fresh", &file]);
    assert_eq!(created["spec"]["name"], "fresh", "{created}");
    task.abort();
}

/// Every problem in a skill, hook or MCP definition comes back in one error, with the shape.
#[tokio::test(flavor = "multi_thread")]
async fn every_problem_in_a_definition_comes_back_at_once() {
    let data = tempfile::tempdir().unwrap();
    let task = server(data.path()).await;
    ok(data.path(), &["plugin", "create", "mine"]);
    let skill = write(
        data.path(),
        "bad-skill.json",
        &json!({"id": "notes", "instructions": 5, "tools": ["read_file"]}),
    );
    let error = fails(data.path(), &["skill", "save", "mine", &skill]);
    for wanted in [
        "name: missing (required text)",
        "instructions: must be text",
        "tools: unknown field",
        "Example",
    ] {
        assert!(error.contains(wanted), "{wanted} missing from:\n{error}");
    }
    let hook = write(
        data.path(),
        "bad-hook.json",
        &json!({"event": "BeforeEverything", "command": "true", "timeout_seconds": 900}),
    );
    let error = fails(data.path(), &["hook", "add", "mine", "guard", &hook]);
    for wanted in [
        "event 'BeforeEverything' is not an event",
        "timeout_seconds 900 must be from 1 to 600",
    ] {
        assert!(error.contains(wanted), "{wanted} missing from:\n{error}");
    }
    let server_file = write(
        data.path(),
        "bad-mcp.json",
        &json!({"command": "node", "url": "https://example.com/mcp", "tool_timeout_secs": 0,
            "env": {"TOKEN": "not a name"}}),
    );
    let error = fails(data.path(), &["mcp", "add", "mine", "docs", &server_file]);
    for wanted in [
        "tool_timeout_secs must be between 1 and 3600",
        "MCP environment values must name environment variables",
    ] {
        assert!(error.contains(wanted), "{wanted} missing from:\n{error}");
    }
    let plugin = write(
        data.path(),
        "bad-plugin.json",
        &json!({"name": "mine",
            "skills": [{"id": "a b", "name": "A", "instructions": "x"}],
            "hooks": [{"name": "h", "event": "RunStart", "command": "true"},
                      {"name": "h", "event": "RunStart", "command": "true"}]}),
    );
    let error = fails(data.path(), &["plugin", "save", &plugin]);
    for wanted in ["skills[0]", "Duplicate hook name"] {
        assert!(error.contains(wanted), "{wanted} missing from:\n{error}");
    }
    // The server answers a raw call (an agent's script, `themis call plugin_action`) the same way.
    let error = fails(
        data.path(),
        &[
            "call",
            "plugin_action",
            &json!({"action": "save_skill", "plugin": "mine",
                "skill": {"id": "notes", "instructions": 5}})
            .to_string(),
        ],
    );
    for wanted in ["name: missing", "instructions: must be text"] {
        assert!(error.contains(wanted), "{wanted} missing from:\n{error}");
    }
    task.abort();
}

/// A local marketplace with two plugins, written to `root`.
fn marketplace(root: &Path) {
    std::fs::create_dir_all(root.join(".claude-plugin")).unwrap();
    std::fs::write(
        root.join(".claude-plugin/marketplace.json"),
        json!({"name": "local-market", "plugins": [
            {"name": "first", "source": "./first"},
            {"name": "second", "source": "./second"}
        ]})
        .to_string(),
    )
    .unwrap();
    for name in ["first", "second"] {
        std::fs::create_dir_all(root.join(name)).unwrap();
        std::fs::write(
            root.join(name).join("SKILL.md"),
            format!("---\nname: {name}\ndescription: The {name} skill\n---\nDo the {name} thing."),
        )
        .unwrap();
    }
}

/// The marketplace is known without a check of it having started.
fn register(data: &Path, source: &Path) {
    std::fs::write(
        data.join("marketplaces.json"),
        json!({"sources": [{"name": "local-market", "source": source, "enabled": true}]})
            .to_string(),
    )
    .unwrap();
}

/// A marketplace-level action finds a listed marketplace under `marketplace` as under `name`,
/// and an unknown name says which names there are.
#[tokio::test(flavor = "multi_thread")]
async fn a_listed_marketplace_is_found_by_either_field_and_unknown_names_list_the_known_ones() {
    let data = tempfile::tempdir().unwrap();
    let source = tempfile::tempdir().unwrap();
    marketplace(source.path());
    register(data.path(), source.path());
    let task = server(data.path()).await;
    let listed = ok(data.path(), &["plugin", "marketplace", "list"]);
    assert!(listed.to_string().contains("local-market"), "{listed}");
    let catalog = ok(
        data.path(),
        &[
            "call",
            "plugin_action",
            r#"{"action":"catalog","marketplace":"local-market"}"#,
        ],
    );
    assert_eq!(
        catalog["plugins"].as_array().map(Vec::len),
        Some(2),
        "{catalog}"
    );
    let error = fails(data.path(), &["plugin", "marketplace", "browse", "local"]);
    assert!(
        error.contains("Unknown marketplace 'local'") && error.contains("local-market"),
        "{error}"
    );
    task.abort();
}

/// `themis plugin preview NAME@MARKET` checks that plugin alone: the rest of the marketplace is
/// not checked (a whole check fetches every plugin's repository, minutes for a big catalog).
#[tokio::test(flavor = "multi_thread")]
async fn a_single_plugin_preview_checks_only_that_plugin() {
    let data = tempfile::tempdir().unwrap();
    let source = tempfile::tempdir().unwrap();
    marketplace(source.path());
    register(data.path(), source.path());
    let task = server(data.path()).await;
    let preview = ok(
        data.path(),
        &[
            "call",
            "plugin_action",
            r#"{"action":"preview","marketplace":"local-market","name":"first"}"#,
        ],
    );
    assert_eq!(preview["skills"][0]["id"], "first", "{preview}");
    let sources = ok(data.path(), &["plugin", "marketplace", "list"]);
    let local = sources
        .as_array()
        .unwrap()
        .iter()
        .find(|source| source["name"] == "local-market")
        .unwrap()
        .clone();
    assert!(
        !matches!(local["status"].as_str(), Some("scanning" | "ready")),
        "a preview started a check of the whole marketplace: {local}"
    );
    let checked = data
        .path()
        .join("compatibility-cache/local-market/previews")
        .read_dir()
        .map(|entries| entries.count())
        .unwrap_or(0);
    assert!(
        checked <= 1,
        "{checked} plugins were checked for one preview"
    );
    // The CLI's own preview command, then the install that follows, use that check.
    let previewed = ok(data.path(), &["plugin", "preview", "first@local-market"]);
    assert_eq!(previewed["spec"]["name"], "first", "{previewed}");
    let installed = ok(data.path(), &["plugin", "install", "first@local-market"]);
    assert_eq!(installed["spec"]["name"], "first", "{installed}");
    task.abort();
}
