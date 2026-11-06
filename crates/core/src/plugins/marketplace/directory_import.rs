use super::*;

/// Imports a skill or plugin folder. `entry` is the marketplace entry, whose `skills`,
/// `mcpServers` and `hooks` declarations count like the plugin manifest's.
pub(super) fn import_directory(
    root: &Path,
    allow_partial: bool,
    entry: Option<&Value>,
    keep_in: Option<&Path>,
) -> anyhow::Result<PluginSpec> {
    import_directory_within(root, allow_partial, entry, keep_in, super::package::LIMITS)
}

/// [`import_directory`] with the bounds given. `keep_in` is where a package too big for the
/// registry is kept; without it, as in a check, it is only hashed.
pub(super) fn import_directory_within(
    root: &Path,
    allow_partial: bool,
    entry: Option<&Value>,
    keep_in: Option<&Path>,
    limits: super::package::Limits,
) -> anyhow::Result<PluginSpec> {
    let manifest_path = root.join(".claude-plugin/plugin.json");
    let manifest: Value = if manifest_path.exists() {
        serde_json::from_str(&fs::read_to_string(&manifest_path)?)?
    } else {
        serde_json::json!({})
    };
    let mut spec = PluginSpec {
        package_kind: Some(
            if !manifest_path.exists() && root.join("SKILL.md").is_file() {
                "skill"
            } else {
                "plugin"
            }
            .into(),
        ),
        name: manifest["name"]
            .as_str()
            .unwrap_or_else(|| {
                root.file_name()
                    .and_then(|s| s.to_str())
                    .filter(|s| safe_name(s))
                    .unwrap_or("imported")
            })
            .into(),
        description: manifest["description"].as_str().unwrap_or_default().into(),
        version: manifest["version"].as_str().unwrap_or_default().into(),
        ..Default::default()
    };
    let components = declared_component_files(&manifest, entry);
    let mut entries = match collect_plugin_files(root, &mut spec, None, &components, limits) {
        Ok(entries) => entries,
        Err(error) => {
            if error.to_string() != RESOURCE_LIMITS {
                return Err(error);
            }
            // Past even a package's bounds. Keep only the plugin's own folders and the files
            // its declarations name.
            crate::diagnostics::emit(
                "plugins",
                "package_over_limits",
                "warn",
                serde_json::json!({"files": limits.files, "bytes": limits.total_bytes}),
            );
            let keep = component_paths(&manifest, entry);
            spec.files.clear();
            spec.executable_files.clear();
            spec.unsupported.clear();
            spec.notices.clear();
            let entries = collect_plugin_files(root, &mut spec, Some(keep), &components, limits)?;
            spec.unsupported
                .push("Large repository: only the plugin's own folders were imported".into());
            entries
        }
    };
    if let Some(icon) = manifest.get("icon").and_then(Value::as_str) {
        let icon = icon.strip_prefix("./").unwrap_or(icon);
        if usable_icon(icon, &spec.files, root) {
            spec.icon = Some(icon.into());
        } else {
            spec.unsupported
                .push("Plugin icon resource is unavailable or unsupported".into());
        }
    }
    let declared = |field: &str| {
        [Some(&manifest), entry]
            .into_iter()
            .flatten()
            .filter_map(|source| source.get(field))
            .collect::<Vec<_>>()
    };
    let skill_roots = declared_paths(
        &declared("skills"),
        "skills",
        &spec.files,
        &mut spec.unsupported,
    );
    for (path, id, name, metadata, instructions) in select_skills(root, &spec.files, &skill_roots)?
    {
        spec.skill_paths.insert(id.clone(), path.clone());
        spec.skills.push(Skill {
            id: id.clone(),
            name,
            description: metadata.get("description").cloned().unwrap_or_default(),
            instructions,
            allowed_tools: vec![],
            scripts: vec![],
        });
        for key in metadata.keys().filter(|key| {
            ![
                "name",
                "description",
                "license",
                "compatibility",
                "allowed-tools",
                "disable-model-invocation",
            ]
            .contains(&key.as_str())
        }) {
            spec.notices
                .push(format!("{path}: skill frontmatter {key}"));
        }
        if metadata
            .get("disable-model-invocation")
            .is_some_and(|value| value == "true")
        {
            spec.manual_skills.push(id);
        }
        if metadata.contains_key("allowed-tools") {
            spec.notices.push(format!(
                "{path}: Claude tool names are not permission grants in Themis"
            ));
        }
    }
    let command_roots = declared_paths(
        &declared("commands"),
        "commands",
        &spec.files,
        &mut spec.unsupported,
    );
    import_commands(&mut spec, &command_roots)?;
    let agent_roots = declared_paths(
        &declared("agents"),
        "subagents",
        &spec.files,
        &mut spec.unsupported,
    );
    super::agents::import_profiles(&mut spec, &agent_roots)?;
    let mut mcp_files = vec![".mcp.json".to_owned()];
    let mut hook_files = vec!["hooks/hooks.json".to_owned()];
    let mut literals = LiteralSecrets::for_plugin(&spec.name);
    let in_catalog = entry.and_then(|entry| entry.get("mcpServers"));
    for value in declared("mcpServers") {
        if value.is_object() {
            // What a marketplace entry says is in its catalog, which Themis doesn't rewrite.
            if in_catalog.is_some_and(|declared| std::ptr::eq(declared, value)) {
                let mut refused = LiteralSecrets::refused();
                import_mcp_components(value, false, &mut spec, allow_partial, &mut refused)?;
            } else {
                import_mcp_components(value, false, &mut spec, allow_partial, &mut literals)?;
            }
        } else {
            mcp_files.extend(declared_paths(
                &[value],
                "MCP configuration",
                &spec.files,
                &mut spec.unsupported,
            ));
        }
    }
    for value in declared("hooks") {
        if value.is_object() {
            import_hooks(value, root, &mut spec)?;
        } else {
            hook_files.extend(declared_paths(
                &[value],
                "hook configuration",
                &spec.files,
                &mut spec.unsupported,
            ));
        }
    }
    for path in unique(mcp_files) {
        if let Some(raw) = spec.files.get(&path) {
            let config: Value = serde_json::from_str(raw)?;
            let servers = config.get("mcpServers").unwrap_or(&config).clone();
            import_mcp_components(&servers, false, &mut spec, allow_partial, &mut literals)?;
        }
    }
    for path in unique(hook_files) {
        if let Some(raw) = spec.files.get(&path) {
            import_hooks(&serde_json::from_str::<Value>(raw)?, root, &mut spec)?;
        }
    }
    spec.starts_disabled = manifest.get("defaultEnabled") == Some(&Value::Bool(false));
    for field in [
        "channels",
        "lspServers",
        "outputStyles",
        "settings",
        "dependencies",
        "experimental",
    ] {
        if manifest.get(field).is_some() {
            spec.unsupported.push(field.into());
        }
    }
    if [Some(&manifest), entry]
        .into_iter()
        .flatten()
        .any(|source| source.get("userConfig").is_some())
    {
        spec.notices
            .push("Install settings (userConfig) are entered in Settings › Secrets".into());
    }
    import_output_styles(&mut spec)?;
    scrub_literals(&literals.replaced, &mut spec, &mut entries)?;
    if let (Some(store), Some(package)) = (keep_in, &spec.package) {
        super::package::keep(&entries, package, store)?;
    }
    Ok(spec)
}

