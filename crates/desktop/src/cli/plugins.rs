use crate::server::Client;
use serde_json::{json, Value};
use std::path::Path;

pub async fn run(client: &Client, words: &[String], directory: &Path) -> Result<Value, String> {
    let mut positional = Vec::new();
    let mut project = None;
    let mut scope = "global".to_owned();
    let mut expected = None;
    let mut reference = None;
    let mut subdirectory = None;
    let mut index = 0;
    while index < words.len() {
        match words[index].as_str() {
            "--project" | "--scope" | "--expected" | "--ref" | "--subdirectory" => {
                let key = &words[index];
                index += 1;
                let value = words.get(index).ok_or("Missing option value")?.clone();
                match key.as_str() {
                    "--project" => project = Some(value),
                    "--scope" => scope = value,
                    "--ref" => reference = Some(value),
                    "--subdirectory" => subdirectory = Some(value),
                    _ => expected = Some(value),
                }
            }
            value if value.starts_with('-') => return Err(format!("Unknown option {value}")),
            _ => positional.push(words[index].as_str()),
        }
        index += 1;
    }
    let mut request = json!({"scope":scope,"projectRoot":project,"expectedRevision":expected});
    let read = |path: &str| -> Result<Value, String> {
        let metadata = std::fs::metadata(path).map_err(|e| e.to_string())?;
        if metadata.len() > 8 * 1024 * 1024 {
            return Err("Configuration exceeds 8 MiB".into());
        }
        serde_json::from_slice(&std::fs::read(path).map_err(|e| e.to_string())?)
            .map_err(|e| format!("Invalid configuration: {e}"))
    };
    match positional.as_slice(){
        ["plugin","list"]=>request["action"]=json!("list"),
        ["plugin","create",name]=>{request["action"]=json!("save");request["spec"]=json!({"name":name});},
        ["plugin","save",file]=>{request["action"]=json!("save");request["spec"]=read(file)?;},
        [component,"import",source] | [component,"import",source,_] if matches!(*component,"plugin"|"skill"|"mcp"|"hook")=>{
            if source.starts_with("https://") || source.starts_with("http://") || reference.is_some() || subdirectory.is_some() {
                request["action"]=json!("import_repository"); request["url"]=json!(source);
                request["name"]=json!(positional.get(3).copied().unwrap_or("imported"));
                request["reference"]=json!(reference);request["subdirectory"]=json!(subdirectory);
            } else { request["action"]=json!("import_path");request["path"]=json!(source);request["name"]=json!(positional.get(3)); }
        },
        ["plugin",action,name] if matches!(*action,"enable"|"disable"|"delete"|"uninstall")=>{request["action"]=json!(if *action=="uninstall"{"delete"}else{action});request["name"]=json!(name);},
        ["plugin","marketplace","list"]=>request["action"]=json!("marketplaces"),
        ["plugin","marketplace","add",name,source]=>{request["action"]=json!("add_marketplace");request["name"]=json!(name);request["source"]=json!(source);},
        ["plugin","marketplace","remove",name]=>{request["action"]=json!("remove_marketplace");request["name"]=json!(name);},
        ["plugin","marketplace",action,name] if matches!(*action,"browse"|"refresh")=>{request["action"]=json!("catalog");request["name"]=json!(name);request["refresh"]=json!(*action=="refresh");},
        ["plugin","install",target]=>{let(name,market)=target.split_once('@').ok_or("Use plugin@marketplace")?;request["action"]=json!("install");request["name"]=json!(name);request["marketplace"]=json!(market);},
        ["plugin",action,name] if matches!(*action,"show"|"export"|"edit"|"update")=>{
            let plugin=find(client,&request,name).await?;
            if *action=="show"{return Ok(plugin);}
            if *action=="export"{return Ok(plugin["spec"].clone());}
            if *action=="update"{request["action"]=json!("update");request["name"]=json!(name);request["marketplace"]=plugin["source"].clone();}
            else {
                if !plugin["source"].is_null(){return Err("Export to a personal copy before editing a marketplace plugin".into());}
                let path=directory.join(format!("plugin-edit-{}.json",uuid::Uuid::new_v4()));
                std::fs::write(&path,serde_json::to_vec_pretty(&plugin["spec"]).map_err(|e|e.to_string())?).map_err(|e|e.to_string())?;
                let editor=std::env::var("EDITOR").unwrap_or_else(|_|"vi".into());let mut parts=editor.split_whitespace();let executable=parts.next().ok_or("EDITOR is empty")?;
                let status=std::process::Command::new(executable).args(parts).arg(&path).status().map_err(|e|e.to_string())?;
                if !status.success(){return Err(format!("Editor failed; draft kept at {}",path.display()));}
                request["spec"]=read(path.to_str().ok_or("Invalid draft path")?)?;request["action"]=json!("save");request["expectedRevision"]=plugin["revision"].clone();
                let saved=client.call("plugin_action",request).await;
                if saved.is_ok(){let _=std::fs::remove_file(&path);}return saved;
            }
        },
        ["skill","list"]=>return client.call("list_prompt_skills",json!({"projectRoot":project})).await,
        ["skill","verify",file]=>{
            let path=directory.join("builtins/create-skill/verify.py");std::fs::create_dir_all(path.parent().ok_or("Missing verifier directory")?).map_err(|e|e.to_string())?;
            std::fs::write(&path,include_str!("../../../core/builtins/create-skill/scripts/verify.py")).map_err(|e|e.to_string())?;
            let output=std::process::Command::new("python3").arg(path).arg(file).output().map_err(|e|e.to_string())?;
            if !output.status.success(){return Err(String::from_utf8_lossy(&output.stderr).into());}return Ok(json!({"valid":true}));
        },
        [component,"list",plugin]|[component,"show",plugin] if matches!(*component,"skill"|"mcp"|"hook")=>{let p=find(client,&request,plugin).await?;return Ok(p["spec"][field(component)].clone());},
        [component,action,plugin,name,file] if matches!(*component,"mcp"|"hook")&&matches!(*action,"add"|"update")=>{
            let p=find(client,&request,plugin).await?;let mut spec=p["spec"].clone();let mut value=read(file)?;
            if *component=="mcp" {if *action=="add"&&spec["mcp"].get(name).is_some(){return Err("Connection already exists".into());}spec["mcp"][name]=value;}
            else {value["name"]=json!(name);let hooks=spec["hooks"].as_array_mut().ok_or("Invalid hook list")?;if *action=="add"&&hooks.iter().any(|h|h["name"]==*name){return Err("Hook already exists".into());}hooks.retain(|h|h["name"]!=*name);hooks.push(value);}
            request["action"]=json!("save");request["spec"]=spec;request["expectedRevision"]=p["revision"].clone();
        },
        ["skill","save",plugin,file]=>{let p=find(client,&request,plugin).await?;request["action"]=json!("save_skill");request["plugin"]=json!(plugin);request["skill"]=read(file)?;request["expectedRevision"]=p["revision"].clone();},
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
        _=>return Err("Use plugin list|create NAME|show NAME|edit NAME|save FILE|import SOURCE [NAME]|export NAME|install NAME@MARKET|update NAME|enable NAME|disable NAME|uninstall NAME; plugin marketplace list|add NAME SOURCE|browse NAME|refresh NAME|remove NAME; skill list|save PLUGIN FILE|verify FILE|delete PLUGIN ID; mcp/hook list PLUGIN|add PLUGIN NAME FILE|update PLUGIN NAME FILE|test PLUGIN NAME|delete PLUGIN NAME. Options: --scope local|global --project PATH --expected REVISION --ref REF --subdirectory PATH. For agent creation: skill create THREAD REQUEST.".into()),
    }
    client.call("plugin_action", request).await
}
fn field(component: &str) -> &str {
    match component {
        "skill" => "skills",
        "mcp" => "mcp",
        _ => "hooks",
    }
}
async fn find(client: &Client, args: &Value, name: &str) -> Result<Value, String> {
    let list = client
        .call(
            "plugin_action",
            json!({"action":"list","projectRoot":args["projectRoot"]}),
        )
        .await?;
    list.as_array()
        .ok_or("Invalid plugin list")?
        .iter()
        .find(|p| p["scope"] == args["scope"] && p["spec"]["name"] == name)
        .cloned()
        .ok_or_else(|| format!("Unknown plugin '{name}' in selected scope"))
}
