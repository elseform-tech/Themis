//! Shared, revisioned plugin storage. App and CLI mutations use the same server.
mod agent_switch;
pub mod agents;
pub mod connections;
pub mod definitions;
mod discovery;
pub mod hooks;
pub mod keys;
mod management;
mod marketplace;
mod materialize;
pub mod oauth;
pub mod package;
mod scans;
mod skill_prompts;
mod validation;
use materialize::safe_create_parent;
pub use scans::{catalog_brands, catalog_groups, compatibility_summary, MarketplaceScan};
use skill_prompts::edited_skill_source;
use validation::validate;
pub use validation::{safe_name, safe_relative};

use crate::skills::Skill;
use anyhow::{bail, Context};
pub use connections::McpServer;
pub use hooks::Hook;
pub use marketplace::{
    mcp_server_from_standard, mcp_server_to_standard, mcp_spec, ImportComponent, ImportPreview,
    ImportReport, Marketplace,
};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(deny_unknown_fields)]
pub struct PluginSpec {
    #[serde(default)]
    pub package_kind: Option<String>,
    #[serde(default)]
    pub disabled_skills: Vec<String>,
    #[serde(default)]
    pub manual_skills: Vec<String>,
    #[serde(default)]
    pub origin: Option<PluginOrigin>,
    #[serde(default)]
    pub skill_paths: BTreeMap<String, String>,
    #[serde(default)]
    pub icon: Option<String>,
    /// The plugin's own icon in the app's icon store, saved when it was checked.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub icon_asset: Option<String>,
    /// A bundled brand logo, matched when it was checked. Never taken from the plugin itself.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub brand: Option<String>,
    pub name: String,
    #[serde(default)]
    pub description: String,
    #[serde(default)]
    pub version: String,
    /// What this plugin's package was when installed from a marketplace, to spot upstream changes.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub package_hash: Option<String>,
    #[serde(default)]
    pub skills: Vec<Skill>,
    #[serde(default)]
    pub mcp: BTreeMap<String, McpServer>,
    #[serde(default)]
    pub hooks: Vec<Hook>,
    /// Profiles `spawn_subagents` can name, from the plugin's `agents/*.md`.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub subagents: Vec<agents::SubagentProfile>,
    /// `defaultEnabled: false`: it arrives switched off, until the person turns it on.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub starts_disabled: bool,
    #[serde(default)]
    pub files: BTreeMap<String, String>,
    /// The whole folder, kept on disk, when it doesn't fit in `files`; those are then only its
    /// components (manifests, skills, commands and subagents).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub package: Option<package::PackageRef>,
    #[serde(default)]
    pub executable_files: Vec<String>,
    #[serde(default)]
    pub unsupported: Vec<String>,
    /// Harmless import notes (dropped binary or oversized resources, unused frontmatter) that
    /// are listed but do not make a package partial.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub notices: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub import_issues: Vec<ImportComponent>,
}

