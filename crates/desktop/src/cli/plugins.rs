use super::components;
use crate::server::Client;
use serde_json::{json, Value};
use std::path::Path;
use themis_core::plugins::definitions::{self, Definition};

/// The plugin, skill, MCP and hook commands, for `themis plugin --help` and malformed calls.
#[rustfmt::skip]
pub const USAGE: &str = "Use plugin list [--kind subagent]|updates|create NAME|show NAME|edit NAME|save DEF|inspect SOURCE [NAME]|import SOURCE [NAME]|export NAME|preview NAME@MARKET|install NAME@MARKET|update NAME|enable NAME|disable NAME|uninstall NAME (SOURCE: a folder, repository or .mcp.json file); plugin marketplace list|preview SOURCE|add SOURCE [NAME]|enable NAME|disable NAME|changes NAME|browse NAME|refresh NAME|check NAME|remove NAME; skill list [PLUGIN]|save PLUGIN DEF|verify DEF|delete PLUGIN ID; mcp|hook list [PLUGIN]|add PLUGIN NAME DEF|update PLUGIN NAME DEF|test PLUGIN NAME|delete PLUGIN NAME; mcp add PLUGIN DEF adds every server of a .mcp.json ({\"mcpServers\": ...}). A missing PLUGIN is created by add and save. DEF is a file, - (stdin) or inline JSON: themis mcp add docs fetch '{\"command\":\"uvx\",\"args\":[\"mcp-server-fetch\"]}'. Options: --scope local|global --project PATH --expected REVISION --ref REF --subdirectory PATH --allow-partial. For agent creation: skill create THREAD REQUEST. `themis skill|mcp|hook --help` shows each definition's exact JSON with every field.";

/// `--help` for `plugin`, `skill`, `mcp` or `hook`: the usage, and for a component its exact
/// definition (every field, required or optional, and an example).
pub fn help(command: &str) -> String {
    match definition(command) {
        Some(definition) => format!("{USAGE}\n\n{}", definition.reference()),
        None => USAGE.to_owned(),
    }
}

fn definition(component: &str) -> Option<Definition> {
    match component {
        "skill" => Some(Definition::Skill),
        "hook" => Some(Definition::Hook),
        "mcp" => Some(Definition::Mcp),
        _ => None,
    }
}

/// The options every integration command takes, anywhere among its words.
pub(super) struct Options<'a> {
    pub(super) positional: Vec<&'a str>,
    pub(super) project: Option<String>,
    scope: String,
    expected: Option<String>,
    reference: Option<String>,
    subdirectory: Option<String>,
    kind: Option<String>,
    pub(super) allow_partial: bool,
}

impl<'a> Options<'a> {
    fn parse(words: &'a [String]) -> Result<Self, String> {
        let mut options = Self {
            positional: Vec::new(),
            project: None,
            scope: "global".to_owned(),
            expected: None,
            reference: None,
            subdirectory: None,
            kind: None,
            allow_partial: false,
        };
        let mut index = 0;
        while index < words.len() {
            match words[index].as_str() {
                "--allow-partial" => options.allow_partial = true,
                key @ ("--project" | "--scope" | "--expected" | "--ref" | "--subdirectory"
                | "--kind") => {
                    index += 1;
                    let value = words.get(index).ok_or("Missing option value")?.clone();
                    match key {
                        "--project" => options.project = Some(value),
                        "--scope" => options.scope = value,
                        "--ref" => options.reference = Some(value),
                        "--subdirectory" => options.subdirectory = Some(value),
                        "--kind" => options.kind = Some(value),
                        _ => options.expected = Some(value),
                    }
                }
                // A lone `-` is a definition read from stdin.
                value if value.starts_with('-') && value != "-" => {
                    return Err(format!("Unknown option {value}"))
                }
                value => options.positional.push(value),
            }
            index += 1;
        }
        Ok(options)
    }
}

