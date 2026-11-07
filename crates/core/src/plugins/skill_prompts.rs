//! Skills offered to the agent and one-component edits: listing, prompt resolution, and saving one skill or hook.

use super::*;

impl PluginStore {
    pub fn available_skills(&self) -> anyhow::Result<Vec<AvailableSkill>> {
        let mut result = self
            .builtin_skills()
            .into_iter()
            .map(|skill| AvailableSkill {
                id: skill.id,
                name: skill.name,
                description: skill.description,
                plugin: "Themis".into(),
                scope: "built-in".into(),
                retired: false,
            })
            .collect::<Vec<_>>();
        for plugin in self.list()?.into_iter().filter(|p| p.enabled) {
            for skill in plugin
                .spec
                .skills
                .iter()
                .filter(|s| !plugin.spec.disabled_skills.contains(&s.id))
            {
                result.push(AvailableSkill {
                    id: plugin.capability_id(&skill.id),
                    name: skill.name.clone(),
                    description: skill.description.clone(),
                    plugin: plugin.spec.name.clone(),
                    scope: plugin.scope.clone(),
                    retired: false,
                });
                if plugin.source.as_deref() != Some("discovered") {
                    result.push(AvailableSkill {
                        id: plugin.skill_id(&skill.id),
                        name: skill.name.clone(),
                        description: skill.description.clone(),
                        plugin: plugin.spec.name.clone(),
                        scope: plugin.scope.clone(),
                        retired: true,
                    });
                }
            }
        }
        for scope in ["global", "local"] {
            if scope == "local" && self.project.is_none() {
                continue;
            }
            let registry = self.registry(scope)?;
            for plugin in registry.revisions.values().filter(|p| {
                registry
                    .current
                    .get(&p.spec.name)
                    .is_some_and(|current| current.enabled && current.revision != p.revision)
            }) {
                for skill in &plugin.spec.skills {
                    result.push(AvailableSkill {
                        id: plugin.skill_id(&skill.id),
                        name: skill.name.clone(),
                        description: skill.description.clone(),
                        plugin: plugin.spec.name.clone(),
                        scope: plugin.scope.clone(),
                        retired: true,
                    });
                }
            }
        }
        Ok(result)
    }

    pub fn resolve_prompt(
        &self,
        prompt: &str,
        legacy: &[Skill],
    ) -> anyhow::Result<(String, Vec<Skill>, Vec<Plugin>)> {
        let mut available = None;
        let builtins = self.builtin_skills();
        let mut registries = std::collections::HashMap::new();
        let mut text = String::new();
        let mut skills = Vec::new();
        let mut plugins = Vec::new();
        let mut rest = prompt;
        while let Some((start, plugin_reference)) = [
            rest.find("[[skill:").map(|start| (start, false)),
            rest.find("[[plugin:").map(|start| (start, true)),
        ]
        .into_iter()
        .flatten()
        .min_by_key(|(start, _)| *start)
        {
            text.push_str(&rest[..start]);
            let tail = &rest[start + if plugin_reference { 9 } else { 8 }..];
            let end = tail.find("]]").context("Unfinished capability reference")?;
            let id = &tail[..end];
            if plugin_reference {
                let (scope, name) = id.split_once("--").context("Invalid plugin reference")?;
                let scope = match scope {
                    "g" => "global",
                    "l" => "local",
                    _ => bail!("Invalid plugin scope"),
                };
                if available.is_none() {
                    available = Some(self.list()?);
                }
                let plugin = available
                    .as_ref()
                    .unwrap()
                    .iter()
                    .find(|plugin| {
                        plugin.scope == scope && plugin.spec.name == name && plugin.enabled
                    })
                    .context("Plugin is missing or disabled")?
                    .clone();
                text.push('@');
                text.push_str(if plugin.source.as_deref() == Some("discovered") {
                    plugin
                        .spec
                        .skills
                        .first()
                        .map(|skill| skill.name.as_str())
                        .unwrap_or(name)
                } else {
                    name
                });
                if !plugins
                    .iter()
                    .any(|known: &Plugin| known.scope == scope && known.spec.name == name)
                {
                    plugins.push(plugin);
                }
                rest = &tail[end + 2..];
                continue;
            }
            let (skill, plugin) = if let Some(skill) = builtins.iter().find(|s| s.id == id).cloned()
            {
                (skill, None)
            } else if let Some(skill) = legacy.iter().find(|s| s.id == id) {
                (skill.clone(), None)
            } else {
                let parts: Vec<_> = id.split("--").collect();
                if !matches!(parts.len(), 3 | 4) {
                    bail!("Unavailable skill '{id}'");
                }
                let scope = match parts[0] {
                    "g" => "global",
                    "l" => "local",
                    _ => bail!("Invalid skill scope"),
                };

                if available.is_none() {
                    available = Some(self.list()?);
                }
                let current = available
                    .as_ref()
                    .unwrap()
                    .iter()
                    .find(|p| p.scope == scope && p.spec.name == parts[1] && p.enabled)
                    .context("Plugin is missing or disabled")?;
                let skill_name = parts[parts.len() - 1];
                if !current
                    .spec
                    .skills
                    .iter()
                    .any(|skill| skill.id == skill_name)
                {
                    bail!("Skill '{skill_name}' has been removed");
                }
                if current
                    .spec
                    .disabled_skills
                    .iter()
                    .any(|id| id == skill_name)
                {
                    bail!("Skill '{skill_name}' is disabled");
                }
                let mut plugin = if parts.len() == 3 {
                    current.clone()
                } else {
                    if !registries.contains_key(scope) {
                        registries.insert(scope, self.registry(scope)?);
                    }
                    registries[scope]
                        .revisions
                        .get(&format!("{}--{}", parts[1], parts[2]))
                        .context("Plugin revision is unavailable; replace the skill reference")?
                        .clone()
                };
                // Current activation controls also apply to saved revision references.
                for (name, server) in &mut plugin.spec.mcp {
                    server.enabled &= current.spec.mcp.get(name).is_some_and(|s| s.enabled);
                }
                for hook in &mut plugin.spec.hooks {
                    hook.enabled &= current
                        .spec
                        .hooks
                        .iter()
                        .find(|h| h.name == hook.name)
                        .is_some_and(|h| h.enabled);
                }
                let mut skill = plugin
                    .spec
                    .skills
                    .iter()
                    .find(|s| s.id == skill_name)
                    .context("Skill is unavailable")?
                    .clone();
                skill.id = id.into();
                if !plugin.spec.files.is_empty() {
                    skill.instructions.push_str(&format!(
                        "\nSupporting files: .themis/plugin-files/{}/",
                        plugin.skill_id("resources")
                    ));
                }
                (skill, Some(plugin))
            };
            text.push_str(&skill.name);
            if !skills.iter().any(|s: &Skill| s.id == skill.id) {
                skills.push(skill);
            }
            if let Some(plugin) = plugin {
                if !plugins.iter().any(|p: &Plugin| {
                    p.scope == plugin.scope
                        && p.spec.name == plugin.spec.name
                        && p.revision == plugin.revision
                }) {
                    plugins.push(plugin);
                }
            }
            rest = &tail[end + 2..];
        }
        text.push_str(rest);
        Ok((text, skills, plugins))
    }

