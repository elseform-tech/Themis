use super::connections::McpOauth;
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
    for field in [
        "package_hash",
        "disabled_skills",
        "origin",
        "icon_asset",
        "brand",
    ] {
        value[field] = Value::Null;
    }
    format!("{:x}", Sha256::digest(value.to_string()))
}
/// Most bytes kept of a plugin's own icon.
const ICON_LIMIT: usize = 256 * 1024;
/// Whether an icon named by a manifest can be used: HTTPS, or a bundled SVG, PNG or WebP.
fn usable_icon(icon: &str, files: &BTreeMap<String, String>, root: &Path) -> bool {
    icon.starts_with("https://")
        || (icon.ends_with(".svg") && files.contains_key(icon))
        || ((icon.ends_with(".png") || icon.ends_with(".webp"))
            && safe_relative(icon)
            && root.join(icon).is_file())
}
/// The file name an icon is stored under, or `None` for anything but an SVG, PNG or WebP.
pub(super) fn icon_asset_name(bytes: &[u8]) -> Option<String> {
    use sha2::{Digest, Sha256};
    if bytes.is_empty() || bytes.len() > ICON_LIMIT {
        return None;
    }
    let text = std::str::from_utf8(bytes).unwrap_or_default().trim_start();
    let extension = if bytes.starts_with(b"\x89PNG\r\n\x1a\n") {
        "png"
    } else if bytes.len() > 12 && &bytes[..4] == b"RIFF" && &bytes[8..12] == b"WEBP" {
        "webp"
    } else if text.starts_with("<svg") || (text.starts_with("<?xml") && text.contains("<svg")) {
        "svg"
    } else {
        return None;
    };
    Some(format!("{:x}.{extension}", Sha256::digest(bytes)))
}
/// An HTTPS icon, bounded in size and time. Only ever called during a compatibility check, so
/// browsing makes no network requests.
async fn download_icon(url: &str) -> anyhow::Result<Vec<u8>> {
    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(10))
        .redirect(reqwest::redirect::Policy::custom(|attempt| {
            if attempt.url().scheme() == "https" && attempt.previous().len() < 3 {
                attempt.follow()
            } else {
                attempt.stop()
            }
        }))
        .build()?;
    let mut response = client.get(url).send().await?.error_for_status()?;
    let mut bytes = Vec::new();
    while let Some(chunk) = response.chunk().await? {
        bytes.extend_from_slice(&chunk);
        if bytes.len() > ICON_LIMIT {
            bail!("Icon is too large");
        }
    }
    Ok(bytes)
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
            (
                "subagent",
                spec.subagents.iter().map(|p| p.name.clone()).collect(),
            ),
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
        let status = if !spec.has_capabilities() {
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
    fn has_capabilities(&self) -> bool {
        !(self.skills.is_empty()
            && self.mcp.is_empty()
            && self.hooks.is_empty()
            && self.subagents.is_empty())
    }
    /// Installation must add a capability; empty personal drafts may still be saved.
    pub fn ensure_installable(&self) -> anyhow::Result<()> {
        if !self.has_capabilities() {
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
        let store = self.directory(scope)?.join(super::package::PACKAGES_DIR);
        let spec = self.path_spec(path, name, allow_partial, Some(&store))?;
        spec.ensure_installable()?;
        self.save(scope, spec, None)
    }
    /// Inspect compatibility without registering or executing plugin capabilities.
    pub fn inspect_path(&self, path: &Path, name: Option<&str>) -> anyhow::Result<ImportPreview> {
        Ok(ImportPreview::new(self.path_spec(path, name, true, None)?))
    }
    fn path_spec(
        &self,
        path: &Path,
        name: Option<&str>,
        allow_partial: bool,
        keep_in: Option<&Path>,
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
                keep_in,
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
        let store = self.directory(scope)?.join(super::package::PACKAGES_DIR);
        let spec = self
            .repository_spec(
                name,
                source,
                reference,
                subdirectory,
                allow_partial,
                Some(&store),
            )
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
            self.repository_spec(name, source, reference, subdirectory, true, None)
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
        self.repository_spec(name, source, reference, subdirectory, false, None)
            .await
    }
    async fn repository_spec(
        &self,
        name: &str,
        source: &str,
        reference: Option<&str>,
        subdirectory: Option<&str>,
        allow_partial: bool,
        keep_in: Option<&Path>,
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
            let mut spec = import_directory(&directory, allow_partial, None, keep_in)?;
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
    /// Saves a plugin's own icon in the icon store and returns its name. An icon that can't be
    /// used is left out: the plugin then gets a brand logo or a tile.
    async fn save_icon(&self, icon: Option<&str>, directory: &Path) -> Option<String> {
        let icon = icon?;
        let bytes = if icon.starts_with("https://") {
            download_icon(icon).await.ok()?
        } else {
            let path = directory.join(icon).canonicalize().ok()?;
            if !path.starts_with(directory.canonicalize().ok()?)
                || fs::metadata(&path).ok()?.len() > ICON_LIMIT as u64
            {
                return None;
            }
            fs::read(path).ok()?
        };
        let name = icon_asset_name(&bytes)?;
        let path = self.global.join("plugin-icons").join(&name);
        fs::create_dir_all(path.parent()?).ok()?;
        if !path.exists() {
            write_bytes(&path, &bytes, false).ok()?;
        }
        Some(name)
    }
    /// A stored icon as a data URL, for the UI to show without any network request.
    pub fn plugin_icon(&self, name: &str) -> anyhow::Result<String> {
        use base64::Engine;
        let (hash, extension) = name.split_once('.').context("Invalid icon name")?;
        let mime = match extension {
            "svg" => "image/svg+xml",
            "png" => "image/png",
            "webp" => "image/webp",
            _ => bail!("Invalid icon name"),
        };
        if hash.len() != 64 || !hash.bytes().all(|byte| byte.is_ascii_hexdigit()) {
            bail!("Invalid icon name");
        }
        let bytes = fs::read(self.global.join("plugin-icons").join(name))?;
        let encoded = base64::engine::general_purpose::STANDARD.encode(bytes);
        Ok(format!("data:{mime};base64,{encoded}"))
    }
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
        let store = self.directory(scope)?.join(super::package::PACKAGES_DIR);
        let spec = self
            .marketplace_spec(marketplace, name, refresh, allow_partial, Some(&store))
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
                enabled: previous.map_or(!spec.starts_disabled, |p| p.enabled),
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
        self.marketplace_spec(marketplace, name, false, false, None)
            .await
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
            self.marketplace_spec(marketplace, name, false, true, None)
                .await?,
        ))
    }
    /// Downloads a checked package's files again to keep them, since a check only hashes them.
    /// A package that has changed since it was checked isn't what was reviewed, so it stops.
    pub(super) async fn keep_checked_package(
        &self,
        scope: &str,
        marketplace: &str,
        name: &str,
        wanted: &super::package::PackageRef,
    ) -> anyhow::Result<()> {
        let store = self.directory(scope)?.join(super::package::PACKAGES_DIR);
        if super::package::verified(&store, wanted)?.is_some() {
            return Ok(());
        }
        let spec = self
            .marketplace_spec(marketplace, name, false, true, Some(&store))
            .await?;
        if spec.package.as_ref() != Some(wanted) {
            if let Some(found) = spec.package {
                let _ = fs::remove_dir_all(store.join(found.hash));
            }
            bail!("This plugin changed after it was checked; check it again before installing");
        }
        Ok(())
    }
    async fn marketplace_spec(
        &self,
        marketplace: &str,
        name: &str,
        refresh: bool,
        allow_partial: bool,
        keep_in: Option<&Path>,
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
            let mut spec = import_directory(&directory, allow_partial, Some(entry), keep_in)?;
            spec.name = name.into();
            spec.package_kind = Some("plugin".into());
            for field in ["lspServers", "dependencies", "settings", "channels"] {
                if entry.get(field).is_some() {
                    spec.unsupported.push(format!(
                        "Marketplace entry {field}: component declarations require plugin manifest"
                    ));
                }
            }
            if let Some(icon) = entry.get("icon").and_then(Value::as_str) {
                let icon = icon.strip_prefix("./").unwrap_or(icon);
                if usable_icon(icon, &spec.files, &directory) {
                    spec.icon = Some(icon.into());
                } else {
                    spec.unsupported
                        .push("Marketplace icon resource is unavailable or unsupported".into());
                }
            }
            spec.brand = super::scans::catalog_brand(marketplace, entry);
            spec.icon_asset = self.save_icon(spec.icon.as_deref(), &directory).await;
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
    let limit = super::package::MAX_CLONE_BYTES;
    run_git_within(
        &[
            "clone",
            "--depth=1",
            "--",
            source,
            path.to_str().context("Invalid cache path")?,
        ],
        None,
        Some((path, limit)),
    )
    .await?;
    if let Some(reference) = reference {
        let fetch = ["fetch", "--depth=1", "origin", reference];
        run_git_within(&fetch, Some(path), Some((path, limit))).await?;
        let checkout = ["checkout", "--detach", "FETCH_HEAD"];
        run_git_within(&checkout, Some(path), Some((path, limit))).await?;
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
    run_git_within(args, directory, None).await
}
/// How long one git command may take: a download of a large repository on a slow line.
const GIT_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(180);
/// How often a download's size is looked at.
const CLONE_WATCH: std::time::Duration = std::time::Duration::from_secs(2);
/// Runs git, stopping it when the folder in `watch` grows past its limit: a repository can't
/// fill the disk before the time limit is up.
async fn run_git_within(
    args: &[&str],
    directory: Option<&Path>,
    watch: Option<(&Path, u64)>,
) -> anyhow::Result<()> {
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
    let too_large = || watch.is_some_and(|(path, limit)| super::package::tree_bytes(path) > limit);
    let mut child = command.spawn()?;
    let status = tokio::time::timeout(GIT_TIMEOUT, async {
        loop {
            tokio::select! {
                status = child.wait() => return status.map_err(anyhow::Error::from),
                () = tokio::time::sleep(CLONE_WATCH) => {
                    if too_large() {
                        let _ = child.kill().await;
                        bail!(REPOSITORY_TOO_LARGE);
                    }
                }
            }
        }
    })
    .await
    .context("Repository download timed out")??;
    if !status.success() {
        bail!("Repository download failed; check repository access, reference, and network");
    }
    // One that finished between two looks.
    if too_large() {
        bail!(REPOSITORY_TOO_LARGE);
    }
    Ok(())
}
const REPOSITORY_TOO_LARGE: &str = "Repository is too large to download (over 2 GiB)";

/// Imports a skill or plugin folder. `entry` is the marketplace entry, whose `skills`,
/// `mcpServers` and `hooks` declarations count like the plugin manifest's.
fn import_directory(
    root: &Path,
    allow_partial: bool,
    entry: Option<&Value>,
    keep_in: Option<&Path>,
) -> anyhow::Result<PluginSpec> {
    import_directory_within(root, allow_partial, entry, keep_in, super::package::LIMITS)
}

/// [`import_directory`] with the bounds given. `keep_in` is where a package too big for the
/// registry is kept; without it, as in a check, it is only hashed.
fn import_directory_within(
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
    import_mcp_components(
        servers,
        opencode,
        &mut spec,
        allow_partial,
        &mut LiteralSecrets::refused(),
    )?;
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
    literals: &mut LiteralSecrets,
) -> anyhow::Result<()> {
    let root = if opencode { "mcp" } else { "mcpServers" };
    for (name, raw) in value.as_object().context("Invalid MCP configuration")? {
        match import_mcp_server(name, raw, opencode, literals) {
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

/// One literal value a plugin's configuration held and a secret now stands for.
#[derive(Debug, Clone)]
struct ReplacedLiteral {
    /// The field it was written in; `None` when it was a `:-default` inside a reference.
    field: Option<String>,
    literal: String,
    /// What takes its place in the plugin's files.
    replacement: String,
    secret: String,
}

/// What an import does with a literal value in a credential field. For a plugin it becomes a named
/// secret, the person fills that in (Secret needed, Add secret), and the literal is discarded: not
/// kept in the registry, its files, logs or the check. A configuration the person pastes or types
/// is refused instead, since saying so at once is more use than dropping what they just wrote.
#[derive(Debug, Default)]
struct LiteralSecrets {
    plugin: Option<String>,
    replaced: Vec<ReplacedLiteral>,
    /// Secrets named so far with the literal each stands for, so two values never share one.
    named: BTreeMap<String, String>,
}

impl LiteralSecrets {
    fn refused() -> Self {
        Self::default()
    }

    fn for_plugin(plugin: &str) -> Self {
        Self {
            plugin: Some(plugin.to_owned()),
            ..Self::default()
        }
    }

    /// The secret for `literal` in `field` of the server `server`.
    fn secret(&mut self, server: &str, field: &str, literal: &str) -> anyhow::Result<String> {
        use crate::plugins::keys::literal_secret_name;
        let Some(plugin) = self.plugin.clone() else {
            bail!(LITERAL_REFUSED);
        };
        let mut name = literal_secret_name(&plugin, field);
        if self.named.get(&name).is_some_and(|known| known != literal) {
            name = literal_secret_name(&plugin, &format!("{server}_{field}"));
        }
        let mut suffix = 2;
        let base = name.clone();
        while self.named.get(&name).is_some_and(|known| known != literal) {
            name = format!("{base}_{suffix}");
            suffix += 1;
        }
        self.named.insert(name.clone(), literal.to_owned());
        self.replaced.push(ReplacedLiteral {
            field: Some(field.to_owned()),
            literal: literal.to_owned(),
            replacement: format!("${{{name}}}"),
            secret: name.clone(),
        });
        Ok(name)
    }

    /// `${name:-literal}` with a default that may not be kept: the reference stays and the
    /// default goes. Refused outside a plugin.
    fn dropped_default(&mut self, name: &str, literal: &str) -> anyhow::Result<()> {
        if self.plugin.is_none() {
            bail!(LITERAL_REFUSED);
        }
        self.replaced.push(ReplacedLiteral {
            field: None,
            literal: format!(":-{literal}"),
            replacement: String::new(),
            secret: name.to_owned(),
        });
        Ok(())
    }
}

/// Whether a literal is worth hunting for through all of a plugin's text: a real value, not a
/// stand-in, and long enough not to be an everyday word.
fn real_literal(replaced: &ReplacedLiteral) -> bool {
    let bare = replaced.literal.trim_start_matches(":-");
    bare.len() >= 8
        && !crate::plugins::keys::placeholder(replaced.field.as_deref().unwrap_or_default(), bare)
}

/// `text`, a JSON configuration, with each replaced value rewritten; `None` when nothing in it is.
fn scrubbed_config(text: &str, replaced: &[ReplacedLiteral]) -> Option<String> {
    fn walk(
        value: &mut Value,
        key: Option<&str>,
        replaced: &[ReplacedLiteral],
        changed: &mut bool,
    ) {
        match value {
            Value::Object(map) => {
                for (name, inner) in map.iter_mut() {
                    walk(inner, Some(name), replaced, changed);
                }
            }
            Value::Array(items) => {
                for inner in items {
                    walk(inner, key, replaced, changed);
                }
            }
            Value::String(text) => {
                for item in replaced {
                    let here = item.field.is_none() || item.field.as_deref() == key;
                    if here && text.contains(&item.literal) {
                        *text = text.replace(&item.literal, &item.replacement);
                        *changed = true;
                    }
                }
            }
            _ => {}
        }
    }
    let mut value: Value = serde_json::from_str(text).ok()?;
    let mut changed = false;
    walk(&mut value, None, replaced, &mut changed);
    changed
        .then(|| serde_json::to_string_pretty(&value).ok())
        .flatten()
}

/// `text` as it may be kept: configuration fields rewritten, and a real value gone from anywhere.
fn scrubbed_text(relative: &str, text: &str, replaced: &[ReplacedLiteral]) -> Option<String> {
    let mut out = relative
        .ends_with(".json")
        .then(|| scrubbed_config(text, replaced))
        .flatten();
    for item in replaced.iter().filter(|item| real_literal(item)) {
        let current = out.as_deref().unwrap_or(text);
        if current.contains(&item.literal) {
            out = Some(current.replace(&item.literal, &item.replacement));
        }
    }
    out
}

/// Takes what a plugin's configuration held out of everything of it that will be kept: the
/// registry's copies, and the package's files (only those of a size that could hold configuration,
/// and text), rewritten in memory. The package's hash then describes what is kept.
fn scrub_literals(
    replaced: &[ReplacedLiteral],
    spec: &mut PluginSpec,
    entries: &mut [super::package::Entry],
) -> anyhow::Result<()> {
    if replaced.is_empty() {
        return Ok(());
    }
    for (relative, text) in &mut spec.files {
        if let Some(clean) = scrubbed_text(relative, text, replaced) {
            *text = clean;
        }
    }
    for entry in entries.iter_mut() {
        if entry.bytes > super::package::MAX_INLINE_FILE_BYTES {
            continue;
        }
        let Ok(text) = String::from_utf8(fs::read(&entry.path)?) else {
            continue;
        };
        if let Some(clean) = scrubbed_text(&entry.relative, &text, replaced) {
            entry.bytes = clean.len() as u64;
            entry.content = Some(clean.into_bytes());
        }
    }
    if !entries.is_empty() {
        spec.package = Some(super::package::describe(entries)?);
    }
    // Names only: what was written there is gone, and this says where to put the real value.
    let mut told = std::collections::BTreeSet::new();
    for item in replaced {
        if told.insert(item.secret.clone()) {
            spec.notices.push(format!(
                "A value written in this plugin's configuration is not kept; it reads the secret {} instead (Settings › Secrets)",
                item.secret
            ));
        }
    }
    Ok(())
}

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
/// plugin-folder paths are kept as written, and anything else is a literal (see [`LiteralSecrets`]).
fn import_env(
    name_of_server: &str,
    server: &mut McpServer,
    env: &Value,
    literals: &mut LiteralSecrets,
) -> anyhow::Result<()> {
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
                    literals,
                )?;
                server.env.insert(key.clone(), name);
            }
            None if crate::plugins::keys::safe_plugin_value(key, value)
                && !(literals.plugin.is_some()
                    && crate::plugins::keys::placeholder(key, value)) =>
            {
                server.env_values.insert(key.clone(), value.to_owned());
            }
            // A template that doesn't read as one reference says nothing to keep or replace.
            None if value.contains("${") || value.contains("{env:") => {
                environment_reference(value)?;
            }
            None => {
                let secret = literals.secret(name_of_server, key, value)?;
                server.env.insert(key.clone(), secret);
            }
        }
    }
    Ok(())
}

