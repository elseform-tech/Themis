//! One way to add each integration on the `themis` CLI, through the real app server and the
//! `themis` binary with an isolated data folder: a definition given as a file, `-` (stdin) or
//! inline JSON; a whole standard `.mcp.json` added with `mcp add PLUGIN DEF`, converted as an
//! import converts it; `mcp|hook|skill list` without a plugin; and the removed
//! `skill|mcp|hook import|inspect` spellings refused with a pointer to `plugin import`.
use serde_json::{json, Value};
use std::io::Write;
use std::path::Path;
use std::process::Stdio;
use themis_desktop::{server::Server, state::AppState};

/// Runs `themis --data-dir DATA ARGS` in `cwd` with `stdin`: success, stdout and stderr.
fn themis_in(data: &Path, cwd: &Path, args: &[&str], stdin: &str) -> (bool, String, String) {
    let mut child = std::process::Command::new(env!("CARGO_BIN_EXE_themis"))
        .arg("--data-dir")
        .arg(data)
        .args(args)
        .current_dir(cwd)
        .env_remove("OPENCODE_KEY")
        .env_remove("THEMIS_AGENT_TOKEN")
        .env_remove("THEMIS_AGENT_PORT")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    child
        .stdin
        .take()
        .unwrap()
        .write_all(stdin.as_bytes())
        .unwrap();
    let output = child.wait_with_output().unwrap();
    (
        output.status.success(),
        String::from_utf8_lossy(&output.stdout).into_owned(),
        String::from_utf8_lossy(&output.stderr).into_owned(),
    )
}

fn ok_in(data: &Path, cwd: &Path, args: &[&str], stdin: &str) -> Value {
    let (success, stdout, stderr) = themis_in(data, cwd, args, stdin);
    assert!(success, "themis {args:?} failed: {stderr}");
    serde_json::from_str(&stdout).unwrap_or(Value::Null)
}

fn ok(data: &Path, args: &[&str]) -> Value {
    ok_in(data, data, args, "")
}

fn fails(data: &Path, args: &[&str], stdin: &str) -> String {
    let (success, stdout, stderr) = themis_in(data, data, args, stdin);
    assert!(!success, "themis {args:?} should fail; printed {stdout}");
    stderr
}

async fn server(data: &Path) -> tokio::task::JoinHandle<()> {
    let state = AppState::new_for_test(data.join("settings.json"));
    let server = Server::bind(state, data).await.unwrap();
    tokio::spawn(async move {
        let _ = server.run().await;
    })
}

fn plugin(data: &Path, name: &str) -> Value {
    ok(data, &["plugin", "show", name])
}