    pub fn save_skill(
        &self,
        scope: &str,
        plugin_name: &str,
        skill: Skill,
        expected: Option<&str>,
    ) -> anyhow::Result<Plugin> {
        let mut spec = self
            .registry(scope)?
            .current
            .get(plugin_name)
            .map(|p| p.spec.clone())
            .unwrap_or(PluginSpec {
                name: plugin_name.into(),
                package_kind: Some("skill".into()),
                ..Default::default()
            });
        spec.skills.retain(|s| s.id != skill.id);
        spec.skills.push(skill);
        self.save(scope, spec, expected)
    }

    /// Adds or replaces one hook, keeping the rest of the plugin. A new or changed hook is saved
    /// turned off; an unchanged one stays as it was.
    pub fn save_hook(
        &self,
        scope: &str,
        plugin_name: &str,
        mut hook: Hook,
        expected: Option<&str>,
    ) -> anyhow::Result<Plugin> {
        let mut spec = self
            .registry(scope)?
            .current
            .get(plugin_name)
            .map(|p| p.spec.clone())
            .unwrap_or(PluginSpec {
                name: plugin_name.into(),
                ..Default::default()
            });
        hook.plugin_root = None;
        hook.runtime_identity = None;
        let definition = |hook: &Hook| {
            let mut value = serde_json::to_value(hook).unwrap_or_default();
            value["enabled"] = serde_json::Value::Null;
            value
        };
        let previous = spec.hooks.iter().position(|old| old.name == hook.name);
        hook.enabled = previous
            .map(|index| &spec.hooks[index])
            .is_some_and(|old| old.enabled && definition(old) == definition(&hook));
        match previous {
            Some(index) => spec.hooks[index] = hook,
            None => spec.hooks.push(hook),
        }
        self.save(scope, spec, expected)
    }
}

// Retain unrelated frontmatter verbatim; literal blocks safely quote edited metadata.
pub(super) fn edited_skill_source(original: &str, old: &Skill, skill: &Skill) -> String {
    let lines: Vec<_> = original.split_inclusive('\n').collect();
    let has_header = lines
        .first()
        .is_some_and(|line| line.trim_end_matches(['\r', '\n']) == "---");
    let closing = has_header
        .then(|| {
            lines
                .iter()
                .skip(1)
                .position(|line| line.trim_end_matches(['\r', '\n']) == "---")
        })
        .flatten()
        .map(|index| index + 1);
    let body = closing
        .map(|end| lines[end + 1..].concat())
        .unwrap_or_else(|| original.into());
    let metadata_changed = old.name != skill.name || old.description != skill.description;
    let header = if !metadata_changed {
        closing
            .map(|end| lines[..=end].concat())
            .unwrap_or_default()
    } else {
        let newline = if original.contains("\r\n") {
            "\r\n"
        } else {
            "\n"
        };
        let mut header = format!("---{newline}");
        let mut skip = false;
        if let Some(end) = closing {
            for line in &lines[1..end] {
                if !line.starts_with([' ', '\t']) && !line.trim().is_empty() {
                    skip = line.split_once(':').is_some_and(|(key, _)| {
                        (key == "name" && old.name != skill.name)
                            || (key == "description" && old.description != skill.description)
                    });
                }
                if !skip {
                    header.push_str(line);
                }
            }
        }
        for (key, value, changed) in [
            ("name", &skill.name, old.name != skill.name),
            (
                "description",
                &skill.description,
                old.description != skill.description,
            ),
        ] {
            if changed || closing.is_none() {
                header.push_str(&format!("{key}: |-{newline}"));
                for line in value.lines() {
                    header.push_str(&format!("  {line}{newline}"));
                }
            }
        }
        header.push_str(&format!("---{newline}"));
        header
    };
    if old.instructions == skill.instructions {
        format!("{header}{body}")
    } else {
        format!("{header}{}\n", skill.instructions)
    }
}