/// Claude plugin commands (`commands/**/*.md`, plus declared command files and folders)
/// become manual-only skills named after the file, folders joined with `-`. Their
/// instructions open with [`crate::prompts::SKILL_COMMAND_NOTE`], which explains
/// `$ARGUMENTS`, `!`command`` and `@path`; an id taken by a skill gets `-command`.
fn import_commands(spec: &mut PluginSpec, declared: &[String]) -> anyhow::Result<()> {
    let roots: Vec<&str> = std::iter::once("commands")
        .chain(declared.iter().map(String::as_str))
        .filter(|root| !root.is_empty())
        .collect();
    let mut commands = BTreeMap::new();
    for path in spec.files.keys().filter(|path| path.ends_with(".md")) {
        let relative = roots.iter().find_map(|root| {
            if path == root {
                Path::new(path).file_name().and_then(|name| name.to_str())
            } else {
                path.strip_prefix(&format!("{root}/"))
            }
        });
        if let Some(relative) = relative {
            commands.insert(
                path.clone(),
                relative.trim_end_matches(".md").replace('/', "-"),
            );
        }
    }
    for (path, id) in commands {
        let taken = |id: &str| spec.skills.iter().any(|skill| skill.id == id);
        let id = if taken(&id) {
            format!("{id}-command")
        } else {
            id
        };
        if !safe_name(&id) || taken(&id) {
            spec.unsupported
                .push(format!("{path}: command name can't be used as a skill"));
            continue;
        }
        let (metadata, body) = parse_skill(&spec.files[&path])?;
        let first_line = body
            .lines()
            .map(|line| line.trim().trim_start_matches('#').trim())
            .find(|line| !line.is_empty())
            .unwrap_or_default();
        let description = metadata
            .get("description")
            .cloned()
            .unwrap_or_else(|| first_line.chars().take(200).collect());
        for key in metadata.keys().filter(|key| key.as_str() != "description") {
            spec.notices.push(if key == "allowed-tools" {
                format!("{path}: Claude tool names are not permission grants in Themis")
            } else {
                format!("{path}: command frontmatter {key}")
            });
        }
        let name = path
            .rsplit('/')
            .next()
            .unwrap_or(&id)
            .trim_end_matches(".md")
            .to_owned();
        spec.skill_paths.insert(id.clone(), path.clone());
        spec.manual_skills.push(id.clone());
        spec.skills.push(Skill {
            id,
            name,
            description,
            instructions: format!("{}\n\n{body}", crate::prompts::SKILL_COMMAND_NOTE),
            allowed_tools: vec![],
            scripts: vec![],
        });
    }
    Ok(())
}
/// Claude's output styles (`output-styles/*.md`) become manual-only skills named `style-<file>`:
/// Themis has no lasting output style, so one applies to the turn it is chosen for, and the
/// compatibility details say so.
fn import_output_styles(spec: &mut PluginSpec) -> anyhow::Result<()> {
    let paths: Vec<String> = spec
        .files
        .keys()
        .filter(|path| {
            path.strip_prefix("output-styles/")
                .is_some_and(|rest| rest.ends_with(".md") && !rest.contains('/'))
        })
        .cloned()
        .collect();
    for path in paths {
        let (metadata, body) = parse_skill(&spec.files[&path])?;
        let stem = path
            .trim_start_matches("output-styles/")
            .trim_end_matches(".md");
        let id = format!("style-{stem}");
        if !safe_name(&id) || spec.skills.iter().any(|skill| skill.id == id) {
            continue;
        }
        spec.notices.push(format!(
            "{path}: an output style, used as the /{id} skill for one turn at a time"
        ));
        spec.skill_paths.insert(id.clone(), path.clone());
        spec.manual_skills.push(id.clone());
        spec.skills.push(Skill {
            name: metadata.get("name").cloned().unwrap_or_else(|| id.clone()),
            description: metadata.get("description").cloned().unwrap_or_default(),
            instructions: format!("{}\n\n{body}", crate::prompts::SKILL_OUTPUT_STYLE_NOTE),
            id,
            allowed_tools: vec![],
            scripts: vec![],
        });
    }
    Ok(())
}
/// Text paths a manifest or marketplace entry declares for `label`, normalised and checked to
/// stay inside the plugin. Problems are reported instead of failing the import.
fn declared_paths(
    values: &[&Value],
    label: &str,
    files: &BTreeMap<String, String>,
    unsupported: &mut Vec<String>,
) -> Vec<String> {
    let mut paths = Vec::new();
    for value in values {
        let Some(texts) = declared_texts(value) else {
            unsupported.push(format!("Declared {label} paths must be text"));
            continue;
        };
        for text in texts {
            let path = text.trim_start_matches("./").trim_end_matches('/');
            if !path.is_empty() && !safe_relative(path) {
                unsupported.push(format!(
                    "Declared {label} path is outside the plugin: {text}"
                ));
            } else if !path.is_empty()
                && !files.contains_key(path)
                && !files
                    .keys()
                    .any(|file| file.starts_with(&format!("{path}/")))
            {
                unsupported.push(format!("Declared {label} path not found: {text}"));
            } else {
                paths.push(path.to_owned());
            }
        }
    }
    paths
}

