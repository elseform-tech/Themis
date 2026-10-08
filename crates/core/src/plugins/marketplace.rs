use super::*;
use serde_json::Value;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ImportComponent {
    pub kind: String,
    pub name: String,
    pub status: String,
    pub field: Option<String>,
    pub reason: Option<String>,
    pub remedy: Option<String>,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ImportReport {
    pub status: String,
    pub components: Vec<ImportComponent>,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ImportPreview {
    pub spec: PluginSpec,
    pub report: ImportReport,
}
impl ImportPreview {
    pub fn from_spec(spec: PluginSpec) -> anyhow::Result<Self> {
        validate(&spec)?;
        Ok(Self::new(spec))
    }
    fn new(spec: PluginSpec) -> Self {
        let mut components = spec.import_issues.clone();
        for (kind, names) in [
            (
                "skill",
                spec.skills.iter().map(|s| s.id.clone()).collect::<Vec<_>>(),
            ),
            ("mcp", spec.mcp.keys().cloned().collect()),
            ("hook", spec.hooks.iter().map(|h| h.name.clone()).collect()),
        ] {
            components.extend(names.into_iter().map(|name| ImportComponent {
                kind: kind.into(),
                name,
                status: "supported".into(),
                field: None,
                reason: None,
                remedy: None,
            }));
        }
        components.extend(spec.unsupported.iter().map(|reason| ImportComponent {
            kind: "package".into(),
            name: spec.name.clone(),
            status: "unsupported".into(),
            field: None,
            reason: Some(reason.clone()),
            remedy: Some(
                "Remove the unsupported component or install only supported components".into(),
            ),
        }));
        let status = if spec.skills.is_empty() && spec.mcp.is_empty() && spec.hooks.is_empty() {
            "unsupported"
        } else if components.iter().any(|c| c.status != "supported") {
            "partial"
        } else {
            "supported"
        };
        Self {
            spec,
            report: ImportReport {
                status: status.into(),
                components,
            },
        }
    }
}

#[derive(Debug)]
struct UnsupportedMcp {
    field: String,
    reason: &'static str,
    remedy: &'static str,
}
impl std::fmt::Display for UnsupportedMcp {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}: {}. {}", self.field, self.reason, self.remedy)
    }
}
impl std::error::Error for UnsupportedMcp {}
fn unsupported_mcp(
    field: impl Into<String>,
    reason: &'static str,
    remedy: &'static str,
) -> anyhow::Error {
    UnsupportedMcp {
        field: field.into(),
        reason,
        remedy,
    }
    .into()
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Marketplace {
    pub name: String,
    pub source: String,
}
#[derive(Default, Serialize, Deserialize)]
struct Catalogs {
    sources: Vec<Marketplace>,
}

impl PluginSpec {
    /// Installation must add a capability; empty personal drafts may still be saved.
    pub fn ensure_installable(&self) -> anyhow::Result<()> {
        if self.skills.is_empty() && self.mcp.is_empty() && self.hooks.is_empty() {
            bail!("No supported capabilities to install. Inspect compatibility details or choose another package.");
        }
        Ok(())
    }
}

impl PluginStore {
    /// Import a local skill/plugin folder, SKILL.md, native JSON bundle, or MCP JSON file.
    pub fn import_path(
        &self,
        scope: &str,
        path: &Path,
        name: Option<&str>,
    ) -> anyhow::Result<Plugin> {
        self.import_path_with_options(scope, path, name, false)
    }
    pub fn import_path_with_options(
        &self,
        scope: &str,
        path: &Path,
        name: Option<&str>,
        allow_partial: bool,
    ) -> anyhow::Result<Plugin> {
        let spec = self.path_spec(path, name, allow_partial)?;
        spec.ensure_installable()?;
        self.save(scope, spec, None)
    }
    /// Inspect compatibility without registering or executing plugin capabilities.
    pub fn inspect_path(&self, path: &Path, name: Option<&str>) -> anyhow::Result<ImportPreview> {
        Ok(ImportPreview::new(self.path_spec(path, name, true)?))
    }
    fn path_spec(
        &self,
        path: &Path,
        name: Option<&str>,
        allow_partial: bool,
    ) -> anyhow::Result<PluginSpec> {
        let path = path.canonicalize().context("Import path is unavailable")?;
        let mut spec = if path.is_dir() || path.file_name().is_some_and(|s| s == "SKILL.md") {
            import_directory(
                if path.is_dir() {
                    &path
                } else {
                    path.parent().context("Missing skill directory")?
                },
                allow_partial,
            )?
        } else {
            let bytes = fs::read(&path)?;
            if bytes.len() > 8 * 1024 * 1024 {
                bail!("Import file exceeds 8 MiB");
            }
            let value: Value = serde_json::from_slice(&bytes).context(
                "Import expects a skill/plugin folder or JSON file; archives are unsupported",
            )?;
            if let Some(spec) = value.get("spec") {
                serde_json::from_value(spec.clone())?
            } else if value.get("name").is_some_and(Value::is_string) {
                serde_json::from_value(value)?
            } else {
                let name = name.unwrap_or("imported-mcp");
                mcp_spec(name, &value, allow_partial)?
            }
        };
        if let Some(name) = name {
            spec.name = name.into();
        }
        spec.origin = Some(PluginOrigin {
            kind: "local".into(),
            location: path.to_string_lossy().into(),
            reference: None,
            subdirectory: None,
        });
        validate(&spec)?;
        if !allow_partial && !spec.import_issues.is_empty() {
            bail!("Plugin contains unsupported components; inspect it and explicitly select partial installation");
        }
        Ok(spec)
    }
    pub fn import_mcp_json(&self, scope: &str, name: &str, input: &str) -> anyhow::Result<Plugin> {
        self.import_mcp_json_with_options(scope, name, input, false)
    }
    pub fn import_mcp_json_with_options(
        &self,
        scope: &str,
        name: &str,
        input: &str,
        allow_partial: bool,
    ) -> anyhow::Result<Plugin> {
        let spec = self.mcp_json_spec(name, input, allow_partial)?;
        spec.ensure_installable()?;
        self.save(scope, spec, None)
    }
    pub fn inspect_mcp_json(&self, name: &str, input: &str) -> anyhow::Result<ImportPreview> {
        Ok(ImportPreview::new(self.mcp_json_spec(name, input, true)?))
    }
    fn mcp_json_spec(
        &self,
        name: &str,
        input: &str,
        allow_partial: bool,
    ) -> anyhow::Result<PluginSpec> {
        if input.len() > 1024 * 1024 {
            bail!("MCP configuration exceeds 1 MiB");
        }
        let value: Value = serde_json::from_str(input).context("Invalid MCP JSON")?;
        let mut spec = mcp_spec(name, &value, allow_partial)?;
        spec.origin = Some(PluginOrigin {
            kind: "mcp-json".into(),
            location: "pasted configuration".into(),
            reference: None,
            subdirectory: None,
        });
        validate(&spec)?;
        Ok(spec)
    }
    pub async fn import_repository(
        &self,
        scope: &str,
        name: &str,
        source: &str,
        reference: Option<&str>,
        subdirectory: Option<&str>,
    ) -> anyhow::Result<Plugin> {
        self.import_repository_with_options(scope, name, source, reference, subdirectory, false)
            .await
    }
    pub async fn import_repository_with_options(
        &self,
        scope: &str,
        name: &str,
        source: &str,
        reference: Option<&str>,
        subdirectory: Option<&str>,
        allow_partial: bool,
    ) -> anyhow::Result<Plugin> {
        let spec = self
            .repository_spec(name, source, reference, subdirectory, allow_partial)
            .await?;
        spec.ensure_installable()?;
        self.save(scope, spec, None)
    }
    pub async fn inspect_repository(
        &self,
        name: &str,
        source: &str,
        reference: Option<&str>,
        subdirectory: Option<&str>,
    ) -> anyhow::Result<ImportPreview> {
        Ok(ImportPreview::new(
            self.repository_spec(name, source, reference, subdirectory, true)
                .await?,
        ))
    }
    pub async fn preview_repository(
        &self,
        name: &str,
        source: &str,
        reference: Option<&str>,
        subdirectory: Option<&str>,
    ) -> anyhow::Result<PluginSpec> {
        self.repository_spec(name, source, reference, subdirectory, false)
            .await
    }
    async fn repository_spec(
        &self,
        name: &str,
        source: &str,
        reference: Option<&str>,
        subdirectory: Option<&str>,
        allow_partial: bool,
    ) -> anyhow::Result<PluginSpec> {
        if !name.is_empty() && !safe_name(name) {
            bail!("Invalid plugin name");
        }
        validate_source(source)?;
        if subdirectory.is_some_and(|path| !safe_relative(path)) {
            bail!("Unsafe repository subdirectory");
        }
        let staging = self
            .global
            .join("marketplace-cache")
            .join(format!("import-{}", revision()));
        let result = async {
            clone_repository(source, &staging, reference).await?;
            let directory = subdirectory
                .map(|path| staging.join(path))
                .unwrap_or_else(|| staging.clone())
                .canonicalize()?;
            if !directory.starts_with(staging.canonicalize()?) {
                bail!("Plugin directory escapes repository");
            }
            let mut spec = import_directory(&directory, allow_partial)?;
            if !name.is_empty() {
                spec.name = name.into();
            } else if directory == staging.canonicalize()?
                && spec.name == staging.file_name().unwrap_or_default().to_string_lossy()
            {
                // A manifest or nested skill folder supplies its own name; never expose staging IDs.
                spec.name = if spec.files.contains_key("SKILL.md") && spec.skills.len() == 1 {
                    spec.skills[0].id.clone()
                } else {
                    source
                        .trim_end_matches('/')
                        .rsplit('/')
                        .next()
                        .unwrap_or_default()
                        .trim_end_matches(".git")
                        .into()
                };
            }
            spec.origin = Some(PluginOrigin {
                kind: "repository".into(),
                location: source.into(),
                reference: reference.map(str::to_owned),
                subdirectory: subdirectory.map(str::to_owned),
            });
            validate(&spec)?;
            Ok(spec)
        }
        .await;
        if staging.exists() {
            let _ = fs::remove_dir_all(&staging);
        }
        result
    }
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
        self.install_with_options(scope, marketplace, name, false)
            .await
    }
    pub async fn install_with_options(
        &self,
        scope: &str,
        marketplace: &str,
        name: &str,
        allow_partial: bool,
    ) -> anyhow::Result<Plugin> {
        let refresh = self
            .registry(scope)?
            .current
            .get(name)
            .is_some_and(|p| p.source.as_deref() == Some(marketplace));
        let mut spec = self
            .marketplace_spec(marketplace, name, refresh, allow_partial)
            .await?;
        spec.ensure_installable()?;
        self.mutate(scope, |registry| {
            let previous = registry.current.get(name);
            if previous.is_some_and(|p| p.source.as_deref() != Some(marketplace)) {
                bail!("A different plugin already uses this name; rename it first");
            }
            if let Some(previous) = previous {
                for (name, server) in &mut spec.mcp {
                    server.enabled = previous.spec.mcp.get(name).is_some_and(|s| s.enabled);
                }
                for hook in &mut spec.hooks {
                    hook.enabled = previous
                        .spec
                        .hooks
                        .iter()
                        .find(|h| h.name == hook.name)
                        .is_some_and(|h| h.enabled);
                }
                for id in &previous.spec.disabled_skills {
                    if spec.skills.iter().any(|s| &s.id == id) && !spec.disabled_skills.contains(id)
                    {
                        spec.disabled_skills.push(id.clone());
                    }
                }
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
    /// Read supported package contents without registering or enabling any capability.
    pub async fn preview(&self, marketplace: &str, name: &str) -> anyhow::Result<PluginSpec> {
        self.marketplace_spec(marketplace, name, false, false).await
    }
    pub async fn inspect_marketplace(
        &self,
        marketplace: &str,
        name: &str,
    ) -> anyhow::Result<ImportPreview> {
        Ok(ImportPreview::new(
            self.marketplace_spec(marketplace, name, false, true)
                .await?,
        ))
    }
    async fn marketplace_spec(
        &self,
        marketplace: &str,
        name: &str,
        refresh: bool,
        allow_partial: bool,
    ) -> anyhow::Result<PluginSpec> {
        if !safe_name(name) {
            bail!("Invalid plugin name");
        }
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
        let mut temporary_root = None;
        let result = async {
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
                    Some("url" | "git-subdir") => {
                        source["url"].as_str().context("Missing URL")?.into()
                    }
                    _ => bail!("Unsupported marketplace source type"),
                };
                validate_source(&url)?;
                let staging = self
                    .global
                    .join("marketplace-cache")
                    .join(format!("plugin-{}", revision()));
                temporary_root = Some(staging.clone());
                clone_repository(
                    &url,
                    &staging,
                    source
                        .get("sha")
                        .or_else(|| source.get("ref"))
                        .map(|value| value.as_str().context("Invalid repository reference"))
                        .transpose()?,
                )
                .await?;
                if let Some(path) = source.get("path") {
                    let path = path.as_str().context("Invalid repository subdirectory")?;
                    if !safe_relative(path) {
                        bail!("Unsafe repository subdirectory");
                    }
                    let directory = staging.join(path).canonicalize()?;
                    if !directory.starts_with(staging.canonicalize()?) {
                        bail!("Plugin directory escapes repository");
                    }
                    directory
                } else {
                    staging
                }
            };
            let mut spec = import_directory(&directory, allow_partial)?;
            spec.name = name.into();
            spec.package_kind = Some("plugin".into());
            for field in [
                "skills",
                "commands",
                "agents",
                "hooks",
                "mcpServers",
                "lspServers",
                "dependencies",
                "settings",
                "userConfig",
                "channels",
            ] {
                if entry.get(field).is_some() {
                    spec.unsupported.push(format!(
                        "Marketplace entry {field}: component declarations require plugin manifest"
                    ));
                }
            }
            if let Some(icon) = entry.get("icon").and_then(Value::as_str) {
                let icon = icon.strip_prefix("./").unwrap_or(icon);
                if icon.starts_with("https://")
                    || (icon.ends_with(".svg") && spec.files.contains_key(icon))
                {
                    spec.icon = Some(icon.into());
                } else {
                    spec.unsupported
                        .push("Marketplace icon resource is unavailable or unsupported".into());
                }
            }
            spec.origin = Some(PluginOrigin {
                kind: "marketplace".into(),
                location: marketplace.into(),
                reference: source
                    .get("sha")
                    .or_else(|| source.get("ref"))
                    .and_then(Value::as_str)
                    .map(str::to_owned),
                subdirectory: source
                    .get("path")
                    .and_then(Value::as_str)
                    .or_else(|| source.as_str())
                    .map(str::to_owned),
            });
            validate(&spec)?;
            Ok(spec)
        }
        .await;
        if let Some(directory) = temporary_root {
            let _ = fs::remove_dir_all(directory);
        }
        result
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
    clone_repository(source, path, None).await
}
async fn clone_repository(
    source: &str,
    path: &Path,
    reference: Option<&str>,
) -> anyhow::Result<()> {
    if reference.is_some_and(|value| {
        value.is_empty() || value.starts_with('-') || value.chars().any(char::is_control)
    }) {
        bail!("Invalid repository reference");
    }
    fs::create_dir_all(path.parent().context("Missing cache parent")?)?;
    run_git(
        &[
            "clone",
            "--depth=1",
            "--",
            source,
            path.to_str().context("Invalid cache path")?,
        ],
        None,
    )
    .await?;
    if let Some(reference) = reference {
        run_git(&["fetch", "--depth=1", "origin", reference], Some(path)).await?;
        run_git(&["checkout", "--detach", "FETCH_HEAD"], Some(path)).await?;
    }
    Ok(())
}
async fn run_git(args: &[&str], directory: Option<&Path>) -> anyhow::Result<()> {
    let mut command = tokio::process::Command::new("git");
    command
        .args(["-c", "core.hooksPath=/dev/null"])
        .args(args)
        .env_clear();
    if let Some(directory) = directory {
        command.current_dir(directory);
    }
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
        .context("Repository download timed out")??;
    if !status.success() {
        bail!("Repository download failed; check repository access, reference, and network");
    }
    Ok(())
}

fn import_directory(root: &Path, allow_partial: bool) -> anyhow::Result<PluginSpec> {
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
    collect_files(
        root,
        root,
        &mut spec.files,
        &mut spec.executable_files,
        &mut spec.unsupported,
    )?;
    if let Some(icon) = manifest.get("icon").and_then(Value::as_str) {
        let icon = icon.strip_prefix("./").unwrap_or(icon);
        if icon.starts_with("https://") || (icon.ends_with(".svg") && spec.files.contains_key(icon))
        {
            spec.icon = Some(icon.into());
        } else {
            spec.unsupported
                .push("Plugin icon resource is unavailable or unsupported".into());
        }
    }
    for (path, content) in &spec.files {
        if path.ends_with("/SKILL.md") || path == "SKILL.md" {
            let (metadata, instructions) = parse_skill(content)?;
            let fallback = if path == "SKILL.md" {
                root.file_name().and_then(|s| s.to_str()).unwrap_or("skill")
            } else {
                Path::new(path)
                    .parent()
                    .and_then(Path::file_name)
                    .and_then(|s| s.to_str())
                    .unwrap_or("skill")
            };
            let name = metadata.get("name").map(String::as_str).unwrap_or(fallback);
            let id = if safe_name(name) { name } else { fallback };
            spec.skill_paths.insert(id.into(), path.clone());
            spec.skills.push(Skill {
                id: id.into(),
                name: name.into(),
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
                spec.unsupported
                    .push(format!("{path}: skill frontmatter {key}"));
            }
            if metadata
                .get("disable-model-invocation")
                .is_some_and(|value| value == "true")
            {
                spec.manual_skills.push(id.into());
            }
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
        import_mcp_components(servers, false, &mut spec, allow_partial)?;
    }
    if let Some(servers) = manifest.get("mcpServers") {
        if servers.is_object() {
            import_mcp_components(servers, false, &mut spec, allow_partial)?;
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
        "skills",
        "commands",
        "agents",
        "channels",
        "defaultEnabled",
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
    for (directory, label) in [
        ("agents/", "Claude subagents"),
        ("bin/", "Plugin PATH executables"),
        ("output-styles/", "Output styles"),
    ] {
        if spec.files.keys().any(|path| path.starts_with(directory)) {
            spec.unsupported.push(label.into());
        }
    }
    if spec.files.keys().any(|p| p.starts_with("commands/")) {
        spec.unsupported
            .push("Legacy commands/ directory (use skills/)".into());
    }
    Ok(spec)
}
pub(super) fn collect_files(
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
pub(super) fn parse_skill(content: &str) -> anyhow::Result<(BTreeMap<String, String>, String)> {
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
fn mcp_spec(name: &str, value: &Value, allow_partial: bool) -> anyhow::Result<PluginSpec> {
    let (servers, opencode) = if let Some(servers) = value.get("mcpServers") {
        if value
            .as_object()
            .is_some_and(|v| v.keys().any(|key| key != "mcpServers"))
        {
            bail!("Only MCP configuration is imported; remove unrelated root settings");
        }
        (servers, false)
    } else if let Some(servers) = value.get("mcp") {
        if value.as_object().is_some_and(|v| {
            v.keys()
                .any(|key| !["mcp", "$schema"].contains(&key.as_str()))
        }) {
            bail!("Only OpenCode MCP configuration is imported; remove unrelated root settings");
        }
        (servers, true)
    } else {
        (value, false)
    };
    let mut spec = PluginSpec {
        package_kind: Some("mcp".into()),
        name: name.into(),
        ..Default::default()
    };
    import_mcp_components(servers, opencode, &mut spec, allow_partial)?;
    if spec.mcp.is_empty() && spec.import_issues.is_empty() {
        bail!("MCP configuration has no connections");
    }
    Ok(spec)
}
fn import_mcp_components(
    value: &Value,
    opencode: bool,
    spec: &mut PluginSpec,
    allow_partial: bool,
) -> anyhow::Result<()> {
    let root = if opencode { "mcp" } else { "mcpServers" };
    for (name, raw) in value.as_object().context("Invalid MCP configuration")? {
        match import_mcp_server(name, raw, opencode) {
            Ok(server) => {
                if spec.mcp.insert(name.clone(), server).is_some() {
                    bail!("Duplicate MCP connection '{name}'");
                }
            }
            Err(error) => {
                if allow_partial {
                    if let Some(issue) = error.downcast_ref::<UnsupportedMcp>() {
                        spec.import_issues.push(ImportComponent {
                            kind: "mcp".into(),
                            name: name.clone(),
                            status: "unsupported".into(),
                            field: Some(format!("{root}.{}", issue.field)),
                            reason: Some(issue.reason.into()),
                            remedy: Some(issue.remedy.into()),
                        });
                        continue;
                    }
                }
                return Err(error.context(format!("{root}.{name}")));
            }
        }
    }
    Ok(())
}
fn environment_reference(value: &str) -> anyhow::Result<String> {
    let name = value.strip_prefix("${").and_then(|v| v.strip_suffix('}'))
        .or_else(|| value.strip_prefix("{env:").and_then(|v| v.strip_suffix('}')))
        .context("Imported credentials/environment values must use ${VARIABLE} or {env:VARIABLE}; literal values are unsupported")?;
    if name.is_empty() || !name.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'_') {
        bail!("Unsupported environment reference");
    }
    Ok(name.into())
}
fn import_mcp_server(name: &str, raw: &Value, opencode: bool) -> anyhow::Result<McpServer> {
    if !safe_name(name) {
        bail!("Invalid MCP connection name");
    }
    let fields = raw
        .as_object()
        .context("MCP connection must be an object")?;
    let allowed: &[&str] = if opencode {
        &[
            "type",
            "command",
            "environment",
            "url",
            "headers",
            "enabled",
            "oauth",
        ]
    } else {
        &[
            "type",
            "command",
            "args",
            "env",
            "url",
            "headers",
            "enabled",
            "bearer_env",
        ]
    };
    let mut compatibility_error = None;
    if let Some(field) = fields.keys().find(|key| !allowed.contains(&key.as_str())) {
        compatibility_error = Some(unsupported_mcp(
            format!("{name}.{field}"),
            "Unsupported MCP field",
            "Remove this field or configure the component manually",
        ));
    }
    if let Some(enabled) = raw.get("enabled") {
        if !enabled.is_boolean() {
            bail!("MCP enabled must be a boolean");
        }
    }
    if let Some(kind) = raw.get("type") {
        let kind = kind
            .as_str()
            .context("MCP transport type must be a string")?;
        let supported = if opencode {
            ["local", "remote"].contains(&kind)
        } else {
            ["stdio", "http", "streamable-http"].contains(&kind)
        };
        if !supported {
            compatibility_error = Some(unsupported_mcp(
                format!("{name}.type"),
                "Unsupported MCP transport",
                "Use stdio or Streamable HTTP; SSE is not supported",
            ));
        }
    }
    if raw.get("oauth").is_some_and(|v| v != &Value::Bool(false)) {
        if let Some(secret) = raw["oauth"].get("clientSecret") {
            environment_reference(
                secret
                    .as_str()
                    .context("OAuth secret must be an environment reference")?,
            )?;
        }
        compatibility_error = Some(unsupported_mcp(
            format!("{name}.oauth"),
            "Browser OAuth is not supported",
            "Use an environment bearer reference when supported by the server",
        ));
    }
    let mut server = McpServer::default();
    if let Some(command) = raw.get("command") {
        if opencode {
            let command = command
                .as_array()
                .context("OpenCode command must be an array")?;
            server.command = Some(
                command
                    .first()
                    .and_then(Value::as_str)
                    .context("Empty MCP command")?
                    .into(),
            );
            server.args = command[1..]
                .iter()
                .map(|v| {
                    v.as_str()
                        .map(str::to_owned)
                        .context("MCP command arguments must be strings")
                })
                .collect::<anyhow::Result<_>>()?;
        } else {
            server.command = Some(
                command
                    .as_str()
                    .context("MCP command must be a string")?
                    .into(),
            );
        }
    }
    if let Some(args) = raw.get("args") {
        server.args = serde_json::from_value(args.clone()).context("Invalid MCP arguments")?;
    }
    if let Some(url) = raw.get("url") {
        server.url = Some(url.as_str().context("MCP URL must be a string")?.into());
    }
    if let Some(env) = raw.get(if opencode { "environment" } else { "env" }) {
        for (key, value) in env
            .as_object()
            .context("MCP environment must be an object")?
        {
            server.env.insert(
                key.clone(),
                environment_reference(
                    value
                        .as_str()
                        .context("MCP environment must contain strings")?,
                )?,
            );
        }
    }
    if let Some(bearer) = raw.get("bearer_env") {
        server.bearer_env = Some(
            bearer
                .as_str()
                .context("Invalid bearer environment reference")?
                .into(),
        );
    }
    if let Some(headers) = raw.get("headers") {
        for (key, value) in headers
            .as_object()
            .context("MCP headers must be an object")?
        {
            if !key.eq_ignore_ascii_case("authorization") || server.bearer_env.is_some() {
                environment_reference(
                    value
                        .as_str()
                        .context("MCP header must be an environment reference")?,
                )?;
                compatibility_error = Some(unsupported_mcp(
                    format!("{name}.headers"),
                    "Unsupported or duplicate authentication headers",
                    "Use one Authorization Bearer environment reference",
                ));
                continue;
            }
            let token = value
                .as_str()
                .and_then(|s| s.strip_prefix("Bearer "))
                .context("Only Bearer environment reference headers are supported")?;
            server.bearer_env = Some(environment_reference(token)?);
        }
    }
    if let Some(kind) = raw.get("type").and_then(Value::as_str) {
        if (["local", "stdio"].contains(&kind) && server.command.is_none())
            || (["remote", "http", "streamable-http"].contains(&kind) && server.url.is_none())
        {
            bail!("MCP transport conflicts with connection configuration");
        }
    }
    if server.command.is_some() && server.bearer_env.is_some() {
        bail!("Stdio authentication must use environment references");
    }
    // Validate credentials and connection shape even when a component is unsupported.
    // Partial installation must never turn an unsafe configuration into a saved resource.
    server.validate()?;
    if let Some(error) = compatibility_error {
        return Err(error);
    }
    Ok(server)
}
fn import_hooks(value: &Value, spec: &mut PluginSpec) -> anyhow::Result<()> {
    let events = value
        .get("hooks")
        .unwrap_or(value)
        .as_object()
        .context("Invalid hook configuration")?;
    for (event, groups) in events {
        let mapped = match event.as_str() {
            "SessionStart" => "RunStart",
            "PreToolUse" => "BeforeTool",
            "PostToolUse" => "AfterTool",
            "Stop" => "RunEnd",
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
                if !matches!(event.as_str(), "PreToolUse" | "PostToolUse")
                    && group["matcher"]
                        .as_str()
                        .is_some_and(|matcher| !matcher.is_empty() && matcher != "*")
                {
                    spec.unsupported.push(format!(
                        "{event}: lifecycle matcher requires manual mapping"
                    ));
                    continue;
                }
                if raw.as_object().is_some_and(|value| {
                    value
                        .keys()
                        .any(|key| !["type", "command", "timeout"].contains(&key.as_str()))
                }) {
                    spec.unsupported
                        .push(format!("{event}: hook options require manual mapping"));
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
                    matcher: group["matcher"]
                        .as_str()
                        .filter(|value| !value.is_empty() && *value != "*")
                        .map(str::to_owned),
                    failure_policy: None,
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
