//! `themis doctor --integrations` (agent CLI plan, step 4) through the real app server and the
//! `themis` binary with an isolated data folder and no credentials: every installed and custom
//! plugin, skill, hook and MCP server, global and in the project, with every problem named by
//! item and scope with a one-line fix, as JSON or text.
#![cfg(unix)]

use serde_json::{json, Value};
use std::path::Path;
use themis_desktop::{server::Server, state::AppState};

fn doctor(data: &Path, project: &Path, extra: &[&str]) -> (bool, String, String) {
    let output = std::process::Command::new(env!("CARGO_BIN_EXE_themis"))
        .arg("--data-dir")
        .arg(data)
        .args(["doctor", "--integrations", "--project"])
        .arg(project)
        .args(extra)
        .current_dir(project)
        .env_remove("OPENCODE_KEY")
        .env_remove(themis_core::agent_cli::TOKEN_ENV)
        .env_remove(themis_core::agent_cli::PORT_ENV)
        .output()
        .unwrap();
    (
        output.status.success(),
        String::from_utf8_lossy(&output.stdout).into_owned(),
        String::from_utf8_lossy(&output.stderr).into_owned(),
    )
}

/// A command-based MCP server on stdin and stdout with one tool, `ping`.
const MCP_STDIO_SERVER: &str = r#"import json, sys
for line in sys.stdin:
    message = json.loads(line)
    if "id" not in message:
        continue
    result = {
        "initialize": {"protocolVersion": "2025-03-26", "capabilities": {"tools": {}},
                       "serverInfo": {"name": "probe", "version": "1"}},
        "tools/list": {"tools": [{"name": "ping", "description": "Answers pong.",
                                  "inputSchema": {"type": "object"}}]},
    }.get(message["method"], {})
    print(json.dumps({"jsonrpc": "2.0", "id": message["id"], "result": result}), flush=True)
"#;

struct Fixture {
    data: tempfile::TempDir,
    project: tempfile::TempDir,
    task: tokio::task::JoinHandle<()>,
}

/// A global plugin `kit` with a fine skill and hook, a hook whose program is missing, one whose
/// script is missing, a server that starts, one that cannot, and one switched off; a project
/// plugin `local-kit` whose hook runs a script that is not executable; and in `kit` a skill
/// whose tool name the store no longer knows (written past the store, as an older version left
/// it).
async fn fixture() -> Fixture {
    let data = tempfile::tempdir().unwrap();
    let project = tempfile::tempdir().unwrap();
    let state = AppState::new_for_test(data.path().join("settings.json"));
    let server_script = project.path().join("server.py");
    std::fs::write(&server_script, MCP_STDIO_SERVER).unwrap();
    let hook = |name: &str, command: &str| json!({"name": name, "event": "BeforeTool", "command": command, "enabled": true});
    state
        .plugin_action(json!({"action": "save", "scope": "global", "spec": {
            "name": "kit",
            "skills": [{"id": "notes", "name": "Notes", "instructions": "Write notes."},
                       {"id": "old", "name": "Old", "instructions": "x"}],
            "hooks": [hook("fine", "true"), hook("gone", "no-such-program-for-doctor --check"),
                      hook("script", "python3 /no/such/doctor-script.py")],
            "mcp": {
                "good": {"command": "python3", "args": [server_script], "enabled": true},
                "broken": {"command": "no-such-mcp-for-doctor", "enabled": true},
                "off": {"command": "python3", "args": [server_script], "enabled": false}
            }
        }}))
        .await
        .unwrap();
    std::fs::write(project.path().join("check.sh"), "#!/bin/sh\nexit 0\n").unwrap();
    state
        .plugin_action(json!({"action": "save", "scope": "local",
            "projectRoot": project.path(), "spec": {
            "name": "local-kit", "hooks": [hook("plain", "./check.sh")]
        }}))
        .await
        .unwrap();
    // An older version allowed `list_dir` on every platform.
    let registry = data.path().join("plugins/registry.json");
    let mut stored: Value = serde_json::from_slice(&std::fs::read(&registry).unwrap()).unwrap();
    stored["current"]["kit"]["spec"]["skills"][1]["allowedTools"] = json!(["list_dir"]);
    std::fs::write(&registry, stored.to_string()).unwrap();
    let server = Server::bind(state, data.path()).await.unwrap();
    let task = tokio::spawn(async move {
        let _ = server.run().await;
    });
    Fixture {
        data,
        project,
        task,
    }
}