/// The text paths in a `skills`, `mcpServers` or `hooks` declaration; `None` when it is
/// neither text nor a list.
fn declared_texts(value: &Value) -> Option<Vec<&str>> {
    match value {
        Value::String(text) => Some(vec![text.as_str()]),
        Value::Array(items) => Some(items.iter().filter_map(Value::as_str).collect()),
        _ => None,
    }
}

/// What to keep when a plugin folder is too large: its component folders and declared paths.
fn component_paths(manifest: &Value, entry: Option<&Value>) -> Vec<String> {
    let declared = ["skills", "mcpServers", "hooks", "agents"]
        .into_iter()
        .flat_map(|field| {
            [Some(manifest), entry]
                .into_iter()
                .flatten()
                .filter_map(move |source| source.get(field))
        })
        .filter_map(declared_texts)
        .flatten()
        .map(|text| {
            text.trim_start_matches("./")
                .trim_end_matches('/')
                .to_owned()
        })
        .filter(|path| safe_relative(path));
    COMPONENT_PATHS
        .iter()
        .map(|path| (*path).to_owned())
        .chain(declared)
        .collect()
}

fn unique(paths: Vec<String>) -> Vec<String> {
    let mut seen = std::collections::BTreeSet::new();
    paths
        .into_iter()
        .filter(|path| seen.insert(path.clone()))
        .collect()
}