/// A literal header value's scheme word and its credential: `Bearer abc` splits, `abc` doesn't.
fn split_scheme(value: &str) -> (&str, &str) {
    ["Bearer ", "Token ", "Basic "]
        .iter()
        .find_map(|scheme| value.strip_prefix(scheme).map(|rest| (*scheme, rest)))
        .filter(|(_, rest)| !rest.trim().is_empty())
        .unwrap_or(("", value))
}

/// Headers: plain non-secret values are kept, `Authorization: Bearer ${NAME}` becomes the
/// bearer key, every other reference must be a `prefix${NAME}` template, and anything else is a
/// literal (see [`LiteralSecrets`]).
fn import_headers(
    name_of_server: &str,
    server: &mut McpServer,
    headers: &Value,
    literals: &mut LiteralSecrets,
) -> anyhow::Result<()> {
    for (key, value) in headers
        .as_object()
        .context("MCP headers must be an object")?
    {
        let value = value
            .as_str()
            .context("MCP header must reference a key as ${NAME}")?;
        if !value.contains("${") && !value.contains("{env:") {
            let stand_in =
                literals.plugin.is_some() && crate::plugins::keys::placeholder(key, value);
            if crate::plugins::keys::safe_literal(key, value) && !stand_in {
                server.header_values.insert(key.clone(), value.to_owned());
                continue;
            }
            if literals.plugin.is_none() {
                crate::plugins::keys::header_template(value)?;
            }
            let (scheme, credential) = split_scheme(value);
            let secret = literals.secret(name_of_server, key, credential)?;
            if key.eq_ignore_ascii_case("authorization")
                && scheme == "Bearer "
                && server.bearer_env.is_none()
            {
                server.bearer_env = Some(secret);
            } else {
                server
                    .headers
                    .insert(key.clone(), format!("{scheme}${{{secret}}}"));
            }
            continue;
        }
        // `{env:NAME}` (OpenCode) and `${env:NAME}` read the same as `${NAME}`.
        let (template, defaults) = crate::plugins::keys::normalize_references(value)
            .context("MCP header values must be an optional prefix followed by one ${NAME}")?;
        add_defaults(server, defaults, literals)?;
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
fn add_defaults(
    server: &mut McpServer,
    defaults: Vec<(String, String)>,
    literals: &mut LiteralSecrets,
) -> anyhow::Result<()> {
    for (name, default) in defaults {
        // `${NAME:-}` says the value is optional; it holds nothing to refuse.
        if !default.is_empty() && !crate::plugins::keys::safe_literal(&name, &default) {
            literals.dropped_default(&name, &default)?;
            continue;
        }
        server.defaults.insert(name, default);
    }
    Ok(())
}
/// Whether `default` for `key` is the whole of the URL's host and can't be one: Datadog writes
/// `https://${DD_MCP_DOMAIN:-not-setup}/`, which means the value is needed, not that a computer
/// called `not-setup` is the server.
fn unusable_host_default(url: &str, key: &str, default: &str) -> bool {
    let rest = url.split_once("://").map_or(url, |(_, rest)| rest);
    let host = rest.split('/').next().unwrap_or(rest);
    host == format!("${{{key}}}") && !default.contains('.') && default != "localhost"
}
fn import_mcp_server(
    name: &str,
    raw: &Value,
    opencode: bool,
    literals: &mut LiteralSecrets,
) -> anyhow::Result<McpServer> {
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
            "cwd",
            "oauth",
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
    let mut server = McpServer::default();
    if let Some(block) = raw.get("oauth") {
        match read_oauth(name, block, opencode, literals) {
            Ok(oauth) => server.oauth = oauth,
            Err(error) if error.is::<UnsupportedMcp>() => compatibility_error = Some(error),
            Err(error) => return Err(error),
        }
    }
    if let Some(cwd) = raw.get("cwd") {
        server.cwd = Some(cwd.as_str().context("MCP cwd must be text")?.to_owned());
    }
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
        let defaults = defaults
            .into_iter()
            .filter(|(key, default)| !unusable_host_default(&url, key, default))
            .collect();
        // A plugin that ships an address left empty (Informatica's): each person has their own,
        // and until it is given there is nothing to connect to.
        let url = match literals.plugin.as_deref() {
            Some(plugin) if url.trim().is_empty() => format!(
                "${{{}}}",
                crate::plugins::keys::literal_secret_name(plugin, &format!("{name}_URL"))
            ),
            _ => url,
        };
        server.url = Some(url);
        add_defaults(&mut server, defaults, literals)?;
    }
    if let Some(env) = raw.get(if opencode { "environment" } else { "env" }) {
        import_env(name, &mut server, env, literals)?;
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
        import_headers(name, &mut server, headers, literals)?;
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
/// Preset clients a provider registered for Claude (those of the render, mongodb-atlas and slack
/// plugins). Themis doesn't sign in as another app; a provider's own client for Themis would go
/// through like any other.
const CLIENTS_REGISTERED_FOR_CLAUDE: &[&str] = &[
    "claude",
    "https://claude.ai/oauth/claude-code-client-metadata",
    "1601185624273.8899143856786",
];

/// A connection's `oauth` block: how its sign-in goes (Integrations signs in after installing).
/// A bare `true` or `false` and an empty block leave it to discovery. A field Themis would not honour is named
/// rather than quietly dropped, and a client's secret must be a reference.
fn read_oauth(
    name: &str,
    block: &Value,
    opencode: bool,
    literals: &mut LiteralSecrets,
) -> anyhow::Result<Option<McpOauth>> {
    if block.is_boolean() {
        return Ok(None);
    }
    let fields = block.as_object().context("MCP oauth must be an object")?;
    let text = |value: &Value, field: &str| -> anyhow::Result<String> {
        value
            .as_str()
            .map(str::to_owned)
            .with_context(|| format!("MCP oauth {field} must be text"))
    };
    let mut oauth = McpOauth::default();
    for (field, value) in fields {
        match field.as_str() {
            "clientId" => {
                let client = text(value, field)?;
                if CLIENTS_REGISTERED_FOR_CLAUDE.contains(&client.as_str()) {
                    return Err(unsupported_mcp(
                        format!("{name}.oauth.{field}"),
                        "This sign-in is set up for Claude, so its provider would see Themis as Claude",
                        "Register a client with the provider and use its clientId",
                    ));
                }
                oauth.client_id = Some(client);
            }
            "clientSecret" => {
                let written = text(value, field)?;
                let reference = environment_reference(&written);
                oauth.client_secret_env = Some(match reference {
                    Ok(reference) => reference,
                    Err(_) if !written.contains("${") && !written.contains("{env:") => {
                        literals.secret(name, "clientSecret", &written)?
                    }
                    Err(error) => return Err(error),
                });
            }
            "scopes" | "scope" => oauth.scopes = Some(read_scopes(value)?),
            "callbackPort" if !opencode => {
                oauth.callback_port = Some(
                    value
                        .as_u64()
                        .and_then(|port| u16::try_from(port).ok())
                        .context("MCP oauth callbackPort must be a port number")?,
                );
            }
            "authServerMetadataUrl" if !opencode => {
                oauth.auth_server_metadata_url = Some(text(value, field)?);
            }
            other => {
                return Err(unsupported_mcp(
                    format!("{name}.oauth.{other}"),
                    "Unsupported MCP oauth field",
                    "Remove this field or configure the component manually",
                ));
            }
        }
    }
    Ok((oauth != McpOauth::default()).then_some(oauth))
}

/// Scopes as one space-separated string, as written either way.
fn read_scopes(value: &Value) -> anyhow::Result<String> {
    if let Some(text) = value.as_str() {
        return Ok(text.to_owned());
    }
    let words = value
        .as_array()
        .and_then(|words| words.iter().map(Value::as_str).collect::<Option<Vec<_>>>())
        .context("MCP oauth scopes must be text or a list of text")?;
    Ok(words.join(" "))
}

/// One connection from the standard `mcpServers` shape used by Claude Desktop, Claude Code
/// and Cursor: `{command, args, env}` for local servers, `{type: "http", url, headers}` for
/// remote ones. Credentials must be `${VARIABLE}` references, as for any import.
pub fn mcp_server_from_standard(name: &str, raw: &Value) -> anyhow::Result<McpServer> {
    import_mcp_server(name, raw, false, &mut LiteralSecrets::refused())
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
        if let Some(cwd) = &server.cwd {
            entry.insert("cwd".into(), json!(cwd));
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
        if let Some(oauth) = &server.oauth {
            let mut block = serde_json::Map::new();
            let mut put = |field: &str, value: Option<Value>| {
                if let Some(value) = value {
                    block.insert(field.to_owned(), value);
                }
            };
            put("clientId", oauth.client_id.as_ref().map(|id| json!(id)));
            put("callbackPort", oauth.callback_port.map(|port| json!(port)));
            put(
                "authServerMetadataUrl",
                oauth
                    .auth_server_metadata_url
                    .as_ref()
                    .map(|url| json!(url)),
            );
            put("scopes", oauth.scopes.as_ref().map(|scopes| json!(scopes)));
            put(
                "clientSecret",
                oauth
                    .client_secret_env
                    .as_ref()
                    .map(|name| json!(format!("${{{name}}}"))),
            );
            entry.insert("oauth".into(), Value::Object(block));
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
        // `Shell` is what other agents call it; plugins for several list both.
        "Bash" | "Shell" => &["shell", "background_task"],
        "Read" => &["read_file"],
        "Write" => &["write_file"],
        // Themis has no notebook tool: a notebook is edited like any other file.
        "Edit" | "MultiEdit" | "NotebookEdit" => &["apply_patch", "write_file"],
        "Grep" => &["search_file"],
        "Glob" | "LS" => &["list_dir", "search_file"],
        "WebFetch" => &["fetch_url"],
        "WebSearch" => &["web_search"],
        // Not a tool here: hooks fire for it when a skill is taken up (`hooks::skill_read_hooks`).
        "Skill" => &["Skill"],
        "Agent" | "Task" => &["spawn_subagents"],
        _ => return None,
    })
}

/// `matcher` split at its top-level `|`: one inside a group or a class doesn't split.
fn matcher_alternatives(matcher: &str) -> Vec<&str> {
    let mut parts = Vec::new();
    let (mut depth, mut class, mut escaped, mut start) = (0usize, false, false, 0);
    for (index, character) in matcher.char_indices() {
        match character {
            _ if escaped => escaped = false,
            '\\' => escaped = true,
            '[' => class = true,
            ']' => class = false,
            '(' if !class => depth += 1,
            ')' if !class => depth = depth.saturating_sub(1),
            '|' if !class && depth == 0 => {
                parts.push(&matcher[start..index]);
                start = index + 1;
            }
            _ => {}
        }
    }
    parts.push(&matcher[start..]);
    parts
}

/// A Claude tool matcher ("Edit|Write") as a Themis matcher, and the names with no Themis tool.
/// Plain names become Themis tool names (those already Themis names are kept). Alternatives
/// naming MCP tools (`mcp__.*__query`, `__exec$`) become a regular expression over Claude's
/// names for them. Any other pattern says nothing Themis can check, so the whole matcher is
/// unmapped: a guard must not silently match nothing, or everything.
fn map_tool_matcher(matcher: &str) -> (Option<String>, Vec<String>) {
    let mut mapped: Vec<String> = Vec::new();
    let mut unmapped = Vec::new();
    let mut patterns: Vec<&str> = Vec::new();
    let alternatives: Vec<&str> = matcher_alternatives(matcher)
        .into_iter()
        .map(str::trim)
        .filter(|name| !name.is_empty())
        .collect();
    let plain = |name: &str| {
        name.bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || byte == b'_')
    };
    for name in alternatives {
        if name.contains("__") {
            patterns.push(name);
            continue;
        }
        if !plain(name) {
            return (None, vec![matcher.to_owned()]);
        }
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
    if !patterns.is_empty() {
        let pattern = patterns.join("|");
        if crate::plugins::hooks::compile_matcher(&pattern).is_err() {
            return (None, vec![matcher.to_owned()]);
        }
        mapped.push(format!("re:{pattern}"));
    }
    ((!mapped.is_empty()).then(|| mapped.join("|")), unmapped)
}

/// What a command hook's Claude options come to.
#[derive(Default)]
struct HookOptions {
    shell: Option<String>,
    background: bool,
    command_prefix: Option<Vec<String>>,
    command_glob: Option<String>,
    args: Option<Vec<String>>,
    rewake: Option<crate::plugins::hooks::HookRewake>,
}

/// A hook's options, or why the hook has to stay out because its meaning wouldn't hold.
/// `windows` because some only matter on the platform the check runs on.
fn read_hook_options(raw: &Value, windows: bool) -> Result<HookOptions, String> {
    let mut options = HookOptions::default();
    let fields = raw.as_object().into_iter().flatten();
    for (key, value) in fields {
        match key.as_str() {
            "type" | "command" | "timeout" => {}
            // Text for Claude's spinner.
            "statusMessage" => {}
            "async" => options.background |= value.as_bool() == Some(true),
            // Runs in the background, and what it reports reaches the agent.
            "asyncRewake" => {
                if value.as_bool() == Some(true) {
                    let label = |field: &str| {
                        raw[field]
                            .as_str()
                            .map(str::trim)
                            .filter(|text| !text.is_empty())
                            .map(|text| capped_label(text).to_owned())
                    };
                    options.background = true;
                    options.rewake = Some(crate::plugins::hooks::HookRewake {
                        message: label("rewakeMessage"),
                        summary: label("rewakeSummary"),
                    });
                }
            }
            "rewakeMessage" | "rewakeSummary" => {}
            "shell" => match value.as_str() {
                Some("bash") => options.shell = Some("bash".to_owned()),
                other => return Err(format!("shell {} is not available", other.unwrap_or("?"))),
            },
            // The hook's command for Windows: on any other platform it never applies. Written
            // for PowerShell, and Windows hooks here run in cmd.
            "commandWindows" if windows => {
                return Err("commandWindows needs PowerShell".to_owned());
            }
            "commandWindows" => {}
            "args" => {
                if windows {
                    return Err("args are quoted for a POSIX shell".to_owned());
                }
                let words = value.as_array().and_then(|words| {
                    words
                        .iter()
                        .map(|word| word.as_str().map(str::to_owned))
                        .collect::<Option<Vec<_>>>()
                });
                options.args = Some(words.ok_or("args must be a list of words")?);
            }
            "if" => {
                let rule = value.as_str().unwrap_or_default();
                match condition_prefix(rule) {
                    Some(words) => options.command_prefix = Some(words),
                    None => {
                        options.command_glob = Some(
                            condition_glob(rule)
                                .ok_or_else(|| format!("condition {rule} can't be checked"))?,
                        );
                    }
                }
            }
            other => return Err(format!("option {other} has no Themis equivalent")),
        }
    }
    Ok(options)
}

/// The command words in an `if` rule: `Bash(git push *)`, `Bash(git push:*)` or
/// `Bash(git push)`. Patterns inside words can't be held to, so they aren't read.
fn condition_prefix(rule: &str) -> Option<Vec<String>> {
    let inner = rule.strip_prefix("Bash(")?.strip_suffix(')')?;
    let words = inner
        .strip_suffix(":*")
        .or_else(|| inner.strip_suffix(" *"))
        .unwrap_or(inner);
    let plain = |word: &str| !word.contains(['*', '?', '[', ']', '(', ')', '"', '\'', '\\', '$']);
    let words: Vec<String> = words.split_whitespace().map(str::to_owned).collect();
    (!words.is_empty() && words.iter().all(|word| plain(word))).then_some(words)
}

/// The pattern in an `if` rule that has `*` inside words: `Bash(git*commit*)`. Something other
/// than `*` must say what it is about, and `*` is the only pattern character checked.
fn condition_glob(rule: &str) -> Option<String> {
    let inner = rule.strip_prefix("Bash(")?.strip_suffix(')')?;
    let says_something = inner.bytes().any(|byte| byte.is_ascii_alphanumeric());
    let plain = !inner.contains(['?', '[', ']', '(', ')', '"', '\'', '\\', '$']);
    (says_something && plain && inner.contains('*')).then(|| inner.to_owned())
}

/// `text`, cut to what a hook's report label may hold.
fn capped_label(text: &str) -> &str {
    let mut end = text.len().min(crate::plugins::hooks::REWAKE_LABEL_LIMIT);
    while !text.is_char_boundary(end) {
        end -= 1;
    }
    &text[..end]
}

/// A program and its arguments as one line for a POSIX shell, each word reaching the program as
/// it was written. Only `${CLAUDE_PLUGIN_ROOT}` is left for the shell to fill in.
fn quoted_command(program: &str, args: &[String]) -> String {
    const ROOT: &str = "${CLAUDE_PLUGIN_ROOT}";
    let quote = |word: &str| {
        if !word.is_empty()
            && word
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || b"_-./:=@%+,".contains(&byte))
        {
            return word.to_owned();
        }
        let escape = |part: &str| {
            part.chars().fold(String::new(), |mut text, character| {
                if matches!(character, '\\' | '"' | '`' | '$') {
                    text.push('\\');
                }
                text.push(character);
                text
            })
        };
        let parts: Vec<String> = word.split(ROOT).map(escape).collect();
        format!("\"{}\"", parts.join(ROOT))
    };
    std::iter::once(program)
        .chain(args.iter().map(String::as_str))
        .map(quote)
        .collect::<Vec<_>>()
        .join(" ")
}

/// What an imported hook gets when it asks for no time limit: Claude's scripts are written for
/// more than a native hook's ten seconds.
const IMPORTED_HOOK_TIMEOUT: u64 = 60;

/// Whether a `PermissionRequest` hook's worth is answering yes, going by its command and the script
/// it runs. Hooks never grant permissions, so such a hook can't do what it is for, and the plugin
/// has to say so. Only text can be read: a compiled program that answers allow still has it
/// ignored when it runs, and logged.
fn answers_allow(command: &str, root: &Path) -> bool {
    static ALLOW: std::sync::LazyLock<regex::Regex> = std::sync::LazyLock::new(|| {
        regex::Regex::new(r#"behavior[\\"']*\s*[:=]\s*[\\"']*allow"#).expect("valid pattern")
    });
    if ALLOW.is_match(command) {
        return true;
    }
    const ROOT: &str = "${CLAUDE_PLUGIN_ROOT}/";
    let Ok(folder) = root.canonicalize() else {
        return false;
    };
    command.match_indices(ROOT).any(|(at, _)| {
        let rest = &command[at + ROOT.len()..];
        let path = rest
            .split(|c: char| c.is_whitespace() || matches!(c, '"' | '\'' | ';' | '|' | '&'))
            .next()
            .unwrap_or_default();
        let Ok(script) = root.join(path).canonicalize() else {
            return false;
        };
        script.starts_with(&folder)
            && fs::metadata(&script).is_ok_and(|meta| meta.len() <= 1024 * 1024)
            && fs::read(&script).is_ok_and(|bytes| ALLOW.is_match(&String::from_utf8_lossy(&bytes)))
    })
}

fn import_hooks(value: &Value, root: &Path, spec: &mut PluginSpec) -> anyhow::Result<()> {
    let events = value
        .get("hooks")
        .unwrap_or(value)
        .as_object()
        .context("Invalid hook configuration")?;
    for (event, groups) in events {
        let mapped = match event.as_str() {
            // Themis has these moments under the same names; see `hooks::EVENTS`.
            "SessionStart" => "SessionStart",
            "SessionEnd" => "SessionEnd",
            "UserPromptExpansion" => "UserPromptExpansion",
            "PermissionRequest" => "PermissionRequest",
            "PermissionDenied" => "PermissionDenied",
            "PostToolBatch" => "PostToolBatch",
            "PostCompact" => "PostCompact",
            "Notification" => "Notification",
            "TaskCreated" => "TaskCreated",
            "TaskCompleted" => "TaskCompleted",
            "ConfigChange" => "ConfigChange",
            "InstructionsLoaded" => "InstructionsLoaded",
            // Fires only when the run failed; `Stop` also runs then, as RunEnd always does.
            "StopFailure" => "RunFailed",
            "PreToolUse" => "BeforeTool",
            "PostToolUse" => "AfterTool",
            "Stop" => "Stop",
            "PreCompact" => "BeforeCompaction",
            "SubagentStart" => "SubagentStart",
            "SubagentStop" => "SubagentEnd",
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
            "PreToolUse"
                | "PostToolUse"
                | "PostToolUseFailure"
                | "PermissionRequest"
                | "PermissionDenied"
        );
        // What each event can be about here, and so what a matcher on it can ever match. An empty
        // list takes any name (a skill's). Claude takes no matcher on the last group.
        let moments: Option<&[&str]> = match event.as_str() {
            "SessionStart" => Some(&["startup", "resume"]),
            "SessionEnd" => Some(&["prompt_input_exit", "other"]),
            "PostCompact" => Some(&["auto"]),
            "Notification" => Some(&[
                "permission_prompt",
                "goal_reached",
                "goal_stuck",
                "goal_waiting",
                "goal_quota",
            ]),
            "ConfigChange" => Some(&["user_settings", "project_settings"]),
            "InstructionsLoaded" => Some(&["session_start"]),
            "UserPromptExpansion" => Some(&[]),
            _ => None,
        };
        let matcher_ignored = matches!(
            event.as_str(),
            "PostToolBatch" | "TaskCreated" | "TaskCompleted"
        );
        for group in groups.as_array().context("Invalid hook groups")? {
            for raw in group["hooks"].as_array().context("Invalid hook list")? {
                if raw["type"] != "command" {
                    // Nothing here asks a model or a web address whether a tool may run.
                    let kind = raw["type"].as_str().unwrap_or("other");
                    spec.unsupported
                        .push(format!("{event}: {kind} hook has no Themis equivalent"));
                    continue;
                }
                // Clearing, compacting and logging out have no equivalent: a matcher naming only
                // those would never run, which isn't the same as working.
                let mut moment_matcher = None;
                if let (Some(moments), Some(matcher)) = (
                    moments,
                    group["matcher"]
                        .as_str()
                        .filter(|matcher| !matcher.is_empty() && *matcher != "*"),
                ) {
                    let reachable = moments.is_empty()
                        || moments
                            .iter()
                            .any(|moment| super::hooks::moment_matches(matcher, moment));
                    let compiles = matcher
                        .chars()
                        .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '|')
                        || super::hooks::compile_matcher(matcher).is_ok();
                    if !reachable || !compiles {
                        spec.unsupported.push(format!(
                            "{event}: matcher {matcher} has no Themis equivalent"
                        ));
                        continue;
                    }
                    moment_matcher = Some(matcher.to_owned());
                }
                if !tool_event
                    && moments.is_none()
                    && !matcher_ignored
                    && group["matcher"]
                        .as_str()
                        .is_some_and(|matcher| !matcher.is_empty() && matcher != "*")
                {
                    spec.unsupported.push(format!(
                        "{event}: lifecycle matcher requires manual mapping"
                    ));
                    continue;
                }
                let options = match read_hook_options(raw, cfg!(windows)) {
                    Ok(options) => options,
                    Err(reason) => {
                        spec.unsupported.push(format!("{event}: {reason}"));
                        continue;
                    }
                };
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
                        if mapped_tools.is_none() {
                            continue;
                        }
                        mapped_tools
                    }
                    _ if moments.is_some() || matcher_ignored => None,
                    other => other.map(str::to_owned),
                };
                let command = raw["command"].as_str().context("Missing hook command")?;
                if event == "PermissionRequest" && answers_allow(command, root) {
                    spec.unsupported.push(format!(
                        "{event}: answers allow, and hooks never grant permissions"
                    ));
                    continue;
                }
                let command = match &options.args {
                    Some(args) => quoted_command(command, args),
                    None => command.to_owned(),
                };
                let (shell, background, command_prefix, command_glob, rewake) = (
                    options.shell,
                    options.background,
                    options.command_prefix,
                    options.command_glob,
                    options.rewake,
                );
                // Nobody waits for one that reports back, and a review takes its time.
                let default_timeout = if rewake.is_some() {
                    super::hooks::MAX_TIMEOUT_SECONDS
                } else {
                    IMPORTED_HOOK_TIMEOUT
                };
                spec.hooks.push(Hook {
                    name: imported_hook_name(mapped, matcher.as_deref(), &command, &spec.hooks),
                    event: mapped.into(),
                    command,
                    enabled: false,
                    matcher,
                    failure_policy: None,
                    shell,
                    background,
                    command_prefix,
                    command_glob,
                    // Claude's way to refuse; a SessionStart hook can't stop a turn there either.
                    // One that reports back says so the same way, whatever it runs at.
                    exit_two_blocks: event != "SessionStart" || rewake.is_some(),
                    rewake,
                    timeout_seconds: raw["timeout"]
                        .as_u64()
                        .unwrap_or(default_timeout)
                        .clamp(1, super::hooks::MAX_TIMEOUT_SECONDS),
                    // A broken session hook must not stop every turn.
                    blocking: event == "PreToolUse" && !background,
                    plugin_root: None,
                    plugin_data: None,
                    env_file: None,
                    moment_matcher,
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
    let tool = matcher
        .and_then(|matcher| matcher.split('|').next())
        .map(|tool| if tool.starts_with("re:") { "mcp" } else { tool });
    let base = script.unwrap_or_else(|| match (event, tool) {
        ("PromptSubmit", _) => "prompt-submit".into(),
        ("RunStart", _) => "run-start".into(),
        ("BeforeCompaction", _) => "before-compaction".into(),
        ("RunEnd", _) => "run-end".into(),
        ("Stop", _) => "stop".into(),
        ("RunFailed", _) => "run-failed".into(),
        ("BeforeTool", Some(tool)) => format!("before-{tool}").replace('_', "-"),
        ("AfterTool", Some(tool)) => format!("after-{tool}").replace('_', "-"),
        ("ToolCallFailed", Some(tool)) => format!("{tool}-failed").replace('_', "-"),
        ("BeforeTool", None) => "before-tool".into(),
        ("AfterTool", None) => "after-tool".into(),
        ("ToolCallFailed", None) => "tool-failed".into(),
        // `SessionEnd` is `session-end`.
        (other, _) => {
            let mut name = String::new();
            for letter in other.chars() {
                if letter.is_ascii_uppercase() && !name.is_empty() {
                    name.push('-');
                }
                name.push(letter.to_ascii_lowercase());
            }
            name
        }
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
    fn only_bounded_svg_png_and_webp_become_icons() {
        let png = [b"\x89PNG\r\n\x1a\n".as_slice(), &[0; 16]].concat();
        assert!(icon_asset_name(&png).unwrap().ends_with(".png"));
        let webp = [b"RIFF\0\0\0\0WEBP".as_slice(), &[0; 16]].concat();
        assert!(icon_asset_name(&webp).unwrap().ends_with(".webp"));
        assert!(icon_asset_name(b"  <svg xmlns='x'/>")
            .unwrap()
            .ends_with(".svg"));
        for bad in [b"".as_slice(), b"<html></html>", b"GIF89a", b"#!/bin/sh"] {
            assert!(icon_asset_name(bad).is_none());
        }
        let huge = [b"<svg>".as_slice(), &vec![b' '; ICON_LIMIT]].concat();
        assert!(icon_asset_name(&huge).is_none());
    }

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
                shell: None,
                background: false,
                command_prefix: None,
                command_glob: None,
                rewake: None,
                exit_two_blocks: false,
                timeout_seconds: 10,
                blocking: false,
                plugin_root: None,
                plugin_data: None,
                env_file: None,
                moment_matcher: None,
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

#[cfg(test)]
mod size_tests {
    use super::super::package::{Limits, LIMITS, MAX_INLINE_FILE_BYTES};
    use super::*;

    fn folder(files: &[(&str, &[u8])]) -> tempfile::TempDir {
        let root = tempfile::tempdir().unwrap();
        let manifest = root.path().join(".claude-plugin/plugin.json");
        fs::create_dir_all(manifest.parent().unwrap()).unwrap();
        fs::write(manifest, r#"{"name": "big"}"#).unwrap();
        for (path, content) in files {
            let path = root.path().join(path);
            fs::create_dir_all(path.parent().unwrap()).unwrap();
            fs::write(path, content).unwrap();
        }
        root
    }

    const SKILL: &[u8] = b"---\nname: s\ndescription: d\n---\nBody";

    #[test]
    fn default_enabled_false_arrives_switched_off() {
        let root = folder(&[("skills/s/SKILL.md", SKILL)]);
        let manifest = root.path().join(".claude-plugin/plugin.json");
        fs::write(&manifest, r#"{"name": "big", "defaultEnabled": false}"#).unwrap();
        let data = tempfile::tempdir().unwrap();
        let store = PluginStore::new(data.path().into(), None).with_home(None);
        let plugin = store.import_path("global", root.path(), None).unwrap();
        assert!(!plugin.enabled);
        assert!(
            plugin.spec.unsupported.is_empty(),
            "{:?}",
            plugin.spec.unsupported
        );
        fs::write(&manifest, r#"{"name": "other", "defaultEnabled": true}"#).unwrap();
        let plugin = store.import_path("global", root.path(), None).unwrap();
        assert!(plugin.enabled);
    }

    #[test]
    fn an_output_style_is_a_manual_skill_and_says_what_it_is() {
        let style: &[u8] = b"---\nname: Terse\ndescription: Short\n---\nBe brief.";
        let root = folder(&[
            ("skills/s/SKILL.md", SKILL),
            ("output-styles/terse.md", style),
        ]);
        let spec = import_directory(root.path(), true, None, None).unwrap();
        let skill = spec
            .skills
            .iter()
            .find(|skill| skill.id == "style-terse")
            .unwrap();
        assert!(
            skill.instructions.ends_with("Be brief."),
            "{}",
            skill.instructions
        );
        assert!(spec.manual_skills.contains(&"style-terse".to_owned()));
        assert!(spec.unsupported.is_empty(), "{:?}", spec.unsupported);
        assert!(spec
            .notices
            .iter()
            .any(|note| note.contains("for one turn")));
    }

    #[test]
    fn a_servers_cwd_stays_inside() {
        let server = |cwd: &str| McpServer {
            command: Some("x".into()),
            cwd: Some(cwd.into()),
            ..Default::default()
        };
        for good in [
            ".",
            "sub/dir",
            "${CLAUDE_PLUGIN_ROOT}",
            "${CLAUDE_PLUGIN_ROOT}/x",
        ] {
            assert!(server(good).validate().is_ok(), "{good}");
        }
        for bad in [
            "..",
            "../x",
            "/etc",
            "${CLAUDE_PLUGIN_ROOT}/../x",
            "a/../../b",
        ] {
            assert!(server(bad).validate().is_err(), "{bad}");
        }
    }

    #[test]
    fn a_small_text_plugin_stays_in_the_registry_as_before() {
        let root = folder(&[("skills/s/SKILL.md", SKILL), ("skills/s/notes.txt", b"n")]);
        let spec = import_directory(root.path(), true, None, None).unwrap();
        assert!(spec.package.is_none());
        assert!(spec.files.contains_key("skills/s/notes.txt"));
        assert!(spec.unsupported.is_empty(), "{:?}", spec.unsupported);
    }

    #[test]
    fn a_large_or_binary_file_moves_the_folder_out_of_the_registry() {
        let large = vec![b'x'; MAX_INLINE_FILE_BYTES as usize + 1];
        for (name, content) in [
            ("vendor/big.js", large.as_slice()),
            ("bin.dat", &[0xff, 0xfe]),
        ] {
            let root = folder(&[("skills/s/SKILL.md", SKILL), (name, content)]);
            let spec = import_directory(root.path(), true, None, None).unwrap();
            let package = spec.package.expect(name);
            assert_eq!(package.files, 3);
            assert!(!spec.files.contains_key(name));
            // What the importer reads stays, so the skill is found.
            assert_eq!(spec.skills.len(), 1);
            assert!(spec.unsupported.is_empty(), "{:?}", spec.unsupported);
            assert!(spec.notices.is_empty(), "{:?}", spec.notices);
        }
    }

    #[test]
    fn a_check_only_hashes_and_an_install_keeps_the_files() {
        let root = folder(&[("skills/s/SKILL.md", SKILL), ("bin.dat", &[0xff])]);
        let store = tempfile::tempdir().unwrap();
        let checked = import_directory(root.path(), true, None, None).unwrap();
        assert_eq!(fs::read_dir(store.path()).unwrap().count(), 0);
        let kept = import_directory(root.path(), true, None, Some(store.path())).unwrap();
        assert_eq!(checked.package, kept.package);
        let hash = kept.package.unwrap().hash;
        assert_eq!(
            fs::read(store.path().join(hash).join("bin.dat")).unwrap(),
            [0xff]
        );
    }

    #[test]
    fn past_the_bounds_a_plugin_keeps_only_its_own_folders_and_says_so() {
        let big = vec![b'x'; 1024];
        let root = folder(&[
            ("skills/s/SKILL.md", SKILL),
            ("src/a.bin", &big),
            ("src/b.bin", &big),
        ]);
        for limits in [
            Limits {
                total_bytes: 1500,
                ..LIMITS
            },
            Limits { files: 3, ..LIMITS },
        ] {
            let spec = import_directory_within(root.path(), true, None, None, limits).unwrap();
            assert!(!spec.files.contains_key("src/a.bin"));
            assert!(
                spec.unsupported
                    .iter()
                    .any(|reason| reason.starts_with("Large repository")),
                "{:?}",
                spec.unsupported
            );
            assert_eq!(spec.skills.len(), 1);
        }
        // A single file past the bound is left out, with a note, and the rest is kept.
        let limits = Limits {
            file_bytes: 100,
            ..LIMITS
        };
        let spec = import_directory_within(root.path(), true, None, None, limits).unwrap();
        assert!(spec
            .notices
            .iter()
            .any(|note| note == "File too large: src/a.bin"));
    }

    #[test]
    fn a_plugin_too_large_even_for_its_own_folders_fails() {
        let root = folder(&[
            ("skills/s/SKILL.md", SKILL),
            ("skills/s/x.bin", &[0u8; 2048]),
        ]);
        let limits = Limits {
            total_bytes: 1024,
            ..LIMITS
        };
        let error = import_directory_within(root.path(), true, None, None, limits).unwrap_err();
        assert_eq!(error.to_string(), RESOURCE_LIMITS);
    }

    /// A check keeps nothing; the install from it downloads the files again, and stops if they
    /// aren't what was checked.
    #[tokio::test]
    async fn installing_a_checked_package_keeps_it_or_stops_if_it_changed() {
        let data = tempfile::tempdir().unwrap();
        let source = tempfile::tempdir().unwrap();
        let manifest = source.path().join(".claude-plugin/marketplace.json");
        fs::create_dir_all(manifest.parent().unwrap()).unwrap();
        fs::write(
            manifest,
            r#"{"name": "local", "plugins": [{"name": "big", "source": "./big"}]}"#,
        )
        .unwrap();
        let plugin = source.path().join("big");
        fs::create_dir_all(plugin.join("skills/s")).unwrap();
        fs::write(plugin.join("skills/s/SKILL.md"), SKILL).unwrap();
        fs::write(plugin.join("tool.bin"), [0xff, 0x01]).unwrap();
        let store = PluginStore::new(data.path().into(), None).with_home(None);
        store
            .add_marketplace("local", source.path().to_str().unwrap())
            .unwrap();
        store.wait_marketplace_scan("local", false).await.unwrap();
        let packages = data
            .path()
            .join("plugins")
            .join(super::super::package::PACKAGES_DIR);
        assert!(!packages.exists(), "a check stored the package");
        let installed = store
            .install_scanned("global", "local", "big", false, None)
            .await
            .unwrap();
        let hash = installed.spec.package.as_ref().unwrap().hash.clone();
        assert_eq!(
            fs::read(packages.join(&hash).join("tool.bin")).unwrap(),
            [0xff, 0x01]
        );
        // Materialized for a run, then the store can be read again from the registry alone.
        let project = tempfile::tempdir().unwrap();
        store
            .materialize(std::slice::from_ref(&installed), project.path())
            .unwrap();
        let copied = project
            .path()
            .join(".themis/plugin-files")
            .join(installed.skill_id("resources"))
            .join("tool.bin");
        assert_eq!(fs::read(copied).unwrap(), [0xff, 0x01]);
        // Upstream changed after the check: what was reviewed is no longer what would arrive.
        store.delete("global", "big").unwrap();
        fs::remove_dir_all(packages.join(&hash)).unwrap();
        fs::write(plugin.join("tool.bin"), [0xff, 0x02]).unwrap();
        let error = store
            .install_scanned("global", "local", "big", false, None)
            .await
            .unwrap_err();
        assert!(
            error.to_string().contains("changed after it was checked"),
            "{error}"
        );
        assert_eq!(fs::read_dir(&packages).unwrap().count(), 0);
    }

    #[tokio::test]
    async fn a_download_past_its_limit_is_refused() {
        let repository = folder(&[("big.bin", &[1u8; 4096])]);
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
                .args(args)
                .current_dir(repository.path())
                .status()
                .unwrap()
                .success());
        }
        let target = tempfile::tempdir().unwrap();
        let path = target.path().join("copy");
        let source = repository.path().to_str().unwrap();
        let args = ["clone", "--depth=1", "--", source, path.to_str().unwrap()];
        let error = run_git_within(&args, None, Some((path.as_path(), 1024)))
            .await
            .unwrap_err();
        assert_eq!(error.to_string(), REPOSITORY_TOO_LARGE);
    }
}
