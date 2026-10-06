//! Read-only discovery of conventional personal and project skills.
use super::*;
use std::collections::BTreeSet;

impl PluginStore {
    pub fn discovered_plugins(&self) -> anyhow::Result<Vec<Plugin>> {
        let mut plugins = Vec::new();
        if let Some(home) = std::env::var_os("HOME") {
            discover_scope(self, "global", Path::new(&home), &mut plugins)?;
        }
        if let Some(project) = &self.project {
            discover_scope(self, "local", project, &mut plugins)?;
        }
        Ok(plugins)
    }
}
fn discover_scope(
    store: &PluginStore,
    scope: &str,
    base: &Path,
    plugins: &mut Vec<Plugin>,
) -> anyhow::Result<()> {
    let registry = store.registry(scope)?;
    let mut seen = BTreeSet::new();
    for source in [".agents/skills", ".codex/skills", ".claude/skills"] {
        let root = base.join(source);
        if root.is_dir() {
            scan_skills(&root, 0, &mut seen, &mut |directory| {
                let skill_file = directory.join("SKILL.md").canonicalize()?;
                let location = skill_file.to_string_lossy().to_string();
                // Stable FNV-1a identity uses the source path, never the mutable instructions.
                let hash = location.bytes().fold(0xcbf29ce484222325u64, |hash, byte| {
                    (hash ^ u64::from(byte)).wrapping_mul(0x100000001b3)
                });
                let plugin_name = format!("discovered-{hash:016x}");
                let fallback = directory
                    .file_name()
                    .and_then(|s| s.to_str())
                    .filter(|s| safe_name(s))
                    .unwrap_or("skill");
                let mut spec = PluginSpec {
                    name: plugin_name.clone(),
                    origin: Some(PluginOrigin {
                        kind: "discovered".into(),
                        location: directory.to_string_lossy().into(),
                        reference: None,
                        subdirectory: None,
                    }),
                    ..Default::default()
                };
                let parsed = (|| -> anyhow::Result<Skill> {
                    super::marketplace::collect_files(
                        directory,
                        directory,
                        &mut spec.files,
                        &mut spec.executable_files,
                        &mut spec.unsupported,
                    )?;
                    spec.executable_files.clear(); // Discovery never grants execute permissions to resources.
                    let content = fs::read_to_string(&skill_file)?;
                    if content.len() > crate::skills::MAX_INSTRUCTIONS_BYTES {
                        bail!("Discovered skill exceeds instruction limit");
                    }
                    let (metadata, instructions) = super::marketplace::parse_skill(&content)?;
                    let name = metadata.get("name").map(String::as_str).unwrap_or(fallback);
                    let id = if safe_name(name) { name } else { fallback };
                    for key in metadata.keys().filter(|key| {
                        ![
                            "name",
                            "description",
                            "license",
                            "compatibility",
                            "disable-model-invocation",
                        ]
                        .contains(&key.as_str())
                    }) {
                        spec.unsupported.push(format!(
                            "Skill frontmatter {key}: host behavior requires manual mapping"
                        ));
                    }
                    if metadata
                        .get("disable-model-invocation")
                        .is_some_and(|v| v == "true")
                    {
                        spec.manual_skills.push(id.into());
                    }
                    let skill = Skill {
                        id: id.into(),
                        name: name.into(),
                        description: metadata.get("description").cloned().unwrap_or_default(),
                        instructions,
                        allowed_tools: vec![],
                        scripts: vec![],
                    };
                    crate::skills::validate_skill_input(
                        &skill.name,
                        &skill.instructions,
                        &skill.allowed_tools,
                        &skill.scripts,
                    )?;
                    Ok(skill)
                })();
                let valid = match parsed {
                    Ok(skill) => {
                        spec.skill_paths.insert(skill.id.clone(), location.clone());
                        spec.skills.push(skill);
                        true
                    }
                    Err(error) => {
                        spec.unsupported.push(format!("Cannot load skill: {error}"));
                        false
                    }
                };
                plugins.push(Plugin {
                    scope: scope.into(),
                    revision: "discovered".into(),
                    enabled: valid && !registry.discovery_disabled.contains(&plugin_name),
                    source: Some("discovered".into()),
                    spec,
                });
                if plugins.len() > 1000 {
                    bail!("Skill discovery exceeds 1000 skills");
                }
                Ok(())
            })?;
        }
    }
    Ok(())
}
fn scan_skills(
    directory: &Path,
    depth: usize,
    seen: &mut BTreeSet<PathBuf>,
    visit: &mut impl FnMut(&Path) -> anyhow::Result<()>,
) -> anyhow::Result<()> {
    let canonical = directory.canonicalize()?;
    if !seen.insert(canonical.clone()) {
        return Ok(());
    }
    if canonical.join("SKILL.md").is_file() {
        return visit(&canonical);
    }
    // ponytail: conventional roots scan eight levels; import deeper skill folders explicitly.
    if depth >= 8 {
        return Ok(());
    }
    let mut children = fs::read_dir(&canonical)?.collect::<Result<Vec<_>, _>>()?;
    children.sort_by_key(|entry| entry.file_name());
    for child in children {
        let path = child.path();
        if path.is_dir() && child.file_name() != ".git" && child.file_name() != "node_modules" {
            scan_skills(&path, depth + 1, seen, visit)?;
        }
    }
    Ok(())
}