/// The largest definition read from a file, stdin or the command line.
const MAX_DEFINITION: usize = 8 * 1024 * 1024;

/// Reads a JSON definition (at most 8 MiB) from `source`: `-` is stdin, text starting with `{` is
/// the JSON itself, anything else a file path. Errors name the form that failed.
pub(super) fn read(source: &str) -> Result<Value, String> {
    let bytes = if source == "-" {
        use std::io::Read;
        let mut input = Vec::new();
        std::io::stdin()
            .take(MAX_DEFINITION as u64 + 1)
            .read_to_end(&mut input)
            .map_err(|e| format!("Cannot read the definition from stdin: {e}"))?;
        input
    } else if is_inline(source) {
        source.as_bytes().to_vec()
    } else {
        let metadata = std::fs::metadata(source).map_err(|e| {
            format!("{source}: {e} (a definition is a file path, - for stdin, or JSON starting with {{)")
        })?;
        if metadata.len() > MAX_DEFINITION as u64 {
            return Err(format!("{source} exceeds 8 MiB"));
        }
        std::fs::read(source).map_err(|e| format!("{source}: {e}"))?
    };
    if bytes.len() > MAX_DEFINITION {
        return Err(format!("{} exceeds 8 MiB", label(source)));
    }
    serde_json::from_slice(&bytes).map_err(|e| format!("{} is not valid JSON: {e}", label(source)))
}

fn is_inline(source: &str) -> bool {
    source.trim_start().starts_with('{')
}

/// How a definition's source is named in messages.
pub(super) fn label(source: &str) -> String {
    if source == "-" {
        "stdin".into()
    } else if is_inline(source) {
        "the inline JSON".into()
    } else {
        source.into()
    }
}

// One line per command shape, as before the usage text moved out (rustfmt skipped it then).
#[rustfmt::skip]
pub async fn run(client: &Client, words: &[String], directory: &Path) -> Result<Value, String> {
    let options = Options::parse(words)?;
    let (positional, project, reference, subdirectory) = (&options.positional, &options.project, &options.reference, &options.subdirectory);
    let mut request = json!({"scope":options.scope,"projectRoot":project,"expectedRevision":options.expected,"allowPartial":options.allow_partial});
    match positional.as_slice(){
        ["plugin","list"]=>return list(client,request,options.kind.as_deref(),project.as_deref()).await,
        ["plugin","create",name]=>{request["action"]=json!("save");request["spec"]=json!({"name":name});},
        ["plugin","save",file]=>{
            let spec=read(file)?;let problems=definitions::plugin_problems(&spec);
            if !problems.is_empty(){return Err(definitions::report(&format!("The plugin in {}",label(file)),&problems));}
            request["action"]=json!("save");request["spec"]=spec;
        },
        ["plugin",import_action,source] | ["plugin",import_action,source,_] if matches!(*import_action,"import"|"inspect")=>{
            if source.starts_with("https://") || source.starts_with("http://") || reference.is_some() || subdirectory.is_some() {
                request["action"]=json!(if *import_action=="inspect" {"inspect_repository"}else{"import_repository"}); request["url"]=json!(source);
                request["name"]=json!(positional.get(3).copied().unwrap_or(""));
                request["reference"]=json!(reference);request["subdirectory"]=json!(subdirectory);
            } else { request["action"]=json!(if *import_action=="inspect" {"inspect_path"}else{"import_path"});request["path"]=json!(source);request["name"]=json!(positional.get(3)); }
        },
        ["plugin",action,name] if matches!(*action,"enable"|"disable"|"delete"|"uninstall")=>{request["action"]=json!(if *action=="uninstall"{"delete"}else{action});request["name"]=json!(name);},
        ["plugin","marketplace","list"]=>request["action"]=json!("sources"),
        ["plugin","updates"]=>request["action"]=json!("updates"),
        ["plugin","marketplace","changes",name]=>{request["action"]=json!("check_source");request["name"]=json!(name);},
        ["plugin","marketplace","preview",source]=>{request["action"]=json!("preview_source");request["source"]=json!(source);},
        ["plugin","marketplace","add",source] | ["plugin","marketplace","add",source,_]=>{request["action"]=json!("add_marketplace");request["source"]=json!(source);request["name"]=json!(positional.get(4));},
        ["plugin","marketplace",switch,name] if matches!(*switch,"enable"|"disable")=>{request["action"]=json!("set_marketplace_enabled");request["name"]=json!(name);request["enabled"]=json!(*switch=="enable");},
        ["plugin","marketplace","remove",name]=>{request["action"]=json!("remove_marketplace");request["name"]=json!(name);},
        ["plugin","marketplace",action,name] if matches!(*action,"browse"|"refresh")=>{request["action"]=json!("catalog");request["name"]=json!(name);request["refresh"]=json!(*action=="refresh");},
        ["plugin","marketplace","check",name]=>{request["action"]=json!("catalog_check");request["name"]=json!(name);},
        ["plugin",action @ ("install"|"preview"),target]=>{
            let(name,market)=target.split_once('@').ok_or("Use NAME@MARKETPLACE (themis plugin marketplace list shows the marketplaces)")?;
            request["action"]=json!(if *action=="install"{"install"}else{"inspect_marketplace"});request["name"]=json!(name);request["marketplace"]=json!(market);
        },
        ["plugin",action,name] if matches!(*action,"show"|"export"|"edit"|"update")=>{
            let plugin=find(client,&request,name).await?;
            if *action=="show"{return Ok(plugin);}
            if *action=="export"{return Ok(plugin["spec"].clone());}
            if *action=="update"{request["action"]=json!("update");request["name"]=json!(name);request["marketplace"]=plugin["source"].clone();}
            else { return edit(client,request,&plugin,directory).await; }
        },
        [component,..] if matches!(*component,"skill"|"mcp"|"hook")=>return components::run(client,request,&options,directory).await,
        _=>return Err(USAGE.into()),
    }
    send(client, request, project.as_deref()).await
}