/// One imported skill: path, id, name, frontmatter and instructions.
type SelectedSkill = (String, String, String, BTreeMap<String, String>, String);

/// Every `SKILL.md` is a skill, as before, unless that gives two skills the same id. Then only
/// Claude's layout counts (a root `SKILL.md`, `skills/<name>/SKILL.md` and declared skill
/// folders), so copies for other tools and reference files nested inside a skill drop out.
/// A plugin with no skills in that layout keeps all of them. Any ids that still collide are
/// taken from the skill's folder path.
fn select_skills(
    root: &Path,
    files: &BTreeMap<String, String>,
    declared: &[String],
) -> anyhow::Result<Vec<SelectedSkill>> {
    let all: Vec<&String> = files
        .keys()
        .filter(|path| path.as_str() == "SKILL.md" || path.ends_with("/SKILL.md"))
        .collect();
    if let Ok(skills) = parse_skills(root, files, &all) {
        if !has_duplicate_ids(&skills) {
            return Ok(skills);
        }
    }
    let roots: Vec<&str> = std::iter::once("skills")
        .chain(declared.iter().map(String::as_str))
        .collect();
    let claude_layout: Vec<&String> = all
        .iter()
        .copied()
        .filter(|path| {
            let folder = skill_folder(path);
            let parent = folder.rsplit_once('/').map_or("", |(parent, _)| parent);
            folder.is_empty() || roots.iter().any(|root| folder == *root || parent == *root)
        })
        .collect();
    // A plugin that keeps its skills elsewhere keeps all of them.
    let selected = if claude_layout.is_empty() {
        all
    } else {
        claude_layout
    };
    let mut skills = parse_skills(root, files, &selected)?;
    let mut counts = BTreeMap::<String, usize>::new();
    for skill in &skills {
        *counts.entry(skill.1.clone()).or_default() += 1;
    }
    for skill in &mut skills {
        if counts[&skill.1] > 1 {
            let derived = skill_folder(&skill.0).replace('/', "-");
            if safe_name(&derived) {
                skill.1 = derived;
            }
        }
    }
    Ok(skills)
}

fn skill_folder(path: &str) -> &str {
    path.strip_suffix("SKILL.md")
        .unwrap_or(path)
        .trim_end_matches('/')
}

fn has_duplicate_ids(skills: &[SelectedSkill]) -> bool {
    let mut ids = std::collections::BTreeSet::new();
    skills.iter().any(|skill| !ids.insert(&skill.1))
}