impl PluginSpec {
    /// What consent to start a server covers: its files, and the kept package by its hash.
    pub fn approved_files(&self) -> BTreeMap<String, String> {
        let mut files = self.files.clone();
        if let Some(kept) = &self.package {
            files.insert("\0package".into(), kept.hash.clone());
        }
        files
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PluginOrigin {
    pub kind: String,
    pub location: String,
    #[serde(default)]
    pub reference: Option<String>,
    #[serde(default)]
    pub subdirectory: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Plugin {
    pub scope: String,
    pub revision: String,
    pub enabled: bool,
    pub source: Option<String>,
    pub spec: PluginSpec,
}

impl Plugin {
    /// Stable capability identity; saved legacy revision IDs still resolve their snapshots.
    pub fn capability_id(&self, skill: &str) -> String {
        format!(
            "{}--{}--{}",
            if self.scope == "local" { "l" } else { "g" },
            self.spec.name,
            skill
        )
    }
    pub fn skill_id(&self, skill: &str) -> String {
        format!(
            "{}--{}--{}--{}",
            if self.scope == "local" { "l" } else { "g" },
            self.spec.name,
            self.revision,
            skill
        )
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AvailableSkill {
    pub id: String,
    pub name: String,
    pub description: String,
    pub plugin: String,
    pub scope: String,
    pub retired: bool,
}

#[derive(Default, Serialize, Deserialize)]
struct Registry {
    current: BTreeMap<String, Plugin>,
    revisions: BTreeMap<String, Plugin>,
    #[serde(default)]
    discovery_disabled: std::collections::BTreeSet<String>,
}

#[derive(Clone)]
pub struct PluginStore {
    skill_paths: Vec<String>,
    pub global: PathBuf,
    pub project: Option<PathBuf>,
    /// Where personal skills are discovered (`~/.agents/skills` and the rest).
    home: Option<PathBuf>,
    /// An agent's store: it never leaves a hook or a command-based MCP server switched on
    /// (see `agent_switch`).
    agent_saves: bool,
}

impl PluginStore {
    pub fn new(global: PathBuf, project: Option<PathBuf>) -> Self {
        Self {
            global,
            project,
            skill_paths: crate::configuration::SkillConfig::default().paths,
            home: crate::sandbox::home_dir(),
            agent_saves: false,
        }
    }

    /// With `agent` set, the store acts for an agent: saves, imports and installs keep new or
    /// changed hooks and command-based MCP servers switched off, and switching one on is refused.
    #[must_use]
    pub fn for_agent(mut self, agent: bool) -> Self {
        self.agent_saves = agent;
        self
    }

    /// The home folder personal skills are discovered in; `None` discovers none.
    #[must_use]
    pub fn with_home(mut self, home: Option<PathBuf>) -> Self {
        self.home = home;
        self
    }
    /// Override project-local skill directories; global discovery retains conventional roots.
    pub fn with_skill_paths(mut self, paths: Vec<String>) -> anyhow::Result<Self> {
        crate::configuration::validate_relative_paths(&paths)?;
        self.skill_paths = paths;
        Ok(self)
    }
    fn directory(&self, scope: &str) -> anyhow::Result<PathBuf> {
        match scope {
            "global" => Ok(self.global.join("plugins")),
            "local" => Ok(self
                .project
                .as_ref()
                .context("Select a project for local plugins")?
                .join(".themis/plugins")),
            _ => bail!("scope must be local or global"),
        }
    }
    fn registry(&self, scope: &str) -> anyhow::Result<Registry> {
        let mut registry: Registry = read_json(&self.directory(scope)?.join("registry.json"))?;
        // Normalize historical snapshots too, without changing revision identities
        // or writing during reads. The next ordinary mutation persists cleanup.
        for plugin in registry
            .current
            .values_mut()
            .chain(registry.revisions.values_mut())
        {
            let imported = plugin
                .source
                .as_ref()
                .is_some_and(|source| source != "discovered")
                || plugin.spec.origin.as_ref().is_some_and(|origin| {
                    matches!(origin.kind.as_str(), "local" | "repository" | "marketplace")
                });
            let spec = &mut plugin.spec;
            if !imported
                || spec.skill_paths.contains_key("connect")
                || spec
                    .files
                    .keys()
                    .any(|path| path == "SKILL.md" || path.ends_with("/SKILL.md"))
                || ![
                    ".mcp.json",
                    ".claude-plugin/plugin.json",
                    "hooks/hooks.json",
                ]
                .iter()
                .any(|path| spec.files.contains_key(*path))
            {
                continue;
            }
            let before = spec.skills.len();
            spec.skills.retain(|skill| {
                !(skill.id == "connect"
                    && skill.name.starts_with("Use ")
                    && skill.description == "Use this plugin's connected tools and hooks"
                    && skill.instructions
                        == "Use the plugin tools to complete the task. Respect Themis approvals."
                    && skill.allowed_tools.is_empty()
                    && skill.scripts.is_empty())
            });
            if spec.skills.len() != before {
                spec.disabled_skills.retain(|id| id != "connect");
                spec.manual_skills.retain(|id| id != "connect");
            }
        }
        Ok(registry)
    }
    fn mutate<T>(
        &self,
        scope: &str,
        f: impl FnOnce(&mut Registry) -> anyhow::Result<T>,
    ) -> anyhow::Result<T> {
        let directory = self.directory(scope)?;
        if scope == "local" {
            safe_create_parent(
                self.project.as_ref().context("Missing project")?,
                &directory,
            )?;
        } else {
            fs::create_dir_all(&directory)?;
        }
        for file in ["registry.json", "registry.lock"] {
            if directory
                .join(file)
                .symlink_metadata()
                .is_ok_and(|m| m.file_type().is_symlink())
            {
                bail!("Symlink plugin store target rejected");
            }
        }
        let lock = fs::OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .open(directory.join("registry.lock"))?;
        lock.try_lock().context("Plugin store is busy; retry")?;
        let outcome = (|| {
            let mut registry = self.registry(scope)?;
            let result = f(&mut registry)?;
            write_json(&directory.join("registry.json"), &registry)?;
            Ok(result)
        })();
        // Explicit unlock prevents a concurrent fork from retaining the lock until exec.
        lock.unlock()?;
        outcome
    }
    pub fn list(&self) -> anyhow::Result<Vec<Plugin>> {
        let mut result: Vec<Plugin> = self.registry("global")?.current.into_values().collect();
        if self.project.is_some() {
            result.extend(self.registry("local")?.current.into_values());
        }
        result.extend(
            self.discovered_plugins()?
                .into_iter()
                .filter(|plugin| {
                    !result.iter().any(|stored| {
                        stored.scope == plugin.scope && stored.spec.name == plugin.spec.name
                    })
                })
                .collect::<Vec<_>>(),
        );
        for plugin in &mut result {
            if plugin.spec.package_kind.is_none() {
                plugin.spec.package_kind = Some(
                    if plugin.source.is_some() {
                        "plugin"
                    } else if plugin.spec.files.contains_key("SKILL.md")
                        && !plugin.spec.files.contains_key(".claude-plugin/plugin.json")
                    {
                        "skill"
                    } else if plugin.spec.skills.is_empty()
                        && plugin.spec.hooks.is_empty()
                        && plugin.spec.subagents.is_empty()
                        && !plugin.spec.mcp.is_empty()
                    {
                        "mcp"
                    } else if plugin.spec.skills.is_empty()
                        && plugin.spec.mcp.is_empty()
                        && plugin.spec.subagents.is_empty()
                        && !plugin.spec.hooks.is_empty()
                    {
                        "hook"
                    } else {
                        "plugin"
                    }
                    .into(),
                );
            }
        }
        Ok(result)
    }
    pub fn save(
        &self,
        scope: &str,
        mut spec: PluginSpec,
        expected: Option<&str>,
    ) -> anyhow::Result<Plugin> {
        validate(&spec)?;
        self.mutate(scope, |registry| {
            let previous = registry.current.get(&spec.name);
            if previous.is_some_and(|p| p.source.is_some()) {
                bail!(
                    "Marketplace plugins are read-only; create a local copy with a different name"
                );
            }
            if previous.map(|p| p.revision.as_str()) != expected {
                bail!("Plugin changed since it was opened; reload before saving");
            }
            // Set by a marketplace check, never by what is being saved.
            spec.brand = previous.and_then(|p| p.spec.brand.clone());
            spec.icon_asset = previous.and_then(|p| p.spec.icon_asset.clone());
            self.keep_new_parts_off(previous, &mut spec);
            if let Some(previous) = previous {
                for skill in &spec.skills {
                    let Some(old) = previous.spec.skills.iter().find(|old| old.id == skill.id)
                    else {
                        continue;
                    };
                    if old.name == skill.name
                        && old.description == skill.description
                        && old.instructions == skill.instructions
                    {
                        continue;
                    }
                    let Some(path) = spec
                        .skill_paths
                        .get(&skill.id)
                        .or_else(|| previous.spec.skill_paths.get(&skill.id))
                        .cloned()
                        .or_else(|| {
                            (previous.spec.skills.len() == 1
                                && previous.spec.files.contains_key("SKILL.md"))
                            .then(|| "SKILL.md".to_owned())
                        })
                    else {
                        continue;
                    };
                    let Some(original) = previous.spec.files.get(&path) else {
                        continue;
                    };
                    // Explicit source edits take precedence over editing parsed fields.
                    if spec.files.get(&path) == Some(original) {
                        spec.files
                            .insert(path.clone(), edited_skill_source(original, old, skill));
                    }
                }
                validate(&spec)?;
            }
            let plugin = Plugin {
                scope: scope.into(),
                revision: revision(),
                enabled: previous.map_or(!spec.starts_disabled, |p| p.enabled),
                source: None,
                spec,
            };
            registry.revisions.insert(
                format!("{}--{}", plugin.spec.name, plugin.revision),
                plugin.clone(),
            );
            registry
                .current
                .insert(plugin.spec.name.clone(), plugin.clone());
            Ok(plugin)
        })
    }
    pub fn set_enabled(&self, scope: &str, name: &str, enabled: bool) -> anyhow::Result<()> {
        let discovered = self
            .discovered_plugins()?
            .into_iter()
            .find(|p| p.scope == scope && p.spec.name == name);
        if enabled
            && discovered
                .as_ref()
                .is_some_and(|plugin| plugin.spec.skills.is_empty())
        {
            bail!("Cannot enable an invalid discovered skill; fix the source file first");
        }
        self.mutate(scope, |registry| {
            if let Some(plugin) = registry.current.get_mut(name) {
                self.refuse_enabling_plugin(plugin, enabled)?;
                plugin.enabled = enabled;
            } else if discovered.is_some() {
                if enabled {
                    registry.discovery_disabled.remove(name);
                } else {
                    registry.discovery_disabled.insert(name.into());
                }
            } else {
                bail!("Unknown plugin");
            }
            Ok(())
        })
    }
    pub fn set_component_enabled(
        &self,
        scope: &str,
        name: &str,
        kind: &str,
        id: &str,
        enabled: bool,
    ) -> anyhow::Result<()> {
        if let Some(plugin) = self
            .discovered_plugins()?
            .into_iter()
            .find(|p| p.scope == scope && p.spec.name == name)
        {
            if kind != "skill" || !plugin.spec.skills.iter().any(|skill| skill.id == id) {
                bail!("Unknown discovered component");
            }
            return self.set_enabled(scope, name, enabled);
        }
        self.mutate(scope, |registry| {
            let plugin = registry.current.get_mut(name).context("Unknown plugin")?;
            self.refuse_switching_on(plugin, kind, id, enabled)?;
            switch_component(plugin, kind, id, enabled)
        })
    }
    pub fn remove_component(
        &self,
        scope: &str,
        name: &str,
        kind: &str,
        id: &str,
    ) -> anyhow::Result<()> {
        if self
            .discovered_plugins()?
            .iter()
            .any(|p| p.scope == scope && p.spec.name == name)
        {
            bail!("Discovered skills are read-only; disable them or remove the source file");
        }
        self.mutate(scope, |registry| {
            let plugin = registry.current.get_mut(name).context("Unknown plugin")?;
            let removed = match kind {
                "skill" => {
                    let old_len = plugin.spec.skills.len();
                    plugin.spec.skills.retain(|skill| skill.id != id);
                    plugin.spec.disabled_skills.retain(|skill| skill != id);
                    plugin.spec.manual_skills.retain(|skill| skill != id);
                    plugin.spec.skill_paths.remove(id);
                    old_len != plugin.spec.skills.len()
                }
                "mcp" | "connection" => plugin.spec.mcp.remove(id).is_some(),
                "hook" => {
                    let old_len = plugin.spec.hooks.len();
                    plugin.spec.hooks.retain(|hook| hook.name != id);
                    old_len != plugin.spec.hooks.len()
                }
                _ => bail!("Component kind must be skill, mcp, connection, or hook"),
            };
            if !removed {
                bail!("Unknown component");
            }
            plugin.revision = revision();
            registry.revisions.insert(
                format!("{}--{}", plugin.spec.name, plugin.revision),
                plugin.clone(),
            );
            Ok(())
        })
    }
    pub fn delete(&self, scope: &str, name: &str) -> anyhow::Result<()> {
        self.mutate(scope, |registry| {
            registry.current.remove(name).context("Unknown plugin")?;
            let _ = fs::remove_dir_all(self.plugin_data_dir(scope, name));
            registry.revisions.retain(|_, p| p.spec.name != name);
            let referenced: std::collections::BTreeSet<&str> = registry
                .current
                .values()
                .chain(registry.revisions.values())
                .filter_map(|plugin| plugin.spec.package.as_ref())
                .map(|package| package.hash.as_str())
                .collect();
            package::prune(
                &self.directory(scope)?.join(package::PACKAGES_DIR),
                &referenced,
            );
            Ok(())
        })
    }
    fn builtin_skills(&self) -> Vec<Skill> {
        let mut skills = Vec::new();
        for (id, name, instructions) in [
            (
                "plugins",
                "Plugins",
                include_str!("../builtins/plugins/SKILL.md"),
            ),
            (
                "skills",
                "Skills",
                include_str!("../builtins/skills/SKILL.md"),
            ),
            ("mcp", "MCP", include_str!("../builtins/mcp/SKILL.md")),
            ("hooks", "Hooks", include_str!("../builtins/hooks/SKILL.md")),
            (
                "automations",
                "Automations",
                include_str!("../builtins/automations/SKILL.md"),
            ),
            (
                "browser-use",
                "Browser use",
                include_str!("../builtins/browser-use/SKILL.md"),
            ),
            ("init", "Init", include_str!("../builtins/init/SKILL.md")),
            (
                "subagents",
                "Subagents",
                include_str!("../builtins/subagents/SKILL.md"),
            ),
        ] {
            skills.push(Skill {
                id: id.into(),
                name: name.into(),
                description: instructions
                    .lines()
                    .find_map(|line| line.strip_prefix("description: "))
                    .unwrap_or(name)
                    .into(),
                instructions: self.guide_paths(id, instructions),
                allowed_tools: vec![],
                scripts: if id == "skills" {
                    vec![crate::skills::SkillScript {
                        name: "verify.py".into(),
                        content: include_str!("../builtins/skills/scripts/verify.py").into(),
                    }]
                } else {
                    vec![]
                },
            });
        }
        skills
    }

    /// The integration guides (`plugins`, `skills`, `mcp`, `hooks`) with this store's folders
    /// filled in, so the agent reads where things live instead of searching the disk.
    fn guide_paths(&self, id: &str, instructions: &str) -> String {
        if !matches!(id, "plugins" | "skills" | "mcp" | "hooks") {
            return instructions.to_owned();
        }
        // Each `/` part joined on its own, so the path shows in the platform's own separators.
        let native = |base: &Path, relative: &str| {
            relative
                .split('/')
                .filter(|part| !part.is_empty())
                .fold(base.to_path_buf(), |path, part| path.join(part))
        };
        let shown = |path: &Path| format!("`{}`", path.display());
        let none = "(none: this run has no project)";
        let project_plugins = self.project.as_ref().map_or_else(
            || none.to_owned(),
            |root| shown(&native(root, ".themis/plugins")),
        );
        let project_root = self
            .project
            .as_ref()
            .map_or_else(|| "PROJECT".to_owned(), |root| root.display().to_string());
        let folders = |base: Option<&PathBuf>, paths: &[String]| match base {
            Some(base) => paths
                .iter()
                .map(|path| shown(&native(base, path)))
                .collect::<Vec<_>>()
                .join(", "),
            None => none.to_owned(),
        };
        let defaults = crate::configuration::SkillConfig::default().paths;
        let mut values = vec![
            ("global_plugins", shown(&self.global.join("plugins"))),
            ("project_plugins", project_plugins),
            ("project_root", project_root),
        ];
        if id == "skills" {
            values.push((
                "personal_skill_folders",
                folders(self.home.as_ref(), &defaults),
            ));
            values.push((
                "project_skill_folders",
                folders(self.project.as_ref(), &self.skill_paths),
            ));
        }
        let values: Vec<(&str, &str)> = values
            .iter()
            .map(|(name, value)| (*name, value.as_str()))
            .collect();
        crate::prompts::fill(instructions, &values)
    }
}

pub(super) fn revision() -> String {
    format!(
        "{:x}",
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos()
    )
}
pub(super) fn read_json<T: serde::de::DeserializeOwned + Default>(
    path: &Path,
) -> anyhow::Result<T> {
    match fs::read(path) {
        Ok(bytes) => {
            if bytes.len() > 32 * 1024 * 1024 {
                bail!("Plugin registry exceeds 32 MiB");
            }
            Ok(serde_json::from_slice(&bytes)?)
        }
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(T::default()),
        Err(e) => Err(e.into()),
    }
}
pub(super) fn write_json(path: &Path, value: &impl Serialize) -> anyhow::Result<()> {
    let bytes = serde_json::to_vec_pretty(value)?;
    if bytes.len() > 32 * 1024 * 1024 {
        bail!("Plugin registry exceeds 32 MiB; previous data was preserved");
    }
    fs::create_dir_all(path.parent().context("Missing store directory")?)?;
    write_bytes(path, &bytes, false)
}
fn write_bytes(path: &Path, bytes: &[u8], executable: bool) -> anyhow::Result<()> {
    #[cfg(not(unix))]
    let _ = executable;
    let temp = path.with_extension(format!("{}.tmp", revision()));
    let result = (|| {
        use std::io::Write;
        let mut options = fs::OpenOptions::new();
        options.write(true).create_new(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(if executable { 0o700 } else { 0o600 });
        }
        let mut file = options.open(&temp)?;
        file.write_all(bytes)?;
        #[cfg(unix)]
        if executable {
            use std::os::unix::fs::PermissionsExt;
            file.set_permissions(fs::Permissions::from_mode(0o700))?;
        }
        file.sync_all()?;
        fs::rename(&temp, path)?;
        Ok(())
    })();
    if result.is_err() {
        let _ = fs::remove_file(temp);
    }
    result
}

/// Turns one skill, MCP server or hook of `plugin` on or off. Flags change the current
/// activation policy, not pinned instruction snapshots.
fn switch_component(
    plugin: &mut Plugin,
    kind: &str,
    id: &str,
    enabled: bool,
) -> anyhow::Result<()> {
    match kind {
        "skill" => {
            if !plugin.spec.skills.iter().any(|s| s.id == id) {
                bail!("Unknown skill");
            }
            plugin.spec.disabled_skills.retain(|s| s != id);
            if !enabled {
                plugin.spec.disabled_skills.push(id.into());
            }
        }
        "mcp" | "connection" => {
            plugin
                .spec
                .mcp
                .get_mut(id)
                .context("Unknown connection")?
                .enabled = enabled
        }
        "hook" => {
            plugin
                .spec
                .hooks
                .iter_mut()
                .find(|h| h.name == id)
                .context("Unknown hook")?
                .enabled = enabled
        }
        _ => bail!("Component kind must be skill, mcp, connection, or hook"),
    }
    Ok(())
}