/// The same server, hook and skill saved from a file, from stdin and inline land the same way;
/// a broken definition says which form it came from.
#[tokio::test(flavor = "multi_thread")]
async fn a_definition_is_a_file_stdin_or_inline_json() {
    let data = tempfile::tempdir().unwrap();
    let task = server(data.path()).await;
    let server_json = json!({"command": "printf", "args": ["ready"]}).to_string();
    let files = tempfile::tempdir().unwrap();
    let file = files.path().join("definition.json");
    std::fs::write(&file, &server_json).unwrap();
    ok(
        data.path(),
        &["mcp", "add", "demo", "from-file", file.to_str().unwrap()],
    );
    ok_in(
        data.path(),
        data.path(),
        &["mcp", "add", "demo", "from-stdin", "-"],
        &server_json,
    );
    ok(data.path(), &["mcp", "add", "demo", "inline", &server_json]);
    let mcp = &plugin(data.path(), "demo")["spec"]["mcp"];
    for name in ["from-file", "from-stdin", "inline"] {
        assert_eq!(mcp[name]["command"], "printf", "{name}: {mcp}");
        assert_eq!(mcp[name]["args"], json!(["ready"]), "{name}");
    }
    // Leading spaces before the brace still read as inline JSON.
    ok(
        data.path(),
        &[
            "mcp",
            "update",
            "demo",
            "inline",
            &format!("  {server_json}"),
        ],
    );
    // So do the single quotes of a shell command passed as one argument: the agent's shell
    // tool runs the program with its arguments as given, no shell in between.
    let quoted = format!("'{server_json}'");
    ok(data.path(), &["mcp", "add", "demo", "quoted", &quoted]);
    assert_eq!(
        plugin(data.path(), "demo")["spec"]["mcp"]["quoted"]["command"],
        "printf"
    );

    let hook = json!({"event": "BeforeTool", "command": "true"}).to_string();
    ok_in(
        data.path(),
        data.path(),
        &["hook", "add", "demo", "guard", "-"],
        &hook,
    );
    ok(data.path(), &["hook", "add", "demo", "inline-guard", &hook]);
    let hooks = plugin(data.path(), "demo")["spec"]["hooks"].clone();
    assert_eq!(hooks.as_array().unwrap().len(), 2, "{hooks}");
    assert!(hooks
        .as_array()
        .unwrap()
        .iter()
        .all(|h| h["enabled"] != true));

    let skill = json!({"id": "notes", "name": "Notes", "instructions": "Write notes."}).to_string();
    ok_in(data.path(), data.path(), &["skill", "verify", "-"], &skill);
    ok(data.path(), &["skill", "verify", &skill]);
    ok(data.path(), &["skill", "save", "demo", &skill]);
    let piped = skill.replace("notes", "piped");
    ok_in(
        data.path(),
        data.path(),
        &["skill", "save", "demo", "-"],
        &piped,
    );
    let skills = plugin(data.path(), "demo")["spec"]["skills"].clone();
    assert_eq!(skills.as_array().unwrap().len(), 2, "{skills}");

    let plugin_json = json!({"name": "whole", "skills": [], "mcp": {}, "hooks": []}).to_string();
    ok_in(
        data.path(),
        data.path(),
        &["plugin", "save", "-"],
        &plugin_json,
    );
    assert_eq!(plugin(data.path(), "whole")["spec"]["name"], "whole");

    let error = fails(
        data.path(),
        &["mcp", "add", "demo", "bad", "{\"command\": "],
        "",
    );
    assert!(
        error.contains("the inline JSON is not valid JSON"),
        "{error}"
    );
    let error = fails(
        data.path(),
        &["hook", "add", "demo", "bad", "-"],
        "not json",
    );
    assert!(error.contains("stdin is not valid JSON"), "{error}");
    let error = fails(data.path(), &["skill", "save", "demo", "missing.json"], "");
    assert!(
        error.contains("missing.json") && error.contains("- for stdin"),
        "{error}"
    );
    let error = fails(
        data.path(),
        &["mcp", "add", "demo", "shape", r#"{"cmd": "x"}"#],
        "",
    );
    assert!(
        error.contains("The MCP server in the inline JSON"),
        "{error}"
    );
    task.abort();
}

/// `mcp add PLUGIN DEF` with a whole `.mcp.json` adds every server, converted as an import
/// converts them and switched off; a server it cannot use fails the call (listed) unless
/// `--allow-partial`; with NAME it takes that one server.
#[tokio::test(flavor = "multi_thread")]
async fn a_whole_mcp_json_adds_every_server() {
    let data = tempfile::tempdir().unwrap();
    let task = server(data.path()).await;
    let config = json!({"mcpServers": {
        "fetch": {"command": "uvx", "args": ["mcp-server-fetch"], "env": {"LOG_LEVEL": "ERROR"}, "timeout": 5},
        "exa": {"type": "http", "url": "https://mcp.exa.ai/mcp", "headers": {"Authorization": "Bearer ${EXA_KEY}"}},
        "odd": {"command": "x", "transport": "pigeon"}
    }});
    let files = tempfile::tempdir().unwrap();
    let file = files.path().join(".mcp.json");
    std::fs::write(&file, config.to_string()).unwrap();
    let error = fails(
        data.path(),
        &["mcp", "add", "bulk", file.to_str().unwrap()],
        "",
    );
    assert!(
        error.contains("odd") && error.contains("transport") && error.contains("--allow-partial"),
        "{error}"
    );
    assert!(
        !ok(data.path(), &["plugin", "list"])
            .as_array()
            .unwrap()
            .iter()
            .any(|p| p["spec"]["name"] == "bulk"),
        "nothing is saved when the call fails"
    );
    let report = ok(
        data.path(),
        &[
            "mcp",
            "add",
            "bulk",
            file.to_str().unwrap(),
            "--allow-partial",
        ],
    );
    assert_eq!(report["added"], json!(["exa", "fetch"]), "{report}");
    let skipped = report["skipped"].as_array().unwrap();
    assert_eq!(skipped.len(), 1, "{report}");
    assert!(skipped[0].as_str().unwrap().contains("odd"), "{report}");
    assert!(
        report["notices"].to_string().contains("timeout"),
        "{report}"
    );
    let mcp = &plugin(data.path(), "bulk")["spec"]["mcp"];
    assert_eq!(mcp["fetch"]["env_values"]["LOG_LEVEL"], "ERROR", "{mcp}");
    assert_eq!(mcp["exa"]["url"], "https://mcp.exa.ai/mcp");
    assert_eq!(mcp["exa"]["bearer_env"], "EXA_KEY", "{mcp}");
    assert_eq!(mcp["fetch"]["enabled"], false);
    assert_eq!(mcp["exa"]["enabled"], false);
    assert!(mcp.get("odd").is_none());

    // Inline too, and a name the plugin already has is refused.
    let inline = json!({"mcpServers": {"memory": {"command": "npx", "args": ["-y", "m"]}}});
    let report = ok(data.path(), &["mcp", "add", "bulk", &inline.to_string()]);
    assert_eq!(report["added"], json!(["memory"]));
    let error = fails(
        data.path(),
        &["mcp", "add", "bulk", &inline.to_string()],
        "",
    );
    assert!(error.contains("already exists"), "{error}");

    // With NAME, only that server; a name the file lacks is refused with the names it has.
    ok(
        data.path(),
        &["mcp", "add", "one", "fetch", file.to_str().unwrap()],
    );
    let mcp = &plugin(data.path(), "one")["spec"]["mcp"];
    assert_eq!(mcp.as_object().unwrap().len(), 1, "{mcp}");
    assert_eq!(mcp["fetch"]["command"], "uvx");
    let error = fails(
        data.path(),
        &["mcp", "add", "one", "nope", file.to_str().unwrap()],
        "",
    );
    assert!(
        error.contains("no server 'nope'") && error.contains("fetch"),
        "{error}"
    );
    // A standard entry with its `type` is converted as well.
    let entry = json!({"type": "stdio", "command": "node", "args": ["s.js"]}).to_string();
    ok(data.path(), &["mcp", "add", "one", "typed", &entry]);
    assert_eq!(
        plugin(data.path(), "one")["spec"]["mcp"]["typed"]["command"],
        "node"
    );
    // Without NAME a single server is not a .mcp.json.
    let error = fails(
        data.path(),
        &["mcp", "add", "one", r#"{"command": "x"}"#],
        "",
    );
    assert!(error.contains("mcpServers"), "{error}");
    // Literal credentials are refused as for any typed configuration.
    let secret = json!({"mcpServers": {"bad": {"command": "run", "env": {"API_KEY": "abc"}}}});
    let error = fails(data.path(), &["mcp", "add", "one", &secret.to_string()], "");
    assert!(error.contains("literal values are unsupported"), "{error}");
    task.abort();
}

/// `mcp list`, `hook list` and `skill list` without a plugin list every one, global and in this
/// project, each with its plugin.
#[tokio::test(flavor = "multi_thread")]
async fn lists_without_a_plugin_show_every_one_with_its_plugin() {
    let data = tempfile::tempdir().unwrap();
    let project = tempfile::tempdir().unwrap();
    std::fs::create_dir(project.path().join(".themis")).unwrap();
    let task = server(data.path()).await;
    let root = project.path().to_str().unwrap();
    let server_json = r#"{"command": "printf"}"#;
    let hook = r#"{"event": "RunEnd", "command": "true"}"#;
    let skill = r#"{"id": "notes", "name": "Notes", "instructions": "Write notes."}"#;
    ok(data.path(), &["mcp", "add", "alpha", "one", server_json]);
    ok(data.path(), &["hook", "add", "alpha", "guard", hook]);
    ok(data.path(), &["skill", "save", "alpha", skill]);
    let local = ["--scope", "local", "--project", root];
    ok(
        data.path(),
        &[&["mcp", "add", "beta", "two", server_json][..], &local].concat(),
    );
    ok(
        data.path(),
        &[&["hook", "add", "beta", "watch", hook][..], &local].concat(),
    );

    // From the project folder, its integrations come with the global ones.
    let servers = ok_in(data.path(), project.path(), &["mcp", "list"], "");
    let rows: Vec<(String, String, String)> = servers
        .as_array()
        .unwrap()
        .iter()
        .map(|row| {
            assert_eq!(row["definition"]["command"], "printf", "{row}");
            (
                row["plugin"].as_str().unwrap().to_owned(),
                row["scope"].as_str().unwrap().to_owned(),
                row["name"].as_str().unwrap().to_owned(),
            )
        })
        .collect();
    assert!(
        rows.contains(&("alpha".into(), "global".into(), "one".into())),
        "{rows:?}"
    );
    assert!(
        rows.contains(&("beta".into(), "local".into(), "two".into())),
        "{rows:?}"
    );
    let hooks = ok_in(data.path(), project.path(), &["hook", "list"], "").to_string();
    assert!(
        hooks.contains("\"guard\"") && hooks.contains("\"watch\""),
        "{hooks}"
    );
    assert!(
        hooks.contains("\"alpha\"") && hooks.contains("\"beta\""),
        "{hooks}"
    );
    let skills = ok(data.path(), &["skill", "list"]);
    assert!(
        skills
            .as_array()
            .unwrap()
            .iter()
            .any(|s| s["name"] == "Notes" && s["plugin"] == "alpha"),
        "{skills}"
    );
    // Elsewhere, only the global ones.
    let global = ok(data.path(), &["mcp", "list"]).to_string();
    assert!(
        global.contains("\"one\"") && !global.contains("\"two\""),
        "{global}"
    );
    // With a plugin, that plugin's definitions in full, as before.
    assert_eq!(
        ok(data.path(), &["mcp", "list", "alpha"])["one"]["command"],
        "printf"
    );
    task.abort();
}

/// The `skill|mcp|hook import|inspect` spellings are gone; each says to use `plugin import`.
#[tokio::test(flavor = "multi_thread")]
async fn removed_import_spellings_point_to_plugin_import() {
    let data = tempfile::tempdir().unwrap();
    let task = server(data.path()).await;
    for (component, action) in [
        ("skill", "import"),
        ("skill", "inspect"),
        ("mcp", "import"),
        ("mcp", "inspect"),
        ("hook", "import"),
    ] {
        let error = fails(data.path(), &[component, action, "./somewhere", "name"], "");
        assert!(
            error.contains(&format!("themis plugin {action} SOURCE")) && error.contains("removed"),
            "{component} {action}: {error}"
        );
    }
    let (_, usage, _) = themis_in(data.path(), data.path(), &["mcp", "--help"], "");
    assert!(
        usage.contains("A missing PLUGIN is created") && usage.contains("- (stdin)"),
        "{usage}"
    );
    task.abort();
}