fn parse_skills(
    root: &Path,
    files: &BTreeMap<String, String>,
    paths: &[&String],
) -> anyhow::Result<Vec<SelectedSkill>> {
    paths
        .iter()
        .map(|path| {
            let (metadata, instructions) = parse_skill(&files[*path])?;
            let fallback = if path.as_str() == "SKILL.md" {
                root.file_name().and_then(|s| s.to_str()).unwrap_or("skill")
            } else {
                Path::new(path.as_str())
                    .parent()
                    .and_then(Path::file_name)
                    .and_then(|s| s.to_str())
                    .unwrap_or("skill")
            };
            let name = metadata.get("name").map(String::as_str).unwrap_or(fallback);
            let id = if safe_name(name) { name } else { fallback };
            Ok((
                (*path).clone(),
                id.into(),
                name.into(),
                metadata,
                instructions,
            ))
        })
        .collect()
}

/// Strict walk used by skill discovery: any symlink or oversized file fails the package.
pub(crate) fn collect_files(
    root: &Path,
    directory: &Path,
    files: &mut BTreeMap<String, String>,
    executable_files: &mut Vec<String>,
    unsupported: &mut Vec<String>,
) -> anyhow::Result<()> {
    let mut walk = FileWalk {
        root,
        files,
        executable_files,
        unsupported,
        notices: None,
        stack: vec![],
        keep: None,
        entries: None,
        limits: super::package::LIMITS,
        total_bytes: 0,
    };
    walk.directory(directory, "")
}

/// Lenient walk used for plugin packages: links that stay inside the plugin are followed,
/// files over the package bound are left out with a notice, and other links are reported.
/// Nothing is read until the folder is known to be within bounds. Returns the files to keep on
/// disk, when they don't fit in the registry; otherwise none.
fn collect_plugin_files(
    root: &Path,
    spec: &mut PluginSpec,
    keep: Option<Vec<String>>,
    components: &[String],
    limits: super::package::Limits,
) -> anyhow::Result<Vec<super::package::Entry>> {
    let root = root.canonicalize()?;
    let mut entries = Vec::new();
    let mut walk = FileWalk {
        root: &root,
        files: &mut spec.files,
        executable_files: &mut spec.executable_files,
        unsupported: &mut spec.unsupported,
        notices: Some(&mut spec.notices),
        stack: vec![root.clone()],
        keep,
        entries: Some(&mut entries),
        limits,
        total_bytes: 0,
    };
    walk.directory(&root, "")?;
    entries.sort_by(|a, b| a.relative.cmp(&b.relative));
    if read_all_inline(&entries, spec)? {
        return Ok(Vec::new());
    }
    read_components_inline(&entries, components, spec)?;
    spec.package = Some(super::package::describe(&entries)?);
    Ok(entries)
}

/// Puts every file in the registry if all of them fit. It stops at the first that doesn't, so at
/// most the registry's bound is ever in memory.
fn read_all_inline(
    entries: &[super::package::Entry],
    spec: &mut PluginSpec,
) -> anyhow::Result<bool> {
    use super::package::{MAX_INLINE_BYTES, MAX_INLINE_FILES, MAX_INLINE_FILE_BYTES};
    let fits = entries.len() <= MAX_INLINE_FILES
        && entries
            .iter()
            .all(|entry| entry.bytes <= MAX_INLINE_FILE_BYTES)
        && entries.iter().map(|entry| entry.bytes).sum::<u64>() <= MAX_INLINE_BYTES as u64;
    if !fits {
        return Ok(false);
    }
    for entry in entries {
        let Ok(text) = String::from_utf8(fs::read(&entry.path)?) else {
            spec.files.clear();
            spec.executable_files.clear();
            return Ok(false);
        };
        spec.files.insert(entry.relative.clone(), text);
        if entry.executable {
            spec.executable_files.push(entry.relative.clone());
        }
    }
    Ok(true)
}

