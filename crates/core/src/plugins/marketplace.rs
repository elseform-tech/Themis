use super::*;
use serde_json::Value;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Marketplace {
    pub name: String,
    pub source: String,
}
#[derive(Default, Serialize, Deserialize)]
struct Catalogs {
    sources: Vec<Marketplace>,
}

impl PluginStore {
    pub fn marketplaces(&self) -> anyhow::Result<Vec<Marketplace>> {
        let lock = self.marketplace_lock()?;
        let result = self.read_marketplaces();
        lock.unlock()?;
        result
    }
    fn marketplace_lock(&self) -> anyhow::Result<fs::File> {
        fs::create_dir_all(&self.global)?;
        let file = fs::OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .open(self.global.join("marketplaces.lock"))?;
        file.try_lock()
            .context("Marketplace store is busy; retry")?;
        Ok(file)
    }
    fn read_marketplaces(&self) -> anyhow::Result<Vec<Marketplace>> {
        let path = self.global.join("marketplaces.json");
        if !path.exists() {
            write_json(
                &path,
                &Catalogs {
                    sources: vec![Marketplace {
                        name: "claude-plugins-official".into(),
                        source: "https://github.com/anthropics/claude-plugins-official.git".into(),
                    }],
                },
            )?;
        }
        Ok(read_json::<Catalogs>(&path)?.sources)
    }
    pub fn add_marketplace(&self, name: &str, source: &str) -> anyhow::Result<()> {
        if !safe_name(name) {
            bail!("Invalid marketplace name");
        }
        validate_source(source)?;
        let lock = self.marketplace_lock()?;
        let mut sources = self.read_marketplaces()?;
        if sources.iter().any(|s| s.name == name) {
            bail!("Marketplace already exists");
        }
        sources.push(Marketplace {
            name: name.into(),
            source: source.into(),
        });
        let result = write_json(
            &self.global.join("marketplaces.json"),
            &Catalogs { sources },
        );
        lock.unlock()?;
        result
    }
    pub fn remove_marketplace(&self, name: &str) -> anyhow::Result<()> {
        let lock = self.marketplace_lock()?;
        let mut sources = self.read_marketplaces()?;
        if !sources.iter().any(|s| s.name == name) {
            bail!("Unknown marketplace");
        }
        sources.retain(|s| s.name != name);
        let result = write_json(
            &self.global.join("marketplaces.json"),
            &Catalogs { sources },
        );
        lock.unlock()?;
        result
    }
    pub async fn catalog(&self, name: &str, refresh: bool) -> anyhow::Result<Value> {
        let directory = self.marketplace_directory(name, refresh).await?;
        let path = directory.join(".claude-plugin/marketplace.json");
        let value: Value = serde_json::from_str(&fs::read_to_string(path)?)?;
        if !value["plugins"].is_array() {
            bail!("Marketplace has no plugin list");
        }
        Ok(value)
    }
    async fn marketplace_directory(&self, name: &str, refresh: bool) -> anyhow::Result<PathBuf> {
        let source = self
            .marketplaces()?
            .into_iter()
            .find(|s| s.name == name)
            .context("Unknown marketplace")?
            .source;
        if !source.starts_with("https://") {
            return Ok(PathBuf::from(source).canonicalize()?);
        }
        let cache = self.global.join("marketplace-cache").join(name);
        if refresh || !cache.exists() {
            let staging = cache.with_extension(revision());
            clone_repo(&source, &staging).await?;
            let backup = cache.with_extension(format!("{}.old", revision()));
            if cache.exists() {
                fs::rename(&cache, &backup)?;
            }
            if let Err(error) = fs::rename(&staging, &cache) {
                if backup.exists() {
                    let _ = fs::rename(&backup, &cache);
                }
                return Err(error.into());
            }
            if backup.exists() {
                fs::remove_dir_all(backup)?;
            }
        }
        Ok(cache)
    }
    pub async fn install(
        &self,
        scope: &str,
        marketplace: &str,
        name: &str,
    ) -> anyhow::Result<Plugin> {
        if !safe_name(name) {
            bail!("Invalid plugin name");
        }
        let refresh = self
            .registry(scope)?
            .current
            .get(name)
            .is_some_and(|p| p.source.as_deref() == Some(marketplace));
        let root = self.marketplace_directory(marketplace, refresh).await?;
        let catalog: Value = serde_json::from_str(&fs::read_to_string(
            root.join(".claude-plugin/marketplace.json"),
        )?)?;
        let entry = catalog["plugins"]
            .as_array()
            .context("Missing plugins")?
            .iter()
            .find(|e| e["name"] == name)
            .context("Plugin not listed")?;
        let source = &entry["source"];
        if source.get("ref").is_some() || source.get("path").is_some() {
            bail!("Marketplace source ref/path overrides are unsupported; add a local checkout instead");
        }
        let directory = if let Some(path) = source.as_str() {
            let path = path.strip_prefix("./").unwrap_or(path);
            if !safe_relative(path) {
                bail!("Unsafe marketplace source path");
            }
            let directory = root.join(path).canonicalize()?;
            if !directory.starts_with(root.canonicalize()?) {
                bail!("Marketplace source escapes repository");
            }
            directory
        } else {
            let url = match source["source"].as_str() {
                Some("github") => format!(
                    "https://github.com/{}.git",
                    source["repo"].as_str().context("Missing repo")?
                ),
                Some("url") => source["url"].as_str().context("Missing URL")?.into(),
                _ => bail!("Unsupported marketplace source type"),
            };
            validate_source(&url)?;
            let staging = self
                .global
                .join("marketplace-cache")
                .join(format!("plugin-{}", revision()));
            clone_repo(&url, &staging).await?;
            staging
        };
        let mut spec = import_directory(&directory)?;
        spec.name = name.into();
        validate(&spec)?;
        self.mutate(scope, |registry| {
            let previous = registry.current.get(name);
            if previous.is_some_and(|p| p.source.as_deref() != Some(marketplace)) {
                bail!("A different plugin already uses this name; rename it first");
            }
            let plugin = Plugin {
                scope: scope.into(),
                revision: revision(),
                enabled: previous.map(|p| p.enabled).unwrap_or(true),
                source: Some(marketplace.into()),
                spec,
            };
            registry.revisions.insert(
                format!("{}--{}", plugin.spec.name, plugin.revision),
                plugin.clone(),
            );
            registry.current.insert(name.into(), plugin.clone());
            Ok(plugin)
        })
    }
}