fn find<'a>(report: &'a Value, kind: &str, item: &str) -> Option<&'a Value> {
    report["problems"]
        .as_array()?
        .iter()
        .find(|finding| finding["kind"] == kind && finding["item"] == item)
}

/// Every problem comes back with its item, scope and a fix; working parts are not reported.
#[tokio::test(flavor = "multi_thread")]
async fn every_problem_has_its_item_scope_and_fix_and_working_parts_pass() {
    let fx = fixture().await;
    let (success, stdout, stderr) = doctor(fx.data.path(), fx.project.path(), &["--json"]);
    assert!(!success, "problems make it fail: {stdout}");
    let report: Value = serde_json::from_str(&stdout).unwrap_or_else(|_| {
        panic!("no JSON report: {stdout}\n{stderr}");
    });
    assert_eq!(report["ok"], false);
    assert_eq!(report["checked"]["plugins"], 2, "{report}");
    assert_eq!(report["checked"]["mcp_servers"], 3, "{report}");
    assert_eq!(report["checked"]["mcp_started"], 1, "{report}");
    let gone = find(&report, "hook", "kit/gone").expect("missing program reported");
    assert_eq!(gone["scope"], "global");
    assert!(gone["problem"]
        .as_str()
        .unwrap()
        .contains("no-such-program-for-doctor"));
    assert!(gone["fix"]
        .as_str()
        .unwrap()
        .contains("themis hook update kit gone"));
    assert!(find(&report, "hook", "kit/script").is_some(), "{report}");
    let plain = find(&report, "hook", "local-kit/plain").expect("not executable");
    assert_eq!(plain["scope"], "local");
    assert!(plain["problem"]
        .as_str()
        .unwrap()
        .contains("not executable"));
    let broken = find(&report, "mcp", "kit/broken").expect("server that cannot start");
    assert!(broken["fix"]
        .as_str()
        .unwrap()
        .contains("themis mcp test kit broken"));
    let old = find(&report, "plugin", "kit").expect("definition the store rejects now");
    assert!(
        old["problem"].as_str().unwrap().contains("list_dir"),
        "{old}"
    );
    for fine in ["kit/fine", "kit/good", "kit/notes"] {
        assert!(
            !report["problems"]
                .to_string()
                .contains(&format!("\"{fine}\"")),
            "{fine} works: {report}"
        );
    }
    let off = report["not_started"].to_string();
    assert!(
        off.contains("kit/off") && off.contains("switched off"),
        "{off}"
    );
    assert_eq!(report["problems"].as_array().unwrap().len(), 5, "{report}");
    fx.task.abort();
}

/// `--no-start` starts no server, so it is only a read; the text report lists each problem with
/// its fix.
#[tokio::test(flavor = "multi_thread")]
async fn without_starting_servers_the_text_report_still_names_each_problem() {
    let fx = fixture().await;
    let (success, stdout, _) = doctor(fx.data.path(), fx.project.path(), &["--no-start"]);
    assert!(!success);
    assert!(stdout.contains("(0 started)"), "{stdout}");
    assert!(stdout.contains("FAIL hook kit/gone (global)"), "{stdout}");
    assert!(stdout.contains("  fix: "), "{stdout}");
    assert!(
        !stdout.contains("kit/broken (global): did not start"),
        "{stdout}"
    );
    assert!(stdout.contains("not started: mcp kit/good"), "{stdout}");
    fx.task.abort();
}

/// With nothing wrong it passes, and says so.
#[tokio::test(flavor = "multi_thread")]
async fn a_clean_setup_passes() {
    let data = tempfile::tempdir().unwrap();
    let project = tempfile::tempdir().unwrap();
    let state = AppState::new_for_test(data.path().join("settings.json"));
    state
        .plugin_action(json!({"action": "save", "scope": "global", "spec": {
            "name": "tidy",
            "skills": [{"id": "notes", "name": "Notes", "instructions": "Write notes."}],
            "hooks": [{"name": "fine", "event": "RunStart", "command": "true"}]
        }}))
        .await
        .unwrap();
    let server = Server::bind(state, data.path()).await.unwrap();
    let task = tokio::spawn(async move {
        let _ = server.run().await;
    });
    let (success, stdout, stderr) = doctor(data.path(), project.path(), &[]);
    assert!(success, "{stdout}\n{stderr}");
    assert!(stdout.contains("No problems found."), "{stdout}");
    task.abort();
}
