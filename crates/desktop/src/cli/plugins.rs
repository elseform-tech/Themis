use crate::server::Client;
use serde_json::{json, Value};
use std::path::Path;
use themis_core::plugins::definitions::{self, Definition};

/// The plugin, skill, MCP and hook commands, for `themis plugin --help` and malformed calls.
#[rustfmt::skip]
pub const USAGE: &str = "Use plugin list|updates|create NAME|show NAME|edit NAME|save FILE|inspect SOURCE [NAME]|import SOURCE [NAME]|export NAME|preview NAME@MARKET|install NAME@MARKET|update NAME|enable NAME|disable NAME|uninstall NAME; plugin marketplace list|preview SOURCE|add SOURCE [NAME]|enable NAME|disable NAME|changes NAME|browse NAME|refresh NAME|check NAME|remove NAME; skill list|save PLUGIN FILE|verify FILE|delete PLUGIN ID; mcp/hook list PLUGIN|add PLUGIN NAME FILE|update PLUGIN NAME FILE|test PLUGIN NAME|delete PLUGIN NAME. Options: --scope local|global --project PATH --expected REVISION --ref REF --subdirectory PATH --allow-partial. For agent creation: skill create THREAD REQUEST. A missing PLUGIN is created. `themis skill|mcp|hook --help` shows each definition's exact JSON with every field.";

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
struct Options<'a> {
    positional: Vec<&'a str>,
    project: Option<String>,
    scope: String,
    expected: Option<String>,
    reference: Option<String>,
    subdirectory: Option<String>,
    allow_partial: bool,
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
            allow_partial: false,
        };
        let mut index = 0;
        while index < words.len() {
            match words[index].as_str() {
                "--allow-partial" => options.allow_partial = true,
                key @ ("--project" | "--scope" | "--expected" | "--ref" | "--subdirectory") => {
                    index += 1;
                    let value = words.get(index).ok_or("Missing option value")?.clone();
                    match key {
                        "--project" => options.project = Some(value),
                        "--scope" => options.scope = value,
                        "--ref" => options.reference = Some(value),
                        "--subdirectory" => options.subdirectory = Some(value),
                        _ => options.expected = Some(value),
                    }
                }
                value if value.starts_with('-') => return Err(format!("Unknown option {value}")),
                value => options.positional.push(value),
            }
            index += 1;
        }
        Ok(options)
    }
}

/// Reads a JSON definition file (at most 8 MiB).
fn read(path: &str) -> Result<Value, String> {
    let metadata = std::fs::metadata(path).map_err(|e| format!("{path}: {e}"))?;
    if metadata.len() > 8 * 1024 * 1024 {
        return Err("Configuration exceeds 8 MiB".into());
    }
    serde_json::from_slice(&std::fs::read(path).map_err(|e| e.to_string())?)
        .map_err(|e| format!("{path} is not valid JSON: {e}"))
}

/// Every problem with a definition before it is sent, with its exact shape.
fn checked(definition: Definition, value: &Value, path: &str) -> Result<(), String> {
    let problems = definition.problems(value);
    if problems.is_empty() {
        return Ok(());
    }
    Err(format!(
        "{}\n\n{}",
        definitions::report(&format!("The {} in {path}", definition.label()), &problems),
        definition.reference()
    ))
}