/// `plugin list`: every installed plugin in full, or with `--kind subagent` their profiles.
async fn list(
    client: &Client,
    mut request: Value,
    kind: Option<&str>,
    project: Option<&str>,
) -> Result<Value, String> {
    if kind.is_some() {
        return subagent_profiles(client, kind, project).await;
    }
    request["action"] = json!("list");
    send(client, request, project).await
}

/// `plugin list --kind subagent`: every subagent profile of the installed plugins, global and in
/// the project, with its plugin, whether that plugin is on, its model (none: the subagent default),
/// tools and whether it only reads; no instructions.
async fn subagent_profiles(
    client: &Client,
    kind: Option<&str>,
    project: Option<&str>,
) -> Result<Value, String> {
    if kind != Some("subagent") {
        return Err("--kind takes subagent (themis mcp|hook|skill list lists the others)".into());
    }
    let mut profiles = Vec::new();
    let mut offset = Some(0);
    while let Some(start) = offset {
        let page = client
            .call(
                "plugin_action",
                json!({"action": "inventory", "kind": "subagent", "offset": start, "limit": 100,
                    "projectRoot": project}),
            )
            .await?;
        for plugin in page["items"].as_array().into_iter().flatten() {
            for profile in plugin["spec"]["subagents"].as_array().into_iter().flatten() {
                let mut row = json!({"plugin": plugin["spec"]["name"], "scope": plugin["scope"],
                    "enabled": plugin["enabled"]});
                if let (Some(row), Some(fields)) = (row.as_object_mut(), profile.as_object()) {
                    row.extend(fields.clone());
                }
                profiles.push(row);
            }
        }
        offset = page["next_offset"].as_u64();
    }
    Ok(Value::Array(profiles))
}

