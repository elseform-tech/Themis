use super::*;

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
    pub(in crate::plugins) async fn catalog_for_scan(
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
    pub(in crate::plugins) async fn marketplace_directory(
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
        let _scan_lock = self.scan_lock_waiting(marketplace).await?;
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
    pub(in crate::plugins) fn install_spec(
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
                switch_off_new_parts(Some(&previous.spec), &mut spec);
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
        let _lock = self.scan_lock_waiting(marketplace).await?;
        self.marketplace_spec(marketplace, name, false, false, None)
            .await
    }
    pub async fn inspect_marketplace(
        &self,
        marketplace: &str,
        name: &str,
    ) -> anyhow::Result<ImportPreview> {
        let _lock = self.scan_lock_waiting(marketplace).await?;
        self.inspect_for_scan(marketplace, name).await
    }
    pub(in crate::plugins) async fn inspect_for_scan(
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
    pub(in crate::plugins) async fn keep_checked_package(
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
            let directory = self
                .source_directory(&root, source, &mut temporary_root)
                .await?;
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

    /// The folder a catalog entry's `source` names: a folder in the catalog repository, or a
    /// repository cloned into a temporary folder, which `temporary_root` records for removal.
    async fn source_directory(
        &self,
        root: &Path,
        source: &Value,
        temporary_root: &mut Option<PathBuf>,
    ) -> anyhow::Result<PathBuf> {
        if let Some(path) = source.as_str() {
            return catalog_folder(root, path);
        }
        if source["source"].as_str() == Some("command") {
            return command_source_directory(source, ALLOWED_COMMANDS, COMMAND_TIMEOUT).await;
        }
        let url = match source["source"].as_str() {
            Some("github") => format!(
                "https://github.com/{}.git",
                source["repo"].as_str().context("Missing repo")?
            ),
            Some("url" | "git-subdir") => source["url"].as_str().context("Missing URL")?.into(),
            _ => bail!("Unsupported marketplace source type"),
        };
        validate_source(&url)?;
        let staging = self
            .global
            .join("marketplace-cache")
            .join(format!("plugin-{}", revision()));
        *temporary_root = Some(staging.clone());
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
        repository_subdirectory(&staging, source)
    }
}

/// A folder inside the catalog's own repository, named by a relative path.
fn catalog_folder(root: &Path, path: &str) -> anyhow::Result<PathBuf> {
    let path = path.strip_prefix("./").unwrap_or(path);
    if !safe_relative(path) {
        bail!("Unsafe marketplace source path");
    }
    let directory = root.join(path).canonicalize()?;
    if !directory.starts_with(root.canonicalize()?) {
        bail!("Marketplace source escapes repository");
    }
    Ok(directory)
}

/// The plugin's folder in a cloned repository: its `path`, or the whole repository.
fn repository_subdirectory(staging: &Path, source: &Value) -> anyhow::Result<PathBuf> {
    let Some(path) = source.get("path") else {
        return Ok(staging.to_path_buf());
    };
    let path = path.as_str().context("Invalid repository subdirectory")?;
    if !safe_relative(path) {
        bail!("Unsafe repository subdirectory");
    }
    let directory = staging.join(path).canonicalize()?;
    if !directory.starts_with(staging.canonicalize()?) {
        bail!("Plugin directory escapes repository");
    }
    Ok(directory)
}