// One line per command shape, as before the usage text moved out (rustfmt skipped it then).
#[rustfmt::skip]
pub async fn run(client: &Client, words: &[String], directory: &Path) -> Result<Value, String> {
    let options = Options::parse(words)?;
    let (positional, project, reference, subdirectory) = (&options.positional, &options.project, &options.reference, &options.subdirectory);
    let mut request = json!({"scope":options.scope,"projectRoot":project,"expectedRevision":options.expected,"allowPartial":options.allow_partial});
    match positional.as_slice(){
        ["plugin","list"]=>request["action"]=json!("list"),
        ["plugin","create",name]=>{request["action"]=json!("save");request["spec"]=json!({"name":name});},
        ["plugin","save",file]=>{
            let spec=read(file)?;let problems=definitions::plugin_problems(&spec);
            if !problems.is_empty(){return Err(definitions::report(&format!("The plugin in {file}"),&problems));}
            request["action"]=json!("save");request["spec"]=spec;
        },
        [component,import_action,source] | [component,import_action,source,_] if matches!(*component,"plugin"|"skill"|"mcp"|"hook") && matches!(*import_action,"import"|"inspect")=>{
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
        ["plugin","marketplace","add",source]=>{request["action"]=json!("add_marketplace");request["source"]=json!(source);},
        ["plugin","marketplace","add",source,name]=>{request["action"]=json!("add_marketplace");request["source"]=json!(source);request["name"]=json!(name);},
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
        ["skill","list"]=>return client.call("list_prompt_skills",json!({"projectRoot":project})).await,
        ["skill","verify",file]=>return verify_skill(file,directory),
        [component,"list",plugin]|[component,"show",plugin] if matches!(*component,"skill"|"mcp"|"hook")=>{let p=find(client,&request,plugin).await?;return Ok(p["spec"][field(component)].clone());},
        [component,action,plugin,name,file] if matches!(*component,"mcp"|"hook")&&matches!(*action,"add"|"update")=>{
            put_component(client,&mut request,component,*action=="add",plugin,name,file).await?;
        },
        ["skill","save",plugin,file]=>{
            let skill=read(file)?;checked(Definition::Skill,&skill,file)?;
            let existing=find_or_new(client,&request,plugin).await?;
            request["action"]=json!("save_skill");request["plugin"]=json!(plugin);request["skill"]=skill;request["expectedRevision"]=existing["revision"].clone();
        },
        [component,action,plugin,name] if matches!(*component,"skill"|"mcp"|"hook")&&matches!(*action,"delete"|"remove"|"enable"|"disable"|"test")=>{
            let p=find(client,&request,plugin).await?;let spec=p["spec"].clone();
            if matches!(*action,"delete"|"remove") {
                request["action"]=json!("remove_component");request["name"]=json!(plugin);request["kind"]=json!(component);request["id"]=json!(name);
            } else if matches!(*action,"enable"|"disable") {
                request["action"]=json!("set_component_enabled");request["name"]=json!(plugin);request["kind"]=json!(component);request["id"]=json!(name);request["enabled"]=json!(*action=="enable");
            } else if *component!="skill" {
                request["action"]=json!(if *component=="mcp"{"test_mcp"}else{"test_hook"});
                request[if *component=="mcp"{"server"}else{"hook"}]=if *component=="mcp"{spec["mcp"].get(name).cloned().ok_or("Unknown connection")?}else{spec["hooks"].as_array().ok_or("Invalid hooks")?.iter().find(|h|h["name"]==*name).cloned().ok_or("Unknown hook")?};
            } else { return Err("Use skill verify FILE".into()); }

        },
        _=>return Err(USAGE.into()),
    }
    send(client, request, project.as_deref()).await
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

/// `skill verify FILE`: a JSON draft is checked as the store checks it, every problem at once; a
/// SKILL.md or skill folder goes through the bundled verifier.
fn verify_skill(file: &str, directory: &Path) -> Result<Value, String> {
    if file.ends_with(".json") {
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

/// `mcp|hook add|update PLUGIN NAME FILE`: checks the definition, then saves the plugin with it
/// (a new plugin when there is none by that name).
async fn put_component(
    client: &Client,
    request: &mut Value,
    component: &str,
    add: bool,
    plugin: &str,
    name: &str,
    file: &str,
) -> Result<(), String> {
    let mut value = read(file)?;
    let existing = find_or_new(client, request, plugin).await?;
    let mut spec = existing["spec"].clone();
    if component == "mcp" {
        checked(Definition::Mcp, &value, file)?;
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
        checked(Definition::Hook, &value, file)?;
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

/// Sends the request, except an agent's hook or MCP test, which runs here in its sandbox.
async fn send(client: &Client, request: Value, project: Option<&str>) -> Result<Value, String> {
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

fn field(component: &str) -> &str {
    match component {
        "skill" => "skills",
        "mcp" => "mcp",
        _ => "hooks",
    }
}
async fn find(client: &Client, args: &Value, name: &str) -> Result<Value, String> {
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
async fn find_or_new(client: &Client, args: &Value, name: &str) -> Result<Value, String> {
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
