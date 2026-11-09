//! `themis skill|mcp|hook …`: one skill, MCP server or hook at a time, through the store. A
//! definition (DEF) is a file, `-` for stdin, or the JSON itself; `mcp add PLUGIN DEF` takes a
//! whole standard `.mcp.json` and adds every server in it, converted as `plugin import` converts
//! one. Folders, repositories and `.mcp.json` files that become a plugin of their own go through
//! `plugin import`.
use super::plugins::{find, find_or_new, label, read, send, Options, USAGE};
use crate::server::Client;
use serde_json::{json, Value};
use std::path::{Path, PathBuf};
use themis_core::plugins::definitions::{self, Definition};
use themis_core::plugins::{mcp_spec, PluginSpec};

pub(super) async fn run(
    client: &Client,
    mut request: Value,
    options: &Options<'_>,
    directory: &Path,
) -> Result<Value, String> {
    let project = options.project.as_deref();
    match options.positional.as_slice() {
        [component, action @ ("import" | "inspect"), ..] => Err(removed(component, action)),
        ["skill", "list"] => {
            let root = here_project(project);
            client
                .call("list_prompt_skills", json!({"projectRoot": root}))
                .await
        }
        [component @ ("mcp" | "hook"), "list"] => list_all(client, component, project).await,
        ["skill", "verify", file] => verify_skill(file, directory),
        [component, "list" | "show", plugin] => {
            let found = find(client, &request, plugin).await?;
            Ok(found["spec"][field(component)].clone())
        }
        ["mcp", "add", plugin, source] => {
            let report = add_servers(client, &mut request, plugin, source, options).await?;
            let saved = send(client, request, project).await?;
            Ok(
                json!({"added": report.0, "skipped": report.1, "notices": report.2, "plugin": saved}),
            )
        }
        [component @ ("mcp" | "hook"), action @ ("add" | "update"), plugin, name, source] => {
            let add = *action == "add";
            put_component(client, &mut request, component, add, plugin, name, source).await?;
            send(client, request, project).await
        }
        ["skill", "save", plugin, source] => {
            let skill = read(source)?;
            checked(Definition::Skill, &skill, source)?;
            let existing = find_or_new(client, &request, plugin).await?;
            request["action"] = json!("save_skill");
            request["plugin"] = json!(plugin);
            request["skill"] = skill;
            request["expectedRevision"] = existing["revision"].clone();
            send(client, request, project).await
        }
        [component, action, plugin, name] => {
            change(client, &mut request, component, action, plugin, name).await?;
            send(client, request, project).await
        }
        _ => Err(USAGE.into()),
    }
}

/// `skill|mcp|hook import|inspect` were removed: a source that becomes a plugin of its own goes
/// through `plugin import|inspect`, one definition through `add` or `save`.
fn removed(component: &str, action: &str) -> String {
    let one = if component == "skill" {
        "themis skill save PLUGIN DEF"
    } else {
        "themis mcp|hook add PLUGIN NAME DEF"
    };
    format!(
        "`themis {component} {action}` was removed. Use `themis plugin {action} SOURCE [NAME]` for a folder, repository or .mcp.json file, or `{one}` (DEF: a file, - or inline JSON) for one definition."
    )
}

/// The project whose integrations a list shows besides the global ones: `--project`, or the
/// current folder when it has a `.themis` folder.
fn here_project(project: Option<&str>) -> Option<PathBuf> {
    if let Some(project) = project {
        return Some(PathBuf::from(project));
    }
    let here = std::env::current_dir().ok()?;
    here.join(".themis").is_dir().then_some(here)
}

/// `mcp|hook list` without a plugin: every server or hook, global and in this project, each with
/// its plugin and scope.
async fn list_all(
    client: &Client,
    component: &str,
    project: Option<&str>,
) -> Result<Value, String> {
    let root = here_project(project);
    let list = client
        .call(
            "plugin_action",
            json!({"action": "list", "projectRoot": root}),
        )
        .await?;
    let mut rows = Vec::new();
    for plugin in list.as_array().ok_or("Invalid plugin list")? {
        let (name, scope, spec) = (&plugin["spec"]["name"], &plugin["scope"], &plugin["spec"]);
        let items: Vec<(Value, &Value)> = if component == "mcp" {
            let servers = spec["mcp"].as_object().into_iter().flatten();
            servers.map(|(id, server)| (json!(id), server)).collect()
        } else {
            let hooks = spec["hooks"].as_array().into_iter().flatten();
            hooks.map(|hook| (hook["name"].clone(), hook)).collect()
        };
        for (id, definition) in items {
            rows.push(
                json!({"plugin": name, "scope": scope, "name": id, "definition": definition}),
            );
        }
    }
    Ok(Value::Array(rows))
}