/// Puts only what the importer and the UI read in the registry: the rest is on disk.
fn read_components_inline(
    entries: &[super::package::Entry],
    components: &[String],
    spec: &mut PluginSpec,
) -> anyhow::Result<()> {
    use super::package::{MAX_INLINE_BYTES, MAX_INLINE_FILES, MAX_INLINE_FILE_BYTES};
    let mut bytes = 0usize;
    for entry in entries
        .iter()
        .filter(|entry| is_component_file(&entry.relative, components))
    {
        if entry.bytes > MAX_INLINE_FILE_BYTES {
            spec.notices
                .push(format!("File too large: {}", entry.relative));
            continue;
        }
        // A binary file beside them is in the package all the same.
        let Ok(text) = String::from_utf8(fs::read(&entry.path)?) else {
            continue;
        };
        bytes += text.len();
        spec.files.insert(entry.relative.clone(), text);
        if entry.executable {
            spec.executable_files.push(entry.relative.clone());
        }
        if spec.files.len() > MAX_INLINE_FILES || bytes > MAX_INLINE_BYTES {
            bail!(RESOURCE_LIMITS);
        }
    }
    Ok(())
}

/// Whether the importer reads this file: manifests, skills, commands, subagents, and files a
/// plugin declares as those.
fn is_component_file(relative: &str, declared: &[String]) -> bool {
    let markdown = relative.ends_with(".md");
    relative.starts_with(".claude-plugin/")
        || matches!(relative, ".mcp.json" | "hooks/hooks.json" | "SKILL.md")
        || relative.ends_with("/SKILL.md")
        || (markdown && relative.starts_with("commands/"))
        || (markdown && relative.starts_with("agents/"))
        || (markdown && relative.starts_with("output-styles/"))
        || declared
            .iter()
            .any(|path| relative == path || (markdown && relative.starts_with(&format!("{path}/"))))
}

/// What a plugin's manifest and marketplace entry declare besides skills, which are always named
/// `SKILL.md`: command, subagent, MCP and hook files and folders, and its icon.
fn declared_component_files(manifest: &Value, entry: Option<&Value>) -> Vec<String> {
    ["commands", "agents", "mcpServers", "hooks", "icon"]
        .into_iter()
        .flat_map(|field| {
            [Some(manifest), entry]
                .into_iter()
                .flatten()
                .filter_map(move |source| source.get(field))
        })
        .filter_map(declared_texts)
        .flatten()
        .map(|text| {
            text.trim_start_matches("./")
                .trim_end_matches('/')
                .to_owned()
        })
        .filter(|path| safe_relative(path))
        .collect()
}

pub(super) const RESOURCE_LIMITS: &str = "Plugin exceeds resource limits";
/// The folders and files a Claude plugin keeps its components in.
const COMPONENT_PATHS: &[&str] = &[
    ".claude-plugin",
    ".mcp.json",
    "SKILL.md",
    "skills",
    "commands",
    "agents",
    "hooks",
    "scripts",
    "assets",
];

struct FileWalk<'a> {
    root: &'a Path,
    files: &'a mut BTreeMap<String, String>,
    executable_files: &'a mut Vec<String>,
    unsupported: &'a mut Vec<String>,
    /// `Some` selects the lenient plugin walk.
    notices: Option<&'a mut Vec<String>>,
    /// Canonical directories being walked, to stop symlink loops.
    stack: Vec<PathBuf>,
    /// When set, only these plugin-relative paths (and the folders leading to them) are read.
    keep: Option<Vec<String>>,
    /// `Some` lists files without reading them, within `limits`. Discovery reads as it walks.
    entries: Option<&'a mut Vec<super::package::Entry>>,
    limits: super::package::Limits,
    total_bytes: u64,
}

