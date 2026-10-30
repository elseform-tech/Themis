use super::*;
use serde_json::{json, Value};

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
    /// Identifies a saved marketplace check of this entry, so an install can refuse a newer one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub check_revision: Option<String>,
    /// [`package_hash`] of the checked package, kept when the saved check drops its contents.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub package_hash: Option<String>,
}
/// What a package contains, ignoring what people change after installing it: switches and the
/// record of an earlier hash.
pub(super) fn package_hash(spec: &PluginSpec) -> String {
    use sha2::{Digest, Sha256};
    let mut value = serde_json::to_value(spec).unwrap_or_default();
    for server in value["mcp"]
        .as_object_mut()
        .into_iter()
        .flat_map(|mcp| mcp.values_mut())
    {
        server["enabled"] = Value::Null;
    }
    for hook in value["hooks"].as_array_mut().into_iter().flatten() {
        hook["enabled"] = Value::Null;
    }
    for field in ["package_hash", "disabled_skills", "origin"] {
        value[field] = Value::Null;
    }
    format!("{:x}", Sha256::digest(value.to_string()))
}
/// A definition without its switch, to tell a changed part from one only turned on or off.
fn same_definition<T: Serialize>(old: &T, new: &T) -> bool {
    let plain = |part: &T| {
        let mut value = serde_json::to_value(part).unwrap_or_default();
        value["enabled"] = Value::Null;
        value
    };
    plain(old) == plain(new)
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
        components.extend(spec.notices.iter().map(|reason| ImportComponent {
            kind: "package".into(),
            name: spec.name.clone(),
            status: "notice".into(),
            field: None,
            reason: Some(reason.clone()),
            remedy: None,
        }));
        let status = if spec.skills.is_empty() && spec.mcp.is_empty() && spec.hooks.is_empty() {
            "unsupported"
        } else if components
            .iter()
            .any(|c| !matches!(c.status.as_str(), "supported" | "notice"))
        {
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
            check_revision: None,
            package_hash: None,
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
    /// Whether Discover shows this source.
    #[serde(default = "shown")]
    pub enabled: bool,
}
fn shown() -> bool {
    true
}
/// The source every install starts with; it can be hidden but not removed.
pub(super) const OFFICIAL_MARKETPLACE: &str = "claude-plugins-official";
/// What a source is, read before adding it.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct SourcePreview {
    pub name: String,
    pub publisher: String,
    pub plugins: usize,
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
                None,
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
            let mut spec = import_directory(&directory, allow_partial, None)?;
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
                        .trim_end_matches(['/', '\\'])
                        .rsplit(['/', '\\'])
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
        // Registration writes use atomic rename; existing snapshots need no exclusive read lock.
        if self.global.join("marketplaces.json").is_file() {
            return self.read_marketplaces();
        }
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
                        name: OFFICIAL_MARKETPLACE.into(),
                        source: "https://github.com/anthropics/claude-plugins-official.git".into(),
                        enabled: true,
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
        let source = normalize_source(source)?;
        let lock = self.marketplace_lock()?;
        let mut sources = self.read_marketplaces()?;
        if sources.iter().any(|s| s.name == name) {
            bail!("A source named {name} is already added");
        }
        sources.push(Marketplace {
            name: name.into(),
            source,
            enabled: true,
        });
        let result = write_json(
            &self.global.join("marketplaces.json"),
            &Catalogs { sources },
        );
        lock.unlock()?;
        result
    }
    /// Reads a source's manifest without adding it: `owner/repo`, an HTTPS Git URL or a local
    /// folder. A Git source only fetches the manifest file.
    pub async fn preview_source(&self, input: &str) -> anyhow::Result<SourcePreview> {
        let source = normalize_source(input)?;
        if source.starts_with("https://") {
            let staging = self
                .global
                .join("marketplace-cache")
                .join(format!(".preview-{}", revision()));
            let result = read_git_manifest(&source, &staging).await;
            if staging.exists() {
                let _ = fs::remove_dir_all(&staging);
            }
            source_preview(&source, &result?)
        } else {
            let directory = PathBuf::from(&source);
            if !directory.is_dir() {
                bail!("This folder can't be read.");
            }
            source_preview(&source, &directory)
        }
    }
    /// Adds a source under the name in its manifest, and returns that name.
    pub async fn add_source(&self, input: &str) -> anyhow::Result<String> {
        let preview = self.preview_source(input).await?;
        self.add_marketplace(&preview.name, &preview.source)?;
        Ok(preview.name)
    }
    pub fn set_marketplace_enabled(&self, name: &str, enabled: bool) -> anyhow::Result<()> {
        let lock = self.marketplace_lock()?;
        let mut sources = self.read_marketplaces()?;
        sources
            .iter_mut()
            .find(|source| source.name == name)
            .context("Unknown marketplace")?
            .enabled = enabled;
        let result = write_json(
            &self.global.join("marketplaces.json"),
            &Catalogs { sources },
        );
        lock.unlock()?;
        result
    }
    /// Removes a source and its saved checks. Plugins installed from it stay installed.
    pub fn remove_marketplace(&self, name: &str) -> anyhow::Result<()> {
        if name == OFFICIAL_MARKETPLACE {
            bail!("The official source can't be removed; turn it off instead");
        }
        let _scan_lock = self.scan_lock(name)?;
        let lock = self.marketplace_lock()?;
        let mut sources = self.read_marketplaces()?;
        if !sources.iter().any(|s| s.name == name) {
            bail!("Unknown marketplace");
        }
        sources.retain(|s| s.name != name);
        let scan = self
            .global
            .join("compatibility-cache")
            .join(name)
            .join("scan.json");
        if scan.exists() {
            fs::remove_file(scan)?;
        }
        let result = write_json(
            &self.global.join("marketplaces.json"),
            &Catalogs { sources },
        );
        lock.unlock()?;
        result
    }
    pub async fn catalog(&self, name: &str, refresh: bool) -> anyhow::Result<Value> {
        let _lock = self.scan_lock(name)?;
        self.catalog_for_scan(name, refresh).await
    }
    pub(super) async fn catalog_for_scan(
        &self,
        name: &str,
        refresh: bool,
    ) -> anyhow::Result<Value> {
        let directory = self.marketplace_directory(name, refresh).await?;
        let path = directory.join(".claude-plugin/marketplace.json");
        let value: Value = serde_json::from_str(&fs::read_to_string(path)?)?;
        if !value["plugins"].is_array() {
            bail!("Marketplace has no plugin list");
        }
        Ok(value)
    }
    pub(super) async fn marketplace_directory(
        &self,
        name: &str,
        refresh: bool,
    ) -> anyhow::Result<PathBuf> {
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
        let _scan_lock = self.scan_lock(marketplace)?;
        let refresh = self
            .registry(scope)?
            .current
            .get(name)
            .is_some_and(|p| p.source.as_deref() == Some(marketplace));
        let spec = self
            .marketplace_spec(marketplace, name, refresh, allow_partial)
            .await?;
        spec.ensure_installable()?;
        self.install_spec(scope, marketplace, name, spec)
    }
    pub(super) fn install_spec(
        &self,
        scope: &str,
        marketplace: &str,
        name: &str,
        mut spec: PluginSpec,
    ) -> anyhow::Result<Plugin> {
        self.mutate(scope, |registry| {
            let previous = registry.current.get(name);
            if previous.is_some_and(|p| p.source.as_deref() != Some(marketplace)) {
                bail!("A different plugin already uses this name; rename it first");
            }
            spec.package_hash = Some(package_hash(&spec));
            if let Some(previous) = previous {
                // Parts that are new or changed arrive turned off, however the old one was set.
                for (name, server) in &mut spec.mcp {
                    server.enabled = previous
                        .spec
                        .mcp
                        .get(name)
                        .is_some_and(|old| old.enabled && same_definition(old, &*server));
                }
                for hook in &mut spec.hooks {
                    hook.enabled = previous
                        .spec
                        .hooks
                        .iter()
                        .find(|old| old.name == hook.name)
                        .is_some_and(|old| old.enabled && same_definition(old, &*hook));
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
        let _lock = self.scan_lock(marketplace)?;
        self.marketplace_spec(marketplace, name, false, false).await
    }
    pub async fn inspect_marketplace(
        &self,
        marketplace: &str,
        name: &str,
    ) -> anyhow::Result<ImportPreview> {
        let _lock = self.scan_lock(marketplace)?;
        self.inspect_for_scan(marketplace, name).await
    }
    pub(super) async fn inspect_for_scan(
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
            let mut spec = import_directory(&directory, allow_partial, Some(entry))?;
            spec.name = name.into();
            spec.package_kind = Some("plugin".into());
            for field in [
                "agents",
                "lspServers",
                "dependencies",
                "settings",
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

/// The stored form of what a person typed: `owner/repo` becomes a GitHub URL, and a `~` path an
/// absolute one.
fn normalize_source(input: &str) -> anyhow::Result<String> {
    let input = input.trim();
    let is_part = |part: &str| {
        !part.is_empty()
            && !part.starts_with('.')
            && part
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b"_.-".contains(&b))
    };
    let source = match input.split_once('/') {
        Some((owner, repo)) if is_part(owner) && is_part(repo) => {
            format!("https://github.com/{owner}/{repo}.git")
        }
        _ => match input.strip_prefix("~/").zip(std::env::var_os("HOME")) {
            Some((rest, home)) => Path::new(&home).join(rest).to_string_lossy().into_owned(),
            None => input.to_owned(),
        },
    };
    validate_source(&source)?;
    Ok(source)
}
/// Fetches only `.claude-plugin/marketplace.json` into `staging`: shallow, sparse and without
/// other file contents, so previewing a large repository stays cheap.
async fn read_git_manifest(source: &str, staging: &Path) -> anyhow::Result<PathBuf> {
    fs::create_dir_all(staging.parent().context("Missing cache parent")?)?;
    let path = staging.to_str().context("Invalid cache path")?;
    let unreachable = "This source can't be reached.";
    run_git(
        &[
            "clone",
            "--depth=1",
            "--filter=blob:none",
            "--no-checkout",
            "--sparse",
            "--",
            source,
            path,
        ],
        None,
    )
    .await
    .context(unreachable)?;
    run_git(
        &[
            "sparse-checkout",
            "set",
            "--no-cone",
            "/.claude-plugin/marketplace.json",
        ],
        Some(staging),
    )
    .await
    .context(unreachable)?;
    run_git(&["checkout"], Some(staging))
        .await
        .context(unreachable)?;
    Ok(staging.to_path_buf())
}
/// What the manifest in `directory` says about a source, or the one-line reason it isn't one.
fn source_preview(source: &str, directory: &Path) -> anyhow::Result<SourcePreview> {
    let path = directory.join(".claude-plugin/marketplace.json");
    let Ok(metadata) = fs::metadata(&path) else {
        bail!("No marketplace found in this source.");
    };
    if metadata.len() > 8 * 1024 * 1024 {
        bail!("This marketplace's catalog is too large.");
    }
    let value: Value = serde_json::from_str(&fs::read_to_string(&path)?)
        .ok()
        .filter(|value: &Value| value["plugins"].is_array())
        .context("No marketplace found in this source.")?;
    let plugins = value["plugins"].as_array().map_or(0, Vec::len);
    if plugins > 2000 {
        bail!("This marketplace has more than 2,000 plugins.");
    }
    // Catalogs may leave out their name; the source's last path part then names them.
    let folder = source
        .trim_end_matches('/')
        .trim_end_matches(".git")
        .rsplit(['/', '\\'])
        .next()
        .unwrap_or_default();
    let name = value["name"].as_str().unwrap_or(folder);
    if !safe_name(name) {
        bail!("This marketplace's name isn't valid.");
    }
    let publisher = ["owner", "author"]
        .iter()
        .find_map(|key| value[key]["name"].as_str().or_else(|| value[key].as_str()))
        .unwrap_or_default();
    Ok(SourcePreview {
        name: name.into(),
        publisher: publisher.into(),
        plugins,
        source: source.into(),
    })
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
/// The commit a remote reference points at, without downloading anything.
pub(super) async fn git_remote_ref(url: &str, reference: Option<&str>) -> anyhow::Result<String> {
    let reference = reference.unwrap_or("HEAD");
    if reference.starts_with('-') || reference.chars().any(char::is_control) {
        bail!("Invalid repository reference");
    }
    let mut command = tokio::process::Command::new("git");
    command
        .args([
            "-c",
            "core.hooksPath=/dev/null",
            "ls-remote",
            "--",
            url,
            reference,
        ])
        .env_clear();
    for name in ["PATH", "HOME", "SYSTEMROOT"] {
        if let Some(value) = std::env::var_os(name) {
            command.env(name, value);
        }
    }
    let output = tokio::time::timeout(
        std::time::Duration::from_secs(30),
        command
            .env("GIT_TERMINAL_PROMPT", "0")
            .kill_on_drop(true)
            .stderr(std::process::Stdio::null())
            .output(),
    )
    .await
    .context("Repository check timed out")??;
    if !output.status.success() {
        bail!("Repository check failed");
    }
    Ok(String::from_utf8_lossy(&output.stdout)
        .split_whitespace()
        .next()
        .unwrap_or_default()
        .to_owned())
}
/// A bounded hash of a folder's files: names, sizes and contents, not modification times. Past
/// the bound only names and sizes count.
pub(super) fn tree_fingerprint(root: &Path) -> anyhow::Result<String> {
    use sha2::{Digest, Sha256};
    const FILES: usize = 5000;
    const CONTENT_BYTES: u64 = 64 * 1024 * 1024;
    let mut hash = Sha256::new();
    let mut stack = vec![root.to_path_buf()];
    let (mut files, mut read) = (0, 0u64);
    while let Some(directory) = stack.pop() {
        let mut entries = fs::read_dir(&directory)?.collect::<Result<Vec<_>, _>>()?;
        entries.sort_by_key(|entry| entry.file_name());
        for entry in entries {
            if entry.file_name() == ".git" {
                continue;
            }
            let path = entry.path();
            let metadata = fs::symlink_metadata(&path)?;
            if metadata.is_dir() {
                stack.push(path);
                continue;
            }
            files += 1;
            if files > FILES {
                return Ok(format!("{:x}", hash.finalize()));
            }
            hash.update(path.strip_prefix(root)?.to_string_lossy().as_bytes());
            hash.update(metadata.len().to_le_bytes());
            if metadata.is_symlink() {
                hash.update(fs::read_link(&path)?.to_string_lossy().as_bytes());
            } else if read + metadata.len() <= CONTENT_BYTES {
                read += metadata.len();
                hash.update(fs::read(&path)?);
            }
        }
    }
    Ok(format!("{:x}", hash.finalize()))
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

/// Imports a skill or plugin folder. `entry` is the marketplace entry, whose `skills`,
/// `mcpServers` and `hooks` declarations count like the plugin manifest's.
fn import_directory(
    root: &Path,
    allow_partial: bool,
    entry: Option<&Value>,
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
    if let Err(error) = collect_plugin_files(root, &mut spec, None) {
        if error.to_string() != RESOURCE_LIMITS {
            return Err(error);
        }
        // Many catalog entries point at a whole repository (a library, an MCP server's
        // source). Keep only the plugin's own folders and the files its declarations name.
        let keep = component_paths(&manifest, entry);
        spec.files.clear();
        spec.executable_files.clear();
        spec.unsupported.clear();
        spec.notices.clear();
        collect_plugin_files(root, &mut spec, Some(keep))?;
        spec.unsupported
            .push("Large repository: only the plugin's own folders were imported".into());
    }
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
    let mut mcp_files = vec![".mcp.json".to_owned()];
    let mut hook_files = vec!["hooks/hooks.json".to_owned()];
    for value in declared("mcpServers") {
        if value.is_object() {
            import_mcp_components(value, false, &mut spec, allow_partial)?;
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
            import_hooks(value, &mut spec)?;
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
            import_mcp_components(&servers, false, &mut spec, allow_partial)?;
        }
    }
    for path in unique(hook_files) {
        if let Some(raw) = spec.files.get(&path) {
            import_hooks(&serde_json::from_str::<Value>(raw)?, &mut spec)?;
        }
    }
    for field in [
        "agents",
        "channels",
        "defaultEnabled",
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
    for (directory, label) in [
        ("agents/", "Claude subagents"),
        ("bin/", "Plugin PATH executables"),
        ("output-styles/", "Output styles"),
    ] {
        if spec.files.keys().any(|path| path.starts_with(directory)) {
            spec.unsupported.push(label.into());
        }
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
    let declared = ["skills", "mcpServers", "hooks"]
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
pub(super) fn collect_files(
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
    };
    walk.directory(directory, "")
}

/// Lenient walk used for plugin packages: links that stay inside the plugin are followed,
/// oversized and binary files are left out with a notice, and other links are reported.
fn collect_plugin_files(
    root: &Path,
    spec: &mut PluginSpec,
    keep: Option<Vec<String>>,
) -> anyhow::Result<()> {
    let root = root.canonicalize()?;
    let mut walk = FileWalk {
        root: &root,
        files: &mut spec.files,
        executable_files: &mut spec.executable_files,
        unsupported: &mut spec.unsupported,
        notices: Some(&mut spec.notices),
        stack: vec![root.clone()],
        keep,
    };
    walk.directory(&root, "")
}

const MAX_PLUGIN_FILE_BYTES: u64 = 1024 * 1024;
const RESOURCE_LIMITS: &str = "Plugin exceeds resource limits";
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
        if metadata.len() > MAX_PLUGIN_FILE_BYTES {
            match self.notices.as_deref_mut() {
                Some(notices) => {
                    notices.push(format!("File too large: {relative}"));
                    return Ok(());
                }
                None => bail!("Plugin file exceeds 1 MiB"),
            }
        }
        let bytes = fs::read(path)?;
        let Ok(text) = String::from_utf8(bytes) else {
            let note = format!("Binary resource omitted: {relative}");
            match self.notices.as_deref_mut() {
                Some(notices) => notices.push(note),
                None => self.unsupported.push(note),
            }
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
        if self.files.len() > 1000
            || self.files.values().map(String::len).sum::<usize>() > 8 * 1024 * 1024
        {
            bail!(RESOURCE_LIMITS);
        }
        Ok(())
    }
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
                for (field, note) in [
                    (
                        "timeout",
                        "its timeout is not used; Themis's own timeouts apply",
                    ),
                    (
                        "autoApprove",
                        "its automatic approvals are not used; Themis approvals apply",
                    ),
                ] {
                    if raw.get(field).is_some() {
                        spec.notices.push(format!("MCP connection {name}: {note}"));
                    }
                }
                // The same server is often declared both in plugin.json and in .mcp.json.
                let same = |existing: &McpServer| {
                    serde_json::to_value(existing).ok() == serde_json::to_value(&server).ok()
                };
                if spec.mcp.get(name).is_some_and(same) {
                    continue;
                }
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
const LITERAL_REFUSED: &str = "Imported credentials/environment values must use ${VARIABLE} or {env:VARIABLE}; literal values are unsupported";

/// A value that must name a key (an OAuth secret, a bearer token): no default allowed.
fn environment_reference(value: &str) -> anyhow::Result<String> {
    match crate::plugins::keys::reference(value) {
        Some((name, None)) => Ok(name),
        Some((_, Some(_))) => bail!("Unsupported environment reference"),
        None if value.contains("${") || value.contains("{env:") => {
            bail!("Unsupported environment reference")
        }
        None => bail!(LITERAL_REFUSED),
    }
}
/// `env` entries: references become keys (with any default), plain non-secret values and
/// plugin-folder paths are kept as written, and anything else is refused.
fn import_env(server: &mut McpServer, env: &Value) -> anyhow::Result<()> {
    for (key, value) in env
        .as_object()
        .context("MCP environment must be an object")?
    {
        let value = value
            .as_str()
            .context("MCP environment must contain strings")?;
        let names_plugin_root = crate::plugins::keys::PLUGIN_ROOTS
            .iter()
            .any(|root| value.contains(root));
        match crate::plugins::keys::reference(value).filter(|_| !names_plugin_root) {
            Some((name, default)) => {
                add_defaults(
                    server,
                    default.map(|d| (name.clone(), d)).into_iter().collect(),
                )?;
                server.env.insert(key.clone(), name);
            }
            None if crate::plugins::keys::safe_plugin_value(key, value) => {
                server.env_values.insert(key.clone(), value.to_owned());
            }
            None => {
                environment_reference(value)?;
            }
        }
    }
    Ok(())
}

/// Headers: plain non-secret values are kept, `Authorization: Bearer ${NAME}` becomes the
/// bearer key, and every other value must be a `prefix${NAME}` template.
fn import_headers(server: &mut McpServer, headers: &Value) -> anyhow::Result<()> {
    for (key, value) in headers
        .as_object()
        .context("MCP headers must be an object")?
    {
        let value = value
            .as_str()
            .context("MCP header must reference a key as ${NAME}")?;
        if !value.contains("${") && !value.contains("{env:") {
            if !crate::plugins::keys::safe_literal(key, value) {
                crate::plugins::keys::header_template(value)?;
            }
            server.header_values.insert(key.clone(), value.to_owned());
            continue;
        }
        // `{env:NAME}` (OpenCode) and `${env:NAME}` read the same as `${NAME}`.
        let (template, defaults) = crate::plugins::keys::normalize_references(value)
            .context("MCP header values must be an optional prefix followed by one ${NAME}")?;
        add_defaults(server, defaults)?;
        let bearer = template
            .strip_prefix("Bearer ")
            .filter(|token| token.starts_with("${"));
        match bearer {
            Some(token)
                if key.eq_ignore_ascii_case("authorization") && server.bearer_env.is_none() =>
            {
                server.bearer_env = Some(environment_reference(token)?);
            }
            _ => {
                crate::plugins::keys::header_template(&template)?;
                server.headers.insert(key.clone(), template);
            }
        }
    }
    Ok(())
}
/// Defaults from `${NAME:-value}`: plain, non-secret values only.
fn add_defaults(server: &mut McpServer, defaults: Vec<(String, String)>) -> anyhow::Result<()> {
    for (name, default) in defaults {
        if !crate::plugins::keys::safe_literal(&name, &default) {
            bail!(LITERAL_REFUSED);
        }
        server.defaults.insert(name, default);
    }
    Ok(())
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
            // Accepted without effect: descriptions and notes, Claude's own switch (imported
            // servers start disabled), its timeout and auto-approval (Themis's apply).
            "description",
            "note",
            "_note",
            "_comment",
            "_meta",
            "disabled",
            "timeout",
            "autoApprove",
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
        // Sign-in happens in Integrations after installing (MCP OAuth with dynamic client
        // registration); preset OAuth client settings are not used.
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
        let url = url.as_str().context("MCP URL must be a string")?;
        let (url, defaults) = crate::plugins::keys::normalize_references(url)
            .context("MCP URL references must be ${NAME}")?;
        server.url = Some(url);
        add_defaults(&mut server, defaults)?;
    }
    if let Some(env) = raw.get(if opencode { "environment" } else { "env" }) {
        import_env(&mut server, env)?;
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
        import_headers(&mut server, headers)?;
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
/// One connection from the standard `mcpServers` shape used by Claude Desktop, Claude Code
/// and Cursor: `{command, args, env}` for local servers, `{type: "http", url, headers}` for
/// remote ones. Credentials must be `${VARIABLE}` references, as for any import.
pub fn mcp_server_from_standard(name: &str, raw: &Value) -> anyhow::Result<McpServer> {
    import_mcp_server(name, raw, false)
}

/// A connection in the standard `mcpServers` shape (the reverse of
/// [`mcp_server_from_standard`]); whether it is enabled is left out.
pub fn mcp_server_to_standard(server: &McpServer) -> Value {
    let mut entry = serde_json::Map::new();
    if let Some(command) = &server.command {
        entry.insert("command".into(), json!(command));
        if !server.args.is_empty() {
            entry.insert("args".into(), json!(server.args));
        }
    }
    if let Some(url) = &server.url {
        entry.insert("type".into(), json!("http"));
        entry.insert("url".into(), json!(url));
        let mut headers: serde_json::Map<String, Value> = server
            .headers
            .iter()
            .map(|(name, template)| (name.clone(), json!(template)))
            .collect();
        if let Some(bearer) = &server.bearer_env {
            headers.insert(
                "Authorization".into(),
                json!(format!("Bearer ${{{bearer}}}")),
            );
        }
        headers.extend(
            server
                .header_values
                .iter()
                .map(|(name, value)| (name.clone(), json!(value))),
        );
        if !headers.is_empty() {
            entry.insert("headers".into(), Value::Object(headers));
        }
    }
    if !server.env.is_empty() || !server.env_values.is_empty() {
        let env: serde_json::Map<String, Value> = server
            .env
            .iter()
            .map(|(key, variable)| {
                let value = match server.defaults.get(variable) {
                    Some(default) => format!("${{{variable}:-{default}}}"),
                    None => format!("${{{variable}}}"),
                };
                (key.clone(), json!(value))
            })
            .chain(
                server
                    .env_values
                    .iter()
                    .map(|(key, value)| (key.clone(), json!(value))),
            )
            .collect();
        entry.insert("env".into(), Value::Object(env));
    }
    Value::Object(entry)
}

/// Themis tools for a Claude tool name in a hook matcher.
fn themis_tools(claude: &str) -> Option<&'static [&'static str]> {
    Some(match claude {
        "Bash" => &["shell", "background_task"],
        "Read" => &["read_file"],
        "Write" => &["write_file"],
        "Edit" | "MultiEdit" => &["apply_patch", "write_file"],
        "Grep" => &["search_file"],
        "Glob" | "LS" => &["list_dir", "search_file"],
        "WebFetch" => &["fetch_url"],
        "WebSearch" => &["web_search"],
        _ => return None,
    })
}

/// A Claude tool matcher ("Edit|Write") as Themis tool names, and the names with no Themis
/// tool. Themis names already in it are kept.
fn map_tool_matcher(matcher: &str) -> (Vec<String>, Vec<String>) {
    let mut mapped: Vec<String> = Vec::new();
    let mut unmapped = Vec::new();
    // MCP tools are named differently in Themis, and a regex group is not a list of names:
    // either is one matcher with no Themis tool.
    if matcher.contains("mcp__") {
        return (mapped, vec!["for MCP tools".to_owned()]);
    }
    if matcher.contains(['(', ')', '[', '*', '.', '\\']) {
        return (mapped, vec![matcher.to_owned()]);
    }
    for name in matcher
        .split('|')
        .map(str::trim)
        .filter(|name| !name.is_empty())
    {
        let tools: Vec<&str> = match themis_tools(name) {
            Some(tools) => tools.to_vec(),
            None if crate::skills::known_tool_names()
                .iter()
                .any(|known| known == name) =>
            {
                vec![name]
            }
            None => {
                unmapped.push(name.to_owned());
                continue;
            }
        };
        for tool in tools {
            if !mapped.iter().any(|known| known == tool) {
                mapped.push(tool.to_owned());
            }
        }
    }
    (mapped, unmapped)
}

fn import_hooks(value: &Value, spec: &mut PluginSpec) -> anyhow::Result<()> {
    let events = value
        .get("hooks")
        .unwrap_or(value)
        .as_object()
        .context("Invalid hook configuration")?;
    for (event, groups) in events {
        let mapped = match event.as_str() {
            // Its output is context for the model, which only PromptSubmit delivers: it runs
            // on every turn instead of once per session.
            "SessionStart" => "PromptSubmit",
            "PreToolUse" => "BeforeTool",
            "PostToolUse" => "AfterTool",
            "Stop" => "RunEnd",
            "PreCompact" => "BeforeCompaction",
            "UserPromptSubmit" => "PromptSubmit",
            // Fires only when the tool call failed.
            "PostToolUseFailure" => "ToolCallFailed",
            _ => {
                spec.unsupported.push(format!("Hook event {event}"));
                continue;
            }
        };
        let tool_event = matches!(
            event.as_str(),
            "PreToolUse" | "PostToolUse" | "PostToolUseFailure"
        );
        for group in groups.as_array().context("Invalid hook groups")? {
            for raw in group["hooks"].as_array().context("Invalid hook list")? {
                if raw["type"] != "command" {
                    spec.unsupported.push(format!("{event}: non-command hook"));
                    continue;
                }
                // Every Claude session starts with "startup"; the other sources have no Themis
                // equivalent.
                let session_matcher = (event == "SessionStart")
                    .then(|| group["matcher"].as_str())
                    .flatten()
                    .filter(|matcher| !matcher.is_empty() && *matcher != "*");
                if let Some(matcher) = session_matcher {
                    if !matcher.split('|').any(|source| source.trim() == "startup") {
                        spec.unsupported.push(format!(
                            "SessionStart: matcher {matcher} has no Themis equivalent"
                        ));
                        continue;
                    }
                }
                if !tool_event
                    && event != "SessionStart"
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
                // Claude names its tools differently: a `Bash` guard must guard `shell`.
                let matcher = match group["matcher"]
                    .as_str()
                    .filter(|value| !value.is_empty() && !matches!(*value, "*" | ".*"))
                {
                    Some(matcher) if tool_event => {
                        let (mapped_tools, unmapped) = map_tool_matcher(matcher);
                        for name in &unmapped {
                            spec.unsupported
                                .push(format!("{event}: matcher {name} has no Themis tool"));
                        }
                        if mapped_tools.is_empty() {
                            continue;
                        }
                        Some(mapped_tools.join("|"))
                    }
                    _ if event == "SessionStart" => None,
                    other => other.map(str::to_owned),
                };
                let command = raw["command"].as_str().context("Missing hook command")?;
                spec.hooks.push(Hook {
                    name: imported_hook_name(mapped, matcher.as_deref(), command, &spec.hooks),
                    event: mapped.into(),
                    command: command.into(),
                    enabled: false,
                    matcher,
                    failure_policy: None,
                    timeout_seconds: raw["timeout"].as_u64().unwrap_or(10).clamp(1, 60),
                    // A broken session hook must not stop every turn.
                    blocking: event == "PreToolUse",
                    plugin_root: None,
                    runtime_identity: None,
                });
            }
        }
    }
    Ok(())
}

/// A name for an imported hook, which has none of its own: the script it runs, or else when it
/// runs (`before-shell`), made unique within the plugin.
fn imported_hook_name(event: &str, matcher: Option<&str>, command: &str, taken: &[Hook]) -> String {
    const SCRIPTS: [&str; 9] = ["sh", "bash", "py", "js", "mjs", "cjs", "ts", "rb", "pl"];
    const GENERIC: [&str; 5] = ["index", "main", "run", "hook", "hooks"];
    let script = command
        .split_whitespace()
        .map(|word| word.trim_matches(|c| matches!(c, '"' | '\'' | ';')))
        .filter_map(|word| word.rsplit(['/', '\\']).next()?.rsplit_once('.'))
        .find(|(_, extension)| SCRIPTS.contains(extension))
        .map(|(stem, _)| {
            stem.to_ascii_lowercase()
                .split(|c: char| !c.is_ascii_alphanumeric())
                .filter(|part| !part.is_empty())
                .collect::<Vec<_>>()
                .join("-")
        })
        .filter(|stem| !stem.is_empty() && !GENERIC.contains(&stem.as_str()));
    let tool = matcher.and_then(|matcher| matcher.split('|').next());
    let base = script.unwrap_or_else(|| match (event, tool) {
        ("PromptSubmit", _) => "prompt-submit".into(),
        ("RunStart", _) => "run-start".into(),
        ("BeforeCompaction", _) => "before-compaction".into(),
        ("RunEnd", _) => "run-end".into(),
        ("BeforeTool", Some(tool)) => format!("before-{tool}").replace('_', "-"),
        ("AfterTool", Some(tool)) => format!("after-{tool}").replace('_', "-"),
        ("ToolCallFailed", Some(tool)) => format!("{tool}-failed").replace('_', "-"),
        ("BeforeTool", None) => "before-tool".into(),
        ("AfterTool", None) => "after-tool".into(),
        _ => "tool-failed".into(),
    });
    let mut name = base.clone();
    let mut number = 1;
    while taken.iter().any(|hook| hook.name == name) {
        number += 1;
        name = format!("{base}-{number}");
    }
    name
}

#[cfg(test)]
mod source_tests {
    use super::*;
    use proptest::prelude::*;

    #[test]
    fn typed_sources_normalize_to_stored_ones() {
        assert_eq!(
            normalize_source(" acme/agent-plugins ").unwrap(),
            "https://github.com/acme/agent-plugins.git"
        );
        assert_eq!(
            normalize_source("https://example.com/a/b.git").unwrap(),
            "https://example.com/a/b.git"
        );
        for bad in [
            "",
            "plugins",
            "../up/here",
            "a/b/c",
            "http://example.com/a.git",
        ] {
            assert!(normalize_source(bad).is_err(), "{bad}");
        }
    }

    proptest! {
        /// Whatever is typed, what gets stored is an HTTPS URL without credentials, or an
        /// absolute path. Nothing relative, and nothing that git could take as an option.
        #[test]
        fn stored_sources_are_https_or_absolute(input in "\\PC{0,40}") {
            if let Ok(source) = normalize_source(&input) {
                prop_assert!(source.starts_with("https://") || Path::new(&source).is_absolute());
                prop_assert!(!source.starts_with('-'));
                if source.starts_with("https://") {
                    let url = reqwest::Url::parse(&source).unwrap();
                    prop_assert!(url.username().is_empty() && url.password().is_none());
                }
            }
        }
    }

    /// Only the manifest is fetched, however large the repository.
    #[cfg(unix)]
    #[tokio::test]
    async fn a_git_source_fetches_only_its_manifest() {
        let repository = tempfile::tempdir().unwrap();
        let root = repository.path();
        fs::create_dir_all(root.join(".claude-plugin")).unwrap();
        fs::write(
            root.join(".claude-plugin/marketplace.json"),
            r#"{"name":"acme","plugins":[{"name":"one"}]}"#,
        )
        .unwrap();
        fs::write(root.join("huge.bin"), "bulk").unwrap();
        for args in [
            vec!["init", "-q"],
            vec!["add", "."],
            vec![
                "-c",
                "user.name=t",
                "-c",
                "user.email=t@t",
                "commit",
                "-qm",
                "x",
            ],
        ] {
            assert!(std::process::Command::new("git")
                .args(&args)
                .current_dir(root)
                .status()
                .unwrap()
                .success());
        }
        let out = tempfile::tempdir().unwrap();
        let staging = out.path().join("preview");
        let url = format!("file://{}", root.display());
        let directory = read_git_manifest(&url, &staging).await.unwrap();
        assert!(!directory.join("huge.bin").exists());
        let preview = source_preview(&url, &directory).unwrap();
        assert_eq!((preview.name.as_str(), preview.plugins), ("acme", 1));
        let missing = read_git_manifest(&format!("{url}-missing"), &out.path().join("other"))
            .await
            .unwrap_err();
        assert_eq!(missing.to_string(), "This source can't be reached.");
    }
}

#[cfg(test)]
mod imported_hook_name_tests {
    use super::*;

    fn named(names: &[&str]) -> Vec<Hook> {
        names
            .iter()
            .map(|name| Hook {
                name: (*name).into(),
                event: "RunEnd".into(),
                command: "true".into(),
                enabled: false,
                matcher: None,
                failure_policy: None,
                timeout_seconds: 10,
                blocking: false,
                plugin_root: None,
                runtime_identity: None,
            })
            .collect()
    }

    #[test]
    fn names_come_from_the_script_or_else_from_when_the_hook_runs() {
        let name = |event, matcher, command| imported_hook_name(event, matcher, command, &[]);
        assert_eq!(
            name(
                "BeforeTool",
                Some("shell"),
                r#"python3 "${CLAUDE_PLUGIN_ROOT}/hooks/Check_Secrets.py""#
            ),
            "check-secrets"
        );
        assert_eq!(
            name(
                "BeforeTool",
                Some("shell|background_task"),
                "if grep -q x; then echo y; fi"
            ),
            "before-shell"
        );
        assert_eq!(
            name("AfterTool", Some("web_search"), "true"),
            "after-web-search"
        );
        assert_eq!(name("ToolCallFailed", None, "true"), "tool-failed");
        assert_eq!(
            name("PromptSubmit", None, "echo 'see notes/today.txt'"),
            "prompt-submit"
        );
        // A generic script name says nothing, so the event names the hook.
        assert_eq!(
            name("RunEnd", None, "node ${CLAUDE_PLUGIN_ROOT}/hooks/index.js"),
            "run-end"
        );
    }

    #[test]
    fn names_stay_unique_and_valid_within_a_plugin() {
        let taken = named(&["run-end", "run-end-2"]);
        let name = imported_hook_name("RunEnd", None, "true", &taken);
        assert_eq!(name, "run-end-3");
        assert!(crate::plugins::safe_name(&name));
        assert!(crate::plugins::safe_name(&imported_hook_name(
            "BeforeTool",
            None,
            "./a--b.sh",
            &[]
        )));
    }
}