fn validate_source(source: &str) -> anyhow::Result<()> {
    if source.starts_with("https://") {
        let url = reqwest::Url::parse(source)?;
        if !url.username().is_empty()
            || url.password().is_some()
            || url.query().is_some()
            || url.fragment().is_some()
        {
            bail!("Use a repository URL without credentials, query, or fragment");
        }
    } else if !Path::new(source).is_absolute() {
        bail!("Use an HTTPS repository URL or absolute local directory");
    }
    Ok(())
}
async fn clone_repo(source: &str, path: &Path) -> anyhow::Result<()> {
    fs::create_dir_all(path.parent().context("Missing cache parent")?)?;
    let mut command = tokio::process::Command::new("git");
    command
        .args(["-c", "core.hooksPath=/dev/null", "clone", "--depth=1", "--"])
        .arg(source)
        .arg(path)
        .env_clear();
    for name in ["PATH", "HOME", "SYSTEMROOT"] {
        if let Some(value) = std::env::var_os(name) {
            command.env(name, value);
        }
    }
    command
        .env("GIT_TERMINAL_PROMPT", "0")
        .kill_on_drop(true)
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null());
    let status = tokio::time::timeout(std::time::Duration::from_secs(60), command.status())
        .await
        .context("Marketplace download timed out")??;
    if !status.success() {
        bail!("Marketplace download failed; check the repository URL and network");
    }
    Ok(())
}

