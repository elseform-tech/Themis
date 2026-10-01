//! Shared, revisioned plugin storage. App and CLI mutations use the same server.
pub mod connections;
pub mod hooks;
mod marketplace;

use crate::skills::Skill;
use anyhow::{bail, Context};
pub use connections::McpServer;
pub use hooks::Hook;
pub use marketplace::Marketplace;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct PluginSpec {
    pub name: String,
    #[serde(default)]
    pub description: String,
    #[serde(default)]
    pub version: String,
    #[serde(default)]
    pub skills: Vec<Skill>,
    #[serde(default)]
    pub mcp: BTreeMap<String, McpServer>,
    #[serde(default)]
    pub hooks: Vec<Hook>,
    #[serde(default)]
    pub files: BTreeMap<String, String>,
    #[serde(default)]
    pub executable_files: Vec<String>,
    #[serde(default)]
    pub unsupported: Vec<String>,
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
}

#[derive(Clone)]
pub struct PluginStore {
    pub global: PathBuf,
    pub project: Option<PathBuf>,
}

impl PluginStore {
    pub fn new(global: PathBuf, project: Option<PathBuf>) -> Self {
        Self { global, project }
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
        read_json(&self.directory(scope)?.join("registry.json"))
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
        Ok(result)
    }
    pub fn save(
        &self,
        scope: &str,
        spec: PluginSpec,
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
            let plugin = Plugin {
                scope: scope.into(),
                revision: revision(),
                enabled: previous.map(|p| p.enabled).unwrap_or(true),
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
        self.mutate(scope, |registry| {
            registry
                .current
                .get_mut(name)
                .context("Unknown plugin")?
                .enabled = enabled;
            Ok(())
        })
    }
    pub fn delete(&self, scope: &str, name: &str) -> anyhow::Result<()> {
        self.mutate(scope, |registry| {
            registry.current.remove(name).context("Unknown plugin")?;
            registry.revisions.retain(|_, p| p.spec.name != name);
            Ok(())
        })
    }
    pub fn available_skills(&self) -> anyhow::Result<Vec<AvailableSkill>> {
        let mut result = vec![AvailableSkill {
            id: "create-skill".into(),
            name: "Create skill".into(),
            description: "Ask the agent to design and save a local or global skill".into(),
            plugin: "Themis".into(),
            scope: "built-in".into(),
            retired: false,
        }];
        for plugin in self.list()?.into_iter().filter(|p| p.enabled) {
            for skill in &plugin.spec.skills {
                result.push(AvailableSkill {
                    id: plugin.skill_id(&skill.id),
                    name: skill.name.clone(),
                    description: skill.description.clone(),
                    plugin: plugin.spec.name.clone(),
                    scope: plugin.scope.clone(),
                    retired: false,
                });
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
        let mut text = String::new();
        let mut skills = Vec::new();
        let mut plugins = Vec::new();
        let mut rest = prompt;
        while let Some(start) = rest.find("[[skill:") {
            text.push_str(&rest[..start]);
            let tail = &rest[start + 8..];
            let end = tail.find("]]").context("Unfinished skill reference")?;
            let id = &tail[..end];
            let (skill, plugin) = if id == "create-skill" {
                (self.creator(), None)
            } else if let Some(skill) = legacy.iter().find(|s| s.id == id) {
                (skill.clone(), None)
            } else {
                let parts: Vec<_> = id.split("--").collect();
                if parts.len() != 4 {
                    bail!("Unavailable skill '{id}'");
                }
                let scope = match parts[0] {
                    "g" => "global",
                    "l" => "local",
                    _ => bail!("Invalid skill scope"),
                };
                let registry = self.registry(scope)?;
                if !registry.current.get(parts[1]).is_some_and(|p| p.enabled) {
                    bail!("Plugin '{}' is missing or disabled", parts[1]);
                }
                let plugin = registry
                    .revisions
                    .get(&format!("{}--{}", parts[1], parts[2]))
                    .context("Plugin revision is unavailable; replace the skill reference")?
                    .clone();
                let mut skill = plugin
                    .spec
                    .skills
                    .iter()
                    .find(|s| s.id == parts[3])
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
    fn creator(&self) -> Skill {
        Skill {
            id: "create-skill".into(),
            name: "Create skill".into(),
            description: "Design reusable skills".into(),
            instructions: include_str!("../builtins/create-skill/SKILL.md").into(),
            allowed_tools: vec![],
            scripts: vec![crate::skills::SkillScript {
                name: "verify.py".into(),
                content: include_str!("../builtins/create-skill/scripts/verify.py").into(),
            }],
        }
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
                ..Default::default()
            });
        spec.skills.retain(|s| s.id != skill.id);
        spec.skills.push(skill);
        self.save(scope, spec, expected)
    }
    pub fn materialize(&self, plugins: &[Plugin], root: &Path) -> anyhow::Result<()> {
        for plugin in plugins {
            for (relative, content) in &plugin.spec.files {
                let directory = root
                    .join(".themis/plugin-files")
                    .join(plugin.skill_id("resources"));
                let path = directory.join(relative);
                let parent = path.parent().context("Missing parent")?;
                safe_create_parent(root, parent)?;
                if path
                    .symlink_metadata()
                    .is_ok_and(|m| m.file_type().is_symlink())
                {
                    bail!("Symlink resource target rejected");
                }
                fs::write(&path, content)?;
                #[cfg(unix)]
                if plugin.spec.executable_files.contains(relative) {
                    use std::os::unix::fs::PermissionsExt;
                    fs::set_permissions(&path, fs::Permissions::from_mode(0o700))?;
                }
            }
        }
        Ok(())
    }
}

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
fn validate(spec: &PluginSpec) -> anyhow::Result<()> {
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
    if spec.files.keys().any(|p| !safe_relative(p))
        || spec.files.values().map(String::len).sum::<usize>() > 8 * 1024 * 1024
    {
        bail!("Unsafe resource path or plugin resources exceed 8 MiB");
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
    Ok(())
}
fn safe_create_parent(root: &Path, parent: &Path) -> anyhow::Result<()> {
    let canonical = root.canonicalize()?;
    let relative = parent.strip_prefix(root)?;
    let mut next = root.to_path_buf();
    for part in relative.components() {
        next.push(part);
        if next.exists() {
            if !next.canonicalize()?.starts_with(&canonical) {
                bail!("Resource path escapes project");
            }
        } else {
            fs::create_dir(&next)?;
        }
    }
    Ok(())
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
    let temp = path.with_extension(format!("{}.tmp", revision()));
    let result = (|| {
        use std::io::Write;
        let mut options = fs::OpenOptions::new();
        options.write(true).create_new(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(0o600);
        }
        let mut file = options.open(&temp)?;
        file.write_all(&bytes)?;
        file.sync_all()?;
        fs::rename(&temp, path)?;
        Ok(())
    })();
    if result.is_err() {
        let _ = fs::remove_file(temp);
    }
    result
}

struct ManagementTool {
    name: &'static str,
    store: PluginStore,
    approvals: std::sync::Arc<dyn crate::tools::ApprovalHook>,
}
impl std::fmt::Debug for ManagementTool {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_tuple("PluginTool").field(&self.name).finish()
    }
}
#[autoagents::async_trait]
impl crate::tools::ToolRuntime for ManagementTool {
    async fn execute(
        &self,
        args: serde_json::Value,
    ) -> Result<serde_json::Value, autoagents::core::tool::ToolCallError> {
        use crate::tools::{RiskLevel, ToolAction};
        crate::tools::check_approval_permitted(
            self.approvals.as_ref(),
            &ToolAction {
                tool: self.name.into(),
                summary: format!(
                    "{} {}",
                    self.name,
                    args.get("scope")
                        .and_then(serde_json::Value::as_str)
                        .unwrap_or("")
                ),
                risk: if self.name == "list_plugins" {
                    RiskLevel::Read
                } else {
                    RiskLevel::Write
                },
            },
        )?;
        let result = (|| -> anyhow::Result<serde_json::Value> {
            if self.name == "list_plugins" {
                return Ok(serde_json::to_value(self.store.list()?)?);
            }
            let skill: Skill = serde_json::from_value(args["skill"].clone())?;
            Ok(serde_json::to_value(self.store.save_skill(
                args["scope"].as_str().unwrap_or("local"),
                args["plugin"].as_str().unwrap_or("personal"),
                skill,
                args["expectedRevision"].as_str(),
            )?)?)
        })();
        result
            .map_err(|e| autoagents::core::tool::ToolCallError::RuntimeError(e.to_string().into()))
    }
}
impl crate::tools::ToolT for ManagementTool {
    fn name(&self) -> &str {
        self.name
    }
    fn description(&self) -> &str {
        if self.name == "list_plugins" {
            "Read available plugins, skills, scopes, and current revisions"
        } else {
            "Validate and save a generated skill in a personal local or global plugin"
        }
    }
    fn args_schema(&self) -> serde_json::Value {
        if self.name == "list_plugins" {
            serde_json::json!({"type":"object","properties":{}})
        } else {
            serde_json::json!({"type":"object","properties":{"scope":{"type":"string","enum":["local","global"]},"plugin":{"type":"string"},"expectedRevision":{"type":["string","null"]},"skill":{"type":"object","properties":{"id":{"type":"string"},"name":{"type":"string"},"description":{"type":"string"},"instructions":{"type":"string"},"allowedTools":{"type":"array","items":{"type":"string"}},"scripts":{"type":"array","items":{"type":"object","properties":{"name":{"type":"string"},"content":{"type":"string"}},"required":["name","content"]}}},"required":["id","name","description","instructions","allowedTools","scripts"]}},"required":["scope","plugin","skill"]})
        }
    }
    fn output_schema(&self) -> Option<serde_json::Value> {
        None
    }
}
impl PluginStore {
    pub fn management_tools(
        &self,
        approvals: std::sync::Arc<dyn crate::tools::ApprovalHook>,
    ) -> Vec<Box<dyn crate::tools::ToolT>> {
        ["list_plugins", "save_skill"]
            .into_iter()
            .map(|name| {
                Box::new(ManagementTool {
                    name,
                    store: self.clone(),
                    approvals: approvals.clone(),
                }) as Box<dyn crate::tools::ToolT>
            })
            .collect()
    }
}