/// `skill|mcp|hook delete|remove|enable|disable|test PLUGIN NAME`.
async fn change(
    client: &Client,
    request: &mut Value,
    component: &str,
    action: &str,
    plugin: &str,
    name: &str,
) -> Result<(), String> {
    let kind = match (component, action) {
        ("skill" | "mcp" | "hook", "delete" | "remove") => "remove_component",
        ("skill" | "mcp" | "hook", "enable" | "disable") => "set_component_enabled",
        ("mcp", "test") => "test_mcp",
        ("hook", "test") => "test_hook",
        ("skill", "test") => return Err("Use skill verify DEF".into()),
        _ => return Err(USAGE.into()),
    };
    let found = find(client, request, plugin).await?;
    request["action"] = json!(kind);
    match kind {
        "test_mcp" => {
            let server = found["spec"]["mcp"].get(name).cloned();
            request["server"] = server.ok_or("Unknown connection")?;
        }
        "test_hook" => {
            let hooks = found["spec"]["hooks"].as_array().ok_or("Invalid hooks")?;
            let hook = hooks.iter().find(|h| h["name"] == *name).cloned();
            request["hook"] = hook.ok_or("Unknown hook")?;
        }
        _ => {
            request["name"] = json!(plugin);
            request["kind"] = json!(component);
            request["id"] = json!(name);
            if kind == "set_component_enabled" {
                request["enabled"] = json!(action == "enable");
            }
        }
    }
    Ok(())
}

/// `skill verify DEF`: a JSON draft (a `.json` file, `-` or inline JSON) is checked as the store
/// checks it, every problem at once; a SKILL.md or skill folder goes through the bundled verifier.
fn verify_skill(file: &str, directory: &Path) -> Result<Value, String> {
    if file.ends_with(".json") || label(file) != file {
        checked(Definition::Skill, &read(file)?, file)?;
        return Ok(json!({"valid": true}));
    }
    let path = directory.join("builtins/skills/verify.py");
    std::fs::create_dir_all(path.parent().ok_or("Missing verifier directory")?)
        .map_err(|e| e.to_string())?;
    std::fs::write(
        &path,
        include_str!("../../../core/builtins/skills/scripts/verify.py"),
    )
    .map_err(|e| e.to_string())?;
    let output = std::process::Command::new("python3")
        .arg(path)
        .arg(file)
        .output()
        .map_err(|e| e.to_string())?;
    if !output.status.success() {
        return Err(String::from_utf8_lossy(&output.stderr).into());
    }
    Ok(json!({"valid": true}))
}

/// Whether an MCP definition is in the standard `.mcp.json` shape (a whole file, or one entry
/// with its `type`) rather than Themis's own.
fn standard_mcp(value: &Value) -> bool {
    ["mcpServers", "mcp", "type"]
        .iter()
        .any(|key| value.get(key).is_some())
}

/// The connections of a standard `.mcp.json`, converted as `plugin import` converts them.
fn converted(document: &Value, source: &str, allow_partial: bool) -> Result<PluginSpec, String> {
    mcp_spec("converted", document, allow_partial)
        .map_err(|error| format!("{}: {error:#}", label(source)))
}

/// One server from a standard definition: an entry with its `type`, or the server called `name`
/// in a whole `.mcp.json`.
fn standard_server(name: &str, value: &Value, source: &str) -> Result<Value, String> {
    let entry = match value.get("mcpServers").or_else(|| value.get("mcp")) {
        Some(servers) => {
            let found = servers.get(name).cloned();
            found.ok_or_else(|| {
                let names: Vec<&String> = servers.as_object().into_iter().flatten().map(|(n, _)| n).collect();
                format!(
                    "{} has no server '{name}' (it has {names:?}); `themis mcp add PLUGIN DEF` without NAME adds every one",
                    label(source)
                )
            })?
        }
        None => value.clone(),
    };
    let spec = converted(&json!({"mcpServers": {name: entry}}), source, false)?;
    let server = spec
        .mcp
        .get(name)
        .ok_or("The server could not be converted")?;
    serde_json::to_value(server).map_err(|e| e.to_string())
}

