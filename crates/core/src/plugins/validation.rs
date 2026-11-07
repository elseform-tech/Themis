//! Checks a plugin definition must pass before it is saved: safe names, paths and components.

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
    if spec
        .package_kind
        .as_deref()
        .is_some_and(|kind| !matches!(kind, "plugin" | "skill" | "mcp" | "hook"))
    {
        bail!("Package kind must be plugin, skill, mcp, or hook");
    }
    if !safe_name(&spec.name) {
        bail!("Plugin name must be a safe single path segment without '--'");
    }
    let mut ids = std::collections::HashSet::new();
    for skill in &spec.skills {
        if !safe_name(&skill.id) || !ids.insert(&skill.id) {
            bail!("Invalid or duplicate skill id");
        }
        crate::skills::validate_skill_input(
            &skill.name,
            &skill.instructions,
            &skill.allowed_tools,
            &skill.scripts,
        )?;
    }
    if spec
        .disabled_skills
        .iter()
        .chain(spec.manual_skills.iter())
        .any(|id| !ids.contains(id))
    {
        bail!("Disabled skill must refer to an existing skill");
    }
    if let Some(icon) = &spec.icon {
        let remote = reqwest::Url::parse(icon).is_ok_and(|url| {
            url.scheme() == "https"
                && url.username().is_empty()
                && url.password().is_none()
                && url.query().is_none()
                && url.fragment().is_none()
        });
        let bundled_svg = icon.ends_with(".svg") && spec.files.contains_key(icon);
        let bundled_image = icon.ends_with(".png") || icon.ends_with(".webp");
        if !remote && !(safe_relative(icon) && (bundled_svg || bundled_image)) {
            bail!("Plugin icon must be an HTTPS URL or a bundled SVG, PNG or WebP");
        }
    }
    if spec.files.keys().any(|p| !safe_relative(p))
        || spec.files.values().map(String::len).sum::<usize>() > package::MAX_INLINE_BYTES
    {
        bail!("Unsafe resource path or plugin resources exceed 8 MiB");
    }
    if let Some(kept) = &spec.package {
        kept.validate()?;
    }
    if spec
        .executable_files
        .iter()
        .any(|p| !spec.files.contains_key(p))
    {
        bail!("Executable resource must refer to a plugin file");
    }
    for (name, server) in &spec.mcp {
        if !safe_name(name) {
            bail!("Invalid MCP name");
        }
        server.validate()?;
    }
    let mut hook_names = std::collections::HashSet::new();
    for hook in &spec.hooks {
        if !hook_names.insert(&hook.name) {
            bail!("Duplicate hook name");
        }
        hook.validate()?;
    }
    let mut profile_names = std::collections::HashSet::new();
    for profile in &spec.subagents {
        if !profile_names.insert(&profile.name) {
            bail!("Duplicate subagent name");
        }
        profile.validate()?;
    }
    Ok(())
}
