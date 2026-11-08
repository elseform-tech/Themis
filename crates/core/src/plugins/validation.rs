//! Checks a plugin definition must pass before it is saved: safe names, paths and components.
//! Every problem is reported at once, each naming the part it is about.

use super::*;

pub fn safe_name(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= 100
        && !name.starts_with('.')
        && !name.contains("--")
        && name
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b"_.-".contains(&b))
}

pub fn safe_relative(path: &str) -> bool {
    !path.is_empty()
        && Path::new(path)
            .components()
            .all(|c| matches!(c, std::path::Component::Normal(_)))
        && !path.contains('\\')
}

pub(super) fn validate(spec: &PluginSpec) -> anyhow::Result<()> {
    let problems = problems(spec);
    if !problems.is_empty() {
        bail!("{}", definitions::report("The plugin", &problems));
    }
    Ok(())
}

/// Every problem with a plugin definition, one line each.
pub fn problems(spec: &PluginSpec) -> Vec<String> {
    let mut problems = Vec::new();
    if spec
        .package_kind
        .as_deref()
        .is_some_and(|kind| !matches!(kind, "plugin" | "skill" | "mcp" | "hook"))
    {
        problems.push("Package kind must be plugin, skill, mcp, or hook".to_owned());
    }
    if !safe_name(&spec.name) {
        problems.push(format!(
            "name '{}': a plugin name must be a safe single path segment without '--'",
            spec.name
        ));
    }
    skill_problems(spec, &mut problems);
    if let Some(problem) = icon_problem(spec) {
        problems.push(problem);
    }
    resource_problems(spec, &mut problems);
    component_problems(spec, &mut problems);
    problems
}

fn skill_problems(spec: &PluginSpec, problems: &mut Vec<String>) {
    let mut ids = std::collections::HashSet::new();
    for (index, skill) in spec.skills.iter().enumerate() {
        let at = format!("skills[{index}] ('{}')", skill.id);
        if !safe_name(&skill.id) {
            problems.push(format!(
                "{at}: id must be a safe single name: letters, digits, '_', '.', '-'"
            ));
        } else if !ids.insert(&skill.id) {
            problems.push(format!("{at}: duplicate skill id"));
        }
        problems.extend(
            crate::skills::skill_input_problems(
                &skill.name,
                &skill.instructions,
                &skill.allowed_tools,
                &skill.scripts,
            )
            .into_iter()
            .map(|problem| format!("{at}: {problem}")),
        );
    }
    for id in spec.disabled_skills.iter().chain(spec.manual_skills.iter()) {
        if !ids.contains(id) {
            problems.push(format!(
                "Disabled skill must refer to an existing skill ('{id}')"
            ));
        }
    }
}

fn icon_problem(spec: &PluginSpec) -> Option<String> {
    let icon = spec.icon.as_ref()?;
    let remote = reqwest::Url::parse(icon).is_ok_and(|url| {
        url.scheme() == "https"
            && url.username().is_empty()
            && url.password().is_none()
            && url.query().is_none()
            && url.fragment().is_none()
    });
    let bundled_svg = icon.ends_with(".svg") && spec.files.contains_key(icon);
    let bundled_image = icon.ends_with(".png") || icon.ends_with(".webp");
    (!remote && !(safe_relative(icon) && (bundled_svg || bundled_image)))
        .then(|| "Plugin icon must be an HTTPS URL or a bundled SVG, PNG or WebP".to_owned())
}

fn resource_problems(spec: &PluginSpec, problems: &mut Vec<String>) {
    if spec.files.keys().any(|p| !safe_relative(p))
        || spec.files.values().map(String::len).sum::<usize>() > package::MAX_INLINE_BYTES
    {
        problems.push("Unsafe resource path or plugin resources exceed 8 MiB".to_owned());
    }
    if let Some(Err(error)) = spec.package.as_ref().map(package::PackageRef::validate) {
        problems.push(format!("{error:#}"));
    }
    if spec
        .executable_files
        .iter()
        .any(|p| !spec.files.contains_key(p))
    {
        problems.push("Executable resource must refer to a plugin file".to_owned());
    }
}

fn component_problems(spec: &PluginSpec, problems: &mut Vec<String>) {
    for (name, server) in &spec.mcp {
        if !safe_name(name) {
            problems.push(format!("mcp.{name}: Invalid MCP name"));
        }
        problems.extend(
            server
                .problems()
                .into_iter()
                .map(|problem| format!("mcp.{name}: {problem}")),
        );
    }
    let mut hook_names = std::collections::HashSet::new();
    for (index, hook) in spec.hooks.iter().enumerate() {
        let at = format!("hooks[{index}] ('{}')", hook.name);
        if !hook_names.insert(&hook.name) {
            problems.push(format!("{at}: Duplicate hook name"));
        }
        problems.extend(
            hook.problems()
                .into_iter()
                .map(|problem| format!("{at}: {problem}")),
        );
    }
    let mut profile_names = std::collections::HashSet::new();
    for profile in &spec.subagents {
        if !profile_names.insert(&profile.name) {
            problems.push(format!(
                "subagents ('{}'): Duplicate subagent name",
                profile.name
            ));
        }
        if let Err(error) = profile.validate() {
            problems.push(format!("subagents ('{}'): {error:#}", profile.name));
        }
    }
}