fn import_directory(root: &Path) -> anyhow::Result<PluginSpec> {
    let manifest_path = root.join(".claude-plugin/plugin.json");
    let manifest: Value = if manifest_path.exists() {
        serde_json::from_str(&fs::read_to_string(&manifest_path)?)?
    } else {
        serde_json::json!({})
    };
    let mut spec = PluginSpec {
        name: manifest["name"].as_str().unwrap_or("imported").into(),
        description: manifest["description"].as_str().unwrap_or_default().into(),
        version: manifest["version"].as_str().unwrap_or_default().into(),
        ..Default::default()
    };
    collect_files(
        root,
        root,
        &mut spec.files,
        &mut spec.executable_files,
        &mut spec.unsupported,
    )?;
    for (path, content) in &spec.files {
        if path.ends_with("/SKILL.md") || path == "SKILL.md" {
            let (metadata, instructions) = parse_skill(content)?;
            let fallback = Path::new(path)
                .parent()
                .and_then(Path::file_name)
                .and_then(|s| s.to_str())
                .unwrap_or("skill");
            let name = metadata.get("name").map(String::as_str).unwrap_or(fallback);
            let id = if safe_name(name) { name } else { fallback };
            spec.skills.push(Skill {
                id: id.into(),
                name: name.into(),
                description: metadata.get("description").cloned().unwrap_or_default(),
                instructions,
                allowed_tools: vec![],
                scripts: vec![],
            });
            if metadata.contains_key("allowed-tools") {
                spec.unsupported.push(format!(
                    "{path}: Claude tool names are not permission grants in Themis"
                ));
            }
        }
    }
    if let Some(raw) = spec.files.get(".mcp.json") {
        let config: Value = serde_json::from_str(raw)?;
        let servers = config.get("mcpServers").unwrap_or(&config);
        import_mcp(servers, &mut spec.mcp)?;
    }
    if let Some(servers) = manifest.get("mcpServers") {
        if servers.is_object() {
            import_mcp(servers, &mut spec.mcp)?;
        } else {
            spec.unsupported
                .push("External MCP configuration paths".into());
        }
    }
    if let Some(raw) = spec.files.get("hooks/hooks.json") {
        import_hooks(&serde_json::from_str::<Value>(raw)?, &mut spec)?;
    }
    if let Some(hooks) = manifest.get("hooks") {
        if hooks.is_object() {
            import_hooks(hooks, &mut spec)?;
        } else {
            spec.unsupported
                .push("External hook configuration paths".into());
        }
    }
    for field in [
        "agents",
        "lspServers",
        "outputStyles",
        "settings",
        "dependencies",
        "userConfig",
        "experimental",
    ] {
        if manifest.get(field).is_some() {
            spec.unsupported.push(field.into());
        }
    }
    if spec.files.keys().any(|p| p.starts_with("commands/")) {
        spec.unsupported
            .push("Legacy commands/ directory (use skills/)".into());
    }
    if spec.skills.is_empty() && (!spec.mcp.is_empty() || !spec.hooks.is_empty()) {
        spec.skills.push(Skill {
            id: "connect".into(),
            name: format!("Use {}", spec.name),
            description: "Use this plugin's connected tools and hooks".into(),
            instructions: "Use the plugin tools to complete the task. Respect Themis approvals."
                .into(),
            allowed_tools: vec![],
            scripts: vec![],
        });
    }
    Ok(spec)
}
fn collect_files(
    root: &Path,
    directory: &Path,
    files: &mut BTreeMap<String, String>,
    executable_files: &mut Vec<String>,
    unsupported: &mut Vec<String>,
) -> anyhow::Result<()> {
    for entry in fs::read_dir(directory)? {
        let entry = entry?;
        let kind = entry.file_type()?;
        let path = entry.path();
        if entry.file_name() == ".git" {
            continue;
        }
        if kind.is_symlink() {
            bail!("Plugin symlinks are unsupported");
        }
        if kind.is_dir() {
            if path.strip_prefix(root)?.components().count() > 32 {
                bail!("Plugin directory nesting exceeds 32 levels");
            }
            collect_files(root, &path, files, executable_files, unsupported)?;
        } else if kind.is_file() {
            if entry.metadata()?.len() > 1024 * 1024 {
                bail!("Plugin file exceeds 1 MiB");
            }
            let bytes = fs::read(&path)?;
            if !bytes.is_ascii() && std::str::from_utf8(&bytes).is_err() {
                unsupported.push(format!(
                    "Binary resource omitted: {}",
                    path.strip_prefix(root)?.display()
                ));
            }
            if let Ok(text) = String::from_utf8(bytes) {
                let relative = path
                    .strip_prefix(root)?
                    .to_string_lossy()
                    .replace('\\', "/");
                #[cfg(unix)]
                {
                    use std::os::unix::fs::PermissionsExt;
                    if entry.metadata()?.permissions().mode() & 0o111 != 0 {
                        executable_files.push(relative.clone());
                    }
                }
                files.insert(relative, text);
            }
            if files.len() > 1000
                || files.values().map(String::len).sum::<usize>() > 8 * 1024 * 1024
            {
                bail!("Plugin exceeds resource limits");
            }
        }
    }
    Ok(())
}
fn parse_skill(content: &str) -> anyhow::Result<(BTreeMap<String, String>, String)> {
    let Some(tail) = content.strip_prefix("---\n") else {
        return Ok((BTreeMap::new(), content.into()));
    };
    let end = tail.find("\n---").context("Unclosed skill frontmatter")?;
    let mut metadata = BTreeMap::new();
    let mut multiline: Option<String> = None;
    for line in tail[..end].lines() {
        if line.starts_with(' ') {
            if let Some(key) = &multiline {
                metadata.entry(key.clone()).and_modify(|v: &mut String| {
                    v.push(' ');
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
fn import_mcp(value: &Value, servers: &mut BTreeMap<String, McpServer>) -> anyhow::Result<()> {
    for (name, raw) in value.as_object().context("Invalid MCP configuration")? {
        let mut server: McpServer = serde_json::from_value(raw.clone())?;
        for reference in server.env.values_mut() {
            if let Some(variable) = reference
                .strip_prefix("${")
                .and_then(|s| s.strip_suffix('}'))
            {
                *reference = variable.into();
            }
        }
        server.enabled = false;
        servers.insert(name.clone(), server);
    }
    Ok(())
}
fn import_hooks(value: &Value, spec: &mut PluginSpec) -> anyhow::Result<()> {
    let Some(events) = value["hooks"].as_object() else {
        return Ok(());
    };
    for (event, groups) in events {
        let mapped = match event.as_str() {
            "SessionStart" => "RunStart",
            "PreToolUse" => "BeforeToolCall",
            "PostToolUse" => "AfterToolCall",
            "PostToolUseFailure" => "ToolCallFailed",
            "Stop" => "RunFinished",
            "PreCompact" => "BeforeCompaction",
            _ => {
                spec.unsupported.push(format!("Hook event {event}"));
                continue;
            }
        };
        for group in groups.as_array().context("Invalid hook groups")? {
            for raw in group["hooks"].as_array().context("Invalid hook list")? {
                if raw["type"] != "command" {
                    spec.unsupported.push(format!("{event}: non-command hook"));
                    continue;
                }
                if group["matcher"]
                    .as_str()
                    .is_some_and(|s| !s.is_empty() && s != "*")
                {
                    spec.unsupported
                        .push(format!("{event}: tool matcher requires manual mapping"));
                    continue;
                }
                spec.hooks.push(Hook {
                    name: format!("imported-{}", spec.hooks.len()),
                    event: mapped.into(),
                    command: raw["command"]
                        .as_str()
                        .context("Missing hook command")?
                        .into(),
                    enabled: false,
                    timeout_seconds: raw["timeout"].as_u64().unwrap_or(10).clamp(1, 60),
                    blocking: event == "PreToolUse" || event == "SessionStart",
                    plugin_root: None,
                    runtime_identity: None,
                });
            }
        }
    }
    Ok(())
}