impl FileWalk<'_> {
    fn directory(&mut self, directory: &Path, logical: &str) -> anyhow::Result<()> {
        let mut entries = fs::read_dir(directory)?.collect::<Result<Vec<_>, _>>()?;
        entries.sort_by_key(|entry| entry.file_name());
        for entry in entries {
            if entry.file_name() == ".git" {
                continue;
            }
            let name = entry.file_name().to_string_lossy().into_owned();
            let relative = if logical.is_empty() {
                name
            } else {
                format!("{logical}/{name}")
            };
            if !self.kept(&relative) {
                continue;
            }
            if relative.split('/').count() > 32 {
                bail!("Plugin directory nesting exceeds 32 levels");
            }
            let kind = entry.file_type()?;
            let path = entry.path();
            if kind.is_symlink() {
                if self.notices.is_none() {
                    bail!("Plugin symlinks are unsupported");
                }
                match self.link_target(&path) {
                    Some(target) if target.is_dir() => {
                        self.stack.push(target.clone());
                        let result = self.directory(&target, &relative);
                        self.stack.pop();
                        result?;
                    }
                    Some(target) if target.is_file() => self.file(&target, relative)?,
                    _ => self
                        .unsupported
                        .push(format!("Link not followed: {relative}")),
                }
            } else if kind.is_dir() {
                self.directory(&path, &relative)?;
            } else if kind.is_file() {
                self.file(&path, relative)?;
            }
        }
        Ok(())
    }

    fn kept(&self, relative: &str) -> bool {
        self.keep.as_ref().is_none_or(|keep| {
            keep.iter().any(|path| {
                relative == path
                    || relative.starts_with(&format!("{path}/"))
                    || path.starts_with(&format!("{relative}/"))
            })
        })
    }

    /// A link is followed only when its target resolves inside the plugin root and is not a
    /// directory already being walked.
    fn link_target(&self, link: &Path) -> Option<PathBuf> {
        let target = link.canonicalize().ok()?;
        (target.starts_with(self.root) && !self.stack.iter().any(|open| open == &target))
            .then_some(target)
    }

    fn file(&mut self, path: &Path, relative: String) -> anyhow::Result<()> {
        let metadata = fs::metadata(path)?;
        if let Some(entries) = self.entries.as_deref_mut() {
            if metadata.len() > self.limits.file_bytes {
                if let Some(notices) = self.notices.as_deref_mut() {
                    notices.push(format!("File too large: {relative}"));
                }
                return Ok(());
            }
            self.total_bytes = self.total_bytes.saturating_add(metadata.len());
            #[cfg(unix)]
            let executable = {
                use std::os::unix::fs::PermissionsExt;
                metadata.permissions().mode() & 0o111 != 0
            };
            #[cfg(not(unix))]
            let executable = false;
            entries.push(super::package::Entry {
                relative,
                path: path.to_owned(),
                bytes: metadata.len(),
                executable,
                content: None,
            });
            if entries.len() > self.limits.files || self.total_bytes > self.limits.total_bytes {
                bail!(RESOURCE_LIMITS);
            }
            return Ok(());
        }
        if metadata.len() > super::package::MAX_INLINE_FILE_BYTES {
            bail!("Plugin file exceeds 1 MiB");
        }
        let bytes = fs::read(path)?;
        let Ok(text) = String::from_utf8(bytes) else {
            self.unsupported
                .push(format!("Binary resource omitted: {relative}"));
            return Ok(());
        };
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            if metadata.permissions().mode() & 0o111 != 0 {
                self.executable_files.push(relative.clone());
            }
        }
        self.files.insert(relative, text);
        if self.files.len() > super::package::MAX_INLINE_FILES
            || self.files.values().map(String::len).sum::<usize>()
                > super::package::MAX_INLINE_BYTES
        {
            bail!(RESOURCE_LIMITS);
        }
        Ok(())
    }
}
pub(crate) fn parse_skill(content: &str) -> anyhow::Result<(BTreeMap<String, String>, String)> {
    let normalized = content.replace("\r\n", "\n");
    let Some(tail) = normalized.strip_prefix("---\n") else {
        return Ok((BTreeMap::new(), content.into()));
    };
    let end = tail
        .find("\n---\n")
        .or_else(|| tail.strip_suffix("\n---").map(str::len))
        .context("Unclosed skill frontmatter")?;
    let mut metadata = BTreeMap::new();
    let mut multiline: Option<String> = None;
    for line in tail[..end].lines() {
        if line.starts_with(' ') {
            if let Some(key) = &multiline {
                metadata.entry(key.clone()).and_modify(|v: &mut String| {
                    if !v.is_empty() {
                        v.push(' ');
                    }
                    v.push_str(line.trim());
                });
            }
            continue;
        }
        if let Some((key, value)) = line.split_once(':') {
            let value = value.trim();
            multiline = if matches!(value, "|" | ">" | "|-" | ">-") {
                Some(key.into())
            } else {
                None
            };
            metadata.insert(
                key.into(),
                if multiline.is_some() {
                    String::new()
                } else {
                    value.trim_matches(['\'', '"']).into()
                },
            );
        }
    }
    Ok((metadata, tail[end + 4..].trim().into()))
}
