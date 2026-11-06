use super::*;

/// What `Skill` hooks are told a skill is called, from its id (`g--plugin--revision--name`,
/// `g--plugin--name`) or its folder: `plugin:name`, or just the name of one of its own.
#[must_use]
pub fn skill_label(id: &str) -> String {
    let parts: Vec<&str> = id.split("--").collect();
    match parts.as_slice() {
        [_, plugin, name] | [_, plugin, _, name] => format!("{plugin}:{name}"),
        _ => id.to_owned(),
    }
}

/// Which skill a read takes up, if it does: a `SKILL.md` Themis put under `.themis/` for the run
/// (the catalog the model picks from, or a plugin's files). A project's own file of that name
/// isn't one, and nor is `.themis` further down a path that doesn't start there.
pub(super) fn skill_being_read(tool: &str, arguments: &Value) -> Option<String> {
    if tool != "read_file" {
        return None;
    }
    let path = arguments["file_path"].as_str()?;
    let path = path.replace('\\', "/");
    let at = if path.starts_with(".themis/") {
        0
    } else {
        path.find("/.themis/")? + 1
    };
    let inside = &path[at..];
    let name = inside.strip_suffix("/SKILL.md")?.rsplit('/').next()?;
    if inside.starts_with(".themis/skill-catalogs/") || inside.starts_with(".themis/skills/") {
        return Some(skill_label(name));
    }
    // `.themis/plugin-files/<scope>--<plugin>--<revision>--resources/skills/<name>/SKILL.md`
    let resources = inside.strip_prefix(".themis/plugin-files/")?;
    let (package, _) = resources.split_once('/')?;
    let plugin = package.split("--").nth(1)?;
    Some(format!("{plugin}:{name}"))
}

/// What a person wrote beside a skill they attached: its mark reads as its name by now.
pub(super) fn command_args(task: &str, skill: &str) -> String {
    let name = skill.rsplit(':').next().unwrap_or(skill);
    task.replacen(name, "", 1).trim().to_owned()
}

/// Runs `Skill` hooks when a tool call takes up a skill: once a run for each skill, before the read
/// so a hook can refuse it, and after it (`after` is whether the read went well).
pub async fn skill_read_hooks(
    tool: &str,
    arguments: &Value,
    after: Option<bool>,
    events: &tokio::sync::mpsc::Sender<crate::runtime::RunEvent>,
) -> anyhow::Result<()> {
    let Some(skill) = skill_being_read(tool, arguments) else {
        return Ok(());
    };
    let Ok(runtime) = ACTIVE.try_with(Clone::clone) else {
        return Ok(());
    };
    match after {
        None => {
            if runtime.first_sight_of(&skill) {
                let call = json!({"tool": "Skill", "arguments": {"skill": skill}});
                if let Err(error) = runtime.emit("BeforeTool", call, events).await {
                    // It can be taken up again once a hook lets it.
                    runtime.forget_skill(&skill);
                    return Err(error);
                }
            }
        }
        Some(ok) => {
            if runtime.report_after(&skill) {
                let call = json!({"tool": "Skill", "arguments": {"skill": skill}, "ok": ok,
                    "output": json!({"success": ok, "commandName": skill}).to_string()});
                runtime.emit("AfterTool", call, events).await?;
            }
        }
    }
    Ok(())
}