/// What `mcp add PLUGIN DEF` added, skipped (one line per server with its field and reason) and
/// noted.
type AddReport = (Vec<String>, Vec<String>, Vec<String>);

/// `mcp add PLUGIN DEF` with a whole `.mcp.json`: every server is added to the plugin, switched
/// off. A server this cannot convert fails the call, listed with the others, unless
/// `--allow-partial` adds the rest.
async fn add_servers(
    client: &Client,
    request: &mut Value,
    plugin: &str,
    source: &str,
    options: &Options<'_>,
) -> Result<AddReport, String> {
    let document = read(source)?;
    if document.get("mcpServers").is_none() && document.get("mcp").is_none() {
        return Err(format!(
            "{} has no \"mcpServers\" object. For one server use `themis mcp add PLUGIN NAME DEF`; without NAME, DEF is a whole .mcp.json ({{\"mcpServers\": {{...}}}}).",
            label(source)
        ));
    }
    let spec = converted(&document, source, true)?;
    let skipped: Vec<String> = spec
        .import_issues
        .iter()
        .map(|issue| {
            let field = issue.field.as_deref().unwrap_or(&issue.name);
            let reason = issue.reason.as_deref().unwrap_or("unsupported");
            format!("{}: {field}: {reason}", issue.name)
        })
        .collect();
    if !skipped.is_empty() && !options.allow_partial {
        return Err(format!(
            "{} has servers Themis cannot use:\n{}\nFix them, or add the others with --allow-partial.",
            label(source),
            skipped.join("\n")
        ));
    }
    if spec.mcp.is_empty() {
        return Err(format!("{} has no server to add", label(source)));
    }
    let existing = find_or_new(client, request, plugin).await?;
    let mut saved = existing["spec"].clone();
    for (name, server) in &spec.mcp {
        if saved["mcp"].get(name).is_some() {
            return Err(format!(
                "Connection '{name}' already exists in '{plugin}'; use mcp update PLUGIN {name} DEF"
            ));
        }
        saved["mcp"][name] = serde_json::to_value(server).map_err(|e| e.to_string())?;
    }
    request["action"] = json!("save");
    request["spec"] = saved;
    request["expectedRevision"] = existing["revision"].clone();
    Ok((spec.mcp.keys().cloned().collect(), skipped, spec.notices))
}

/// `mcp|hook add|update PLUGIN NAME DEF`: checks the definition, then saves the plugin with it
/// (a new plugin when there is none by that name). An MCP definition in the standard shape (one
/// entry with its `type`, or a whole `.mcp.json` holding NAME) is converted as an import would.
async fn put_component(
    client: &Client,
    request: &mut Value,
    component: &str,
    add: bool,
    plugin: &str,
    name: &str,
    source: &str,
) -> Result<(), String> {
    let mut value = read(source)?;
    let existing = find_or_new(client, request, plugin).await?;
    let mut spec = existing["spec"].clone();
    if component == "mcp" {
        if standard_mcp(&value) {
            value = standard_server(name, &value, source)?;
        } else {
            checked(Definition::Mcp, &value, source)?;
        }
        if add && spec["mcp"].get(name).is_some() {
            return Err(format!(
                "Connection '{name}' already exists in '{plugin}'; use mcp update"
            ));
        }
        spec["mcp"][name] = value;
    } else {
        if let Some(object) = value.as_object_mut() {
            object.insert("name".into(), json!(name));
        }
        checked(Definition::Hook, &value, source)?;
        let hooks = spec["hooks"].as_array_mut().ok_or("Invalid hook list")?;
        if add && hooks.iter().any(|h| h["name"] == *name) {
            return Err(format!(
                "Hook '{name}' already exists in '{plugin}'; use hook update"
            ));
        }
        hooks.retain(|h| h["name"] != *name);
        hooks.push(value);
    }
    request["action"] = json!("save");
    request["spec"] = spec;
    request["expectedRevision"] = existing["revision"].clone();
    Ok(())
}

fn field(component: &str) -> &str {
    match component {
        "skill" => "skills",
        "mcp" => "mcp",
        _ => "hooks",
    }
}

/// Every problem with a definition before it is sent, with its exact shape.
fn checked(definition: Definition, value: &Value, source: &str) -> Result<(), String> {
    let problems = definition.problems(value);
    if problems.is_empty() {
        return Ok(());
    }
    let subject = format!("The {} in {}", definition.label(), label(source));
    Err(format!(
        "{}\n\n{}",
        definitions::report(&subject, &problems),
        definition.reference()
    ))
}
