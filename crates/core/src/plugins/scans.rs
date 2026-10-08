//! Derived compatibility snapshots, never an integration registry or runtime health check.
use super::*;
use serde_json::Value;

#[derive(Clone, Default, Serialize, Deserialize)]
pub struct MarketplaceScan {
    pub revision: String,
    pub status: String,
    pub completed: usize,
    pub total: usize,
    pub catalog: Value,
    pub checks: BTreeMap<String, ImportPreview>,
    pub errors: BTreeMap<String, String>,
    pub error: Option<String>,
    source: String,
    version: u32,
    #[serde(default)]
    refresh: bool,
}

fn preview_file(name: &str) -> String {
    // Hex keeps distinct catalog names distinct on case-insensitive filesystems.
    let key: String = name.bytes().map(|byte| format!("{byte:02x}")).collect();
    format!("{key}.json")
}

impl PluginStore {
    fn scan_directory(&self, name: &str) -> anyhow::Result<PathBuf> {
        if !safe_name(name) {
            bail!("Invalid marketplace name");
        }
        Ok(self.global.join("compatibility-cache").join(name))
    }
    fn scan_lock_file(&self, name: &str) -> anyhow::Result<fs::File> {
        let directory = self.scan_directory(name)?;
        fs::create_dir_all(&directory)?;
        let lock = fs::OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .open(directory.join("scan.lock"))?;
        Ok(lock)
    }
    pub(super) fn scan_lock(&self, name: &str) -> anyhow::Result<fs::File> {
        let lock = self.scan_lock_file(name)?;
        lock.try_lock()
            .context("Marketplace is being checked; try again when scanning finishes")?;
        Ok(lock)
    }
    async fn wait_scan_lock(&self, name: &str) -> anyhow::Result<fs::File> {
        let lock = self.scan_lock_file(name)?;
        loop {
            match lock.try_lock() {
                Ok(()) => return Ok(lock),
                Err(fs::TryLockError::WouldBlock) => {
                    tokio::time::sleep(std::time::Duration::from_millis(25)).await
                }
                Err(error) => return Err(error.into()),
            }
        }
    }
    fn scan_state(&self, name: &str) -> anyhow::Result<MarketplaceScan> {
        read_json(&self.scan_directory(name)?.join("scan.json"))
    }
    /// Starts at most one scan, or returns its saved progress. The file lock also survives
    /// multiple app-server processes; process exit releases it so the next request resumes.
    pub fn start_marketplace_scan(
        &self,
        name: &str,
        refresh: bool,
    ) -> anyhow::Result<MarketplaceScan> {
        let source = self
            .marketplaces()?
            .into_iter()
            .find(|m| m.name == name)
            .context("Unknown marketplace")?
            .source;
        let lock = match self.scan_lock(name) {
            Ok(lock) => lock,
            Err(error) => {
                let mut state = self.scan_state(name)?;
                if state.source == source
                    && (state.status == "scanning" || !refresh && state.status == "ready")
                {
                    // A completed writer may still be releasing its lock. Keep callers waiting.
                    state.status = "scanning".into();
                    return Ok(state);
                }
                return Err(error);
            }
        };
        let mut state = self.scan_state(name).unwrap_or_default();
        let source_changed = state.source != source;
        if refresh || source_changed || state.version != 1 {
            state = MarketplaceScan {
                revision: revision(),
                source,
                version: 1,
                refresh: refresh || source_changed,
                ..Default::default()
            };
        }
        if matches!(state.status.as_str(), "ready" | "failed") {
            return Ok(state);
        }
        state.status = "scanning".into();
        write_json(&self.scan_directory(name)?.join("scan.json"), &state)?;
        let store = self.clone();
        let name = name.to_owned();
        let returned = state.clone();
        tokio::spawn(async move {
            let _lock = lock;
            let outcome = store.run_marketplace_scan(&name, &mut state).await;
            if let Err(error) = outcome {
                state.status = "failed".into();
                state.error = Some(error.to_string());
                let _ = write_json(
                    &store.scan_directory(&name).unwrap().join("scan.json"),
                    &state,
                );
            }
        });
        Ok(returned)
    }
    async fn run_marketplace_scan(
        &self,
        name: &str,
        state: &mut MarketplaceScan,
    ) -> anyhow::Result<()> {
        let directory = self.scan_directory(name)?;
        if state.catalog.is_null() {
            state.catalog = self.catalog_for_scan(name, state.refresh).await?;
        }
        let entries = state.catalog["plugins"]
            .as_array()
            .context("Missing plugins")?;
        if entries.len() > 2000 {
            bail!("Marketplace exceeds 2,000 plugins");
        }
        let names = entries
            .iter()
            .map(|entry| {
                entry["name"]
                    .as_str()
                    .filter(|name| safe_name(name))
                    .map(str::to_owned)
                    .context("Invalid catalog plugin name")
            })
            .collect::<anyhow::Result<Vec<_>>>()?;
        if names
            .iter()
            .collect::<std::collections::BTreeSet<_>>()
            .len()
            != names.len()
        {
            bail!("Duplicate catalog plugin name");
        }
        state.total = names.len();
        write_json(&directory.join("scan.json"), state)?;
        // ponytail: one clone at a time per marketplace; use a bounded pool if measured scan latency warrants it.
        for plugin in names {
            if state.checks.contains_key(&plugin) || state.errors.contains_key(&plugin) {
                continue;
            }
            match self.inspect_for_scan(name, &plugin).await {
                Ok(mut preview) => {
                    write_json(
                        &directory.join("previews").join(preview_file(&plugin)),
                        &preview,
                    )?;
                    // Tiles need counts and reasons; full instruction bodies stay in per-plugin files.
                    preview.spec.files.clear();
                    for skill in &mut preview.spec.skills {
                        skill.instructions.clear();
                    }
                    state.checks.insert(plugin, preview);
                }
                Err(error) => {
                    state.errors.insert(plugin, error.to_string());
                }
            }
            state.completed = state.checks.len() + state.errors.len();
            write_json(&directory.join("scan.json"), state)?;
            tokio::task::yield_now().await;
        }
        state.status = "ready".into();
        write_json(&directory.join("scan.json"), state)
    }
    pub async fn wait_marketplace_scan(
        &self,
        name: &str,
        refresh: bool,
    ) -> anyhow::Result<MarketplaceScan> {
        let mut state = self.start_marketplace_scan(name, refresh)?;
        while state.status == "scanning" {
            tokio::time::sleep(std::time::Duration::from_millis(100)).await;
            state = self.start_marketplace_scan(name, false)?;
        }
        if state.status != "ready" {
            bail!(
                "{}",
                state
                    .error
                    .as_deref()
                    .unwrap_or("Compatibility scan failed; refresh to retry")
            );
        }
        Ok(state)
    }
    fn ready_scan(&self, marketplace: &str) -> anyhow::Result<MarketplaceScan> {
        let source = self
            .marketplaces()?
            .into_iter()
            .find(|m| m.name == marketplace)
            .context("Unknown marketplace")?
            .source;
        let state = self.scan_state(marketplace)?;
        if state.status != "ready" || state.source != source || state.version != 1 {
            bail!("Marketplace is not ready; finish its compatibility scan first");
        }
        Ok(state)
    }
    fn read_scanned_preview(
        &self,
        marketplace: &str,
        name: &str,
        state: &MarketplaceScan,
    ) -> anyhow::Result<ImportPreview> {
        if let Some(error) = state.errors.get(name) {
            bail!("{error}");
        }
        if !state.checks.contains_key(name) || !safe_name(name) {
            bail!("Plugin has no completed compatibility check");
        }
        let bytes = fs::read(
            self.scan_directory(marketplace)?
                .join("previews")
                .join(preview_file(name)),
        )?;
        if bytes.len() > 32 * 1024 * 1024 {
            bail!("Cached preview exceeds 32 MiB");
        }
        let preview: ImportPreview = serde_json::from_slice(&bytes)?;
        if preview.spec.name != name {
            bail!("Cached plugin identity does not match; refresh the marketplace");
        }
        ImportPreview::from_spec(preview.spec)
    }
    pub async fn scanned_marketplace_preview(
        &self,
        marketplace: &str,
        name: &str,
    ) -> anyhow::Result<ImportPreview> {
        let _lock = self.wait_scan_lock(marketplace).await?;
        self.read_scanned_preview(marketplace, name, &self.ready_scan(marketplace)?)
    }
    pub async fn install_scanned(
        &self,
        scope: &str,
        marketplace: &str,
        name: &str,
        allow_partial: bool,
        expected_scan: Option<&str>,
    ) -> anyhow::Result<Plugin> {
        let _lock = self.wait_scan_lock(marketplace).await?;
        let state = self.ready_scan(marketplace)?;
        if expected_scan.is_some_and(|expected| expected != state.revision) {
            bail!("Marketplace changed; reopen the plugin before installing");
        }
        let preview = self.read_scanned_preview(marketplace, name, &state)?;
        if !allow_partial && preview.report.status == "partial" {
            bail!("Plugin contains unsupported components; explicitly select partial installation");
        }
        preview.spec.ensure_installable()?;
        self.install_spec(scope, marketplace, name, preview.spec)
    }
}