/// `plugin edit NAME`: the definition opens in `$EDITOR`, then is saved against its revision.
async fn edit(
    client: &Client,
    mut request: Value,
    plugin: &Value,
    directory: &Path,
) -> Result<Value, String> {
    if !plugin["source"].is_null() {
        return Err("Export to a personal copy before editing a marketplace plugin".into());
    }
    let path = directory.join(format!("plugin-edit-{}.json", uuid::Uuid::new_v4()));
    std::fs::write(
        &path,
        serde_json::to_vec_pretty(&plugin["spec"]).map_err(|e| e.to_string())?,
    )
    .map_err(|e| e.to_string())?;
    let editor = std::env::var("EDITOR").unwrap_or_else(|_| "vi".into());
    let mut parts = editor.split_whitespace();
    let executable = parts.next().ok_or("EDITOR is empty")?;
    let status = std::process::Command::new(executable)
        .args(parts)
        .arg(&path)
        .status()
        .map_err(|e| e.to_string())?;
    if !status.success() {
        return Err(format!("Editor failed; draft kept at {}", path.display()));
    }
    request["spec"] = read(path.to_str().ok_or("Invalid draft path")?)?;
    request["action"] = json!("save");
    request["expectedRevision"] = plugin["revision"].clone();
    let saved = client.call("plugin_action", request).await;
    if saved.is_ok() {
        let _ = std::fs::remove_file(&path);
    }
    saved
}

/// Sends the request, except an agent's hook or MCP test, which runs here in its sandbox.
pub(super) async fn send(
    client: &Client,
    request: Value,
    project: Option<&str>,
) -> Result<Value, String> {
    let test = matches!(request["action"].as_str(), Some("test_mcp" | "test_hook"));
    if test && crate::server::in_agent_shell() {
        return sandboxed_test(&request, project).await;
    }
    client.call("plugin_action", request).await
}
/// In an agent's shell the server refuses to start a test's command outside the sandbox, so
/// `themis mcp|hook test` runs it here, in the run's sandbox, with nothing the shell itself lacks.
async fn sandboxed_test(request: &Value, project: Option<&str>) -> Result<Value, String> {
    let root = match project {
        Some(project) => std::path::PathBuf::from(project),
        None => std::env::current_dir().map_err(|e| e.to_string())?,
    };
    let action = request["action"].as_str().unwrap_or_default();
    match crate::state::definition_test::run(action, request, &root, true).await {
        Ok(mut value) => {
            if let Some(object) = value.as_object_mut() {
                object.insert("sandboxed".into(), json!(true));
            } else {
                value = json!({"sandboxed": true, "result": value});
            }
            Ok(value)
        }
        Err(error) => Err(themis_core::prompts::fill(
            themis_core::prompts::AGENT_CLI_SANDBOXED_TEST_FAILED,
            &[("error", &format!("{error:#}"))],
        )),
    }
}

pub(super) async fn find(client: &Client, args: &Value, name: &str) -> Result<Value, String> {
    let plugin = find_or_new(client, args, name).await?;
    if plugin.get("scope").is_none() {
        return Err(format!(
            "Unknown plugin '{name}' in {} scope (themis plugin list shows them)",
            args["scope"].as_str().unwrap_or("global")
        ));
    }
    Ok(plugin)
}
/// The plugin by this name in the request's scope, or a new, empty one (no `scope`, no
/// `revision`) to save into.
pub(super) async fn find_or_new(
    client: &Client,
    args: &Value,
    name: &str,
) -> Result<Value, String> {
    let list = client
        .call(
            "plugin_action",
            json!({"action":"list","projectRoot":args["projectRoot"]}),
        )
        .await?;
    let found = list
        .as_array()
        .ok_or("Invalid plugin list")?
        .iter()
        .find(|p| p["scope"] == args["scope"] && p["spec"]["name"] == name)
        .cloned();
    Ok(found
        .unwrap_or_else(|| json!({"spec": {"name": name, "skills": [], "mcp": {}, "hooks": []}})))
}
