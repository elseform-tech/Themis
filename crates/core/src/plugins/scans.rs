//! Derived compatibility snapshots, never an integration registry or runtime health check.
use super::*;
use futures_util::{stream, StreamExt};
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
    /// When this check finished, in milliseconds since the epoch.
    #[serde(default)]
    pub checked_at: Option<u64>,
    source: String,
    version: u32,
    #[serde(default)]
    refresh: bool,
}

/// One row of the Sources list.
#[derive(Clone, Serialize)]
pub struct SourceStatus {
    pub name: String,
    pub source: String,
    pub enabled: bool,
    pub official: bool,
    pub status: String,
    pub completed: usize,
    pub total: usize,
    pub checked_at: Option<u64>,
    pub error: Option<String>,
}

/// Bumped whenever import rules change, so saved checks are redone with the new rules.
const SCAN_VERSION: u32 = 7;

/// Discover groups for catalogs we sort by hand, keyed by marketplace and plugin name.
const PLUGIN_GROUPS: &str = include_str!("../../builtins/plugin-groups.json");

fn bundled_groups(marketplace: &str) -> Option<BTreeMap<String, String>> {
    let all: BTreeMap<String, BTreeMap<String, String>> =
        serde_json::from_str(PLUGIN_GROUPS).expect("bundled plugin groups");
    all.get(marketplace).cloned()
}

/// The Discover group of each catalog entry: the bundled map first, then the entry's catalog
/// `category`, else Other.
pub fn catalog_groups(marketplace: &str, catalog: &Value) -> BTreeMap<String, String> {
    let bundled = bundled_groups(marketplace).unwrap_or_default();
    catalog["plugins"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|entry| {
            let name = entry["name"].as_str()?;
            let group = bundled.get(name).cloned().unwrap_or_else(|| {
                category_group(entry["category"].as_str().unwrap_or_default()).into()
            });
            Some((name.to_owned(), group))
        })
        .collect()
}

fn category_group(category: &str) -> &'static str {
    match category {
        "development" | "testing" => "Coding",
        "deployment" | "migration" => "Cloud & deploy",
        "database" => "Data & databases",
        "monitoring" => "Monitoring",
        "security" => "Security",
        "productivity" => "Work apps",
        "design" => "Design",
        _ => "Other",
    }
}

/// Counts of a finished check: plugins per status, and how many plugins each reason affects.
/// For a hand-sorted catalog it also lists entries the bundled group map doesn't cover yet.
pub fn compatibility_summary(marketplace: &str, scan: &MarketplaceScan) -> Value {
    let mut statuses = BTreeMap::<String, usize>::new();
    let mut reasons = BTreeMap::<String, usize>::new();
    for preview in scan.checks.values() {
        *statuses.entry(preview.report.status.clone()).or_default() += 1;
        let keys: std::collections::BTreeSet<String> = preview
            .report
            .components
            .iter()
            .filter(|c| !matches!(c.status.as_str(), "supported" | "notice"))
            .filter_map(|c| c.reason.as_deref().map(reason_key))
            .collect();
        for key in keys {
            *reasons.entry(key).or_default() += 1;
        }
    }
    for error in scan.errors.values() {
        *statuses.entry("failed".into()).or_default() += 1;
        *reasons.entry(reason_key(error)).or_default() += 1;
    }
    let mut reasons: Vec<_> = reasons.into_iter().collect();
    reasons.sort_by(|a, b| b.1.cmp(&a.1).then_with(|| a.0.cmp(&b.0)));
    let ungrouped: Vec<&str> = bundled_groups(marketplace)
        .map(|bundled| {
            scan.catalog["plugins"]
                .as_array()
                .into_iter()
                .flatten()
                .filter_map(|entry| entry["name"].as_str())
                .filter(|name| !bundled.contains_key(*name))
                .collect()
        })
        .unwrap_or_default();
    serde_json::json!({
        "total": scan.total,
        "ungrouped": ungrouped,
        "statuses": statuses,
        "reasons": reasons.into_iter().map(|(reason, plugins)| serde_json::json!({"reason": reason, "plugins": plugins})).collect::<Vec<_>>(),
    })
}

/// Groups reasons that differ only in a path or component name ("Link not followed: a/b").
fn reason_key(reason: &str) -> String {
    match reason.split_once(": ") {
        Some((head, tail)) if head.contains('/') || head.starts_with("mcpServers.") => tail.into(),
        Some((head, _)) => head.into(),
        None => reason.into(),
    }
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
    fn scan_state(&self, name: &str) -> anyhow::Result<MarketplaceScan> {
        read_json(&self.scan_directory(name)?.join("scan.json"))
    }
    /// Every source with where its check stands, without starting one.
    pub fn sources(&self) -> anyhow::Result<Vec<SourceStatus>> {
        Ok(self
            .marketplaces()?
            .into_iter()
            .map(|source| {
                let state = self
                    .scan_state(&source.name)
                    .ok()
                    .filter(|state| state.source == source.source && state.version == SCAN_VERSION)
                    .unwrap_or_default();
                SourceStatus {
                    official: source.name == super::marketplace::OFFICIAL_MARKETPLACE,
                    name: source.name,
                    source: source.source,
                    enabled: source.enabled,
                    status: state.status,
                    completed: state.completed,
                    total: state.total,
                    checked_at: state.checked_at,
                    error: state.error,
                }
            })
            .collect())
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
        if refresh || source_changed || state.version != SCAN_VERSION {
            state = MarketplaceScan {
                revision: revision(),
                source,
                version: SCAN_VERSION,
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
                state.error = Some(format!("{error:#}"));
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
        let pending: Vec<_> = names
            .into_iter()
            .filter(|plugin| {
                !state.checks.contains_key(plugin) && !state.errors.contains_key(plugin)
            })
            .collect();
        let mut checks = stream::iter(pending)
            .map(|plugin| async move {
                let result = self.inspect_for_scan(name, &plugin).await;
                (plugin, result)
            })
            .buffer_unordered(4);
        while let Some((plugin, result)) = checks.next().await {
            match result {
                Ok(mut preview) => {
                    preview.check_revision = Some(revision());
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
                    state.errors.insert(plugin, format!("{error:#}"));
                }
            }
            state.completed = state.checks.len() + state.errors.len();
            write_json(&directory.join("scan.json"), state)?;
            tokio::task::yield_now().await;
        }
        state.status = "ready".into();
        state.checked_at = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .ok()
            .and_then(|elapsed| u64::try_from(elapsed.as_millis()).ok());
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
    /// Waits for this entry's own check, starting or resuming the scan if needed. The rest of
    /// the catalog may still be being checked.
    async fn wait_marketplace_entry(
        &self,
        marketplace: &str,
        name: &str,
    ) -> anyhow::Result<MarketplaceScan> {
        loop {
            let state = self.start_marketplace_scan(marketplace, false)?;
            if state.checks.contains_key(name) || state.errors.contains_key(name) {
                return Ok(state);
            }
            match state.status.as_str() {
                "scanning" => tokio::time::sleep(std::time::Duration::from_millis(100)).await,
                "ready" => bail!("Plugin has no completed compatibility check"),
                _ => bail!(
                    "{}",
                    state
                        .error
                        .as_deref()
                        .unwrap_or("Compatibility scan failed; refresh to retry")
                ),
            }
        }
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
        let check_revision = preview.check_revision;
        let mut preview = ImportPreview::from_spec(preview.spec)?;
        preview.check_revision = check_revision;
        Ok(preview)
    }
    pub async fn scanned_marketplace_preview(
        &self,
        marketplace: &str,
        name: &str,
    ) -> anyhow::Result<ImportPreview> {
        let state = self.wait_marketplace_entry(marketplace, name).await?;
        self.read_scanned_preview(marketplace, name, &state)
    }
    /// Installs an entry from its saved check, whether or not the rest of the catalog is done.
    /// `expected_check` is the check that was reviewed: a newer one stops the install.
    pub async fn install_scanned(
        &self,
        scope: &str,
        marketplace: &str,
        name: &str,
        allow_partial: bool,
        expected_check: Option<&str>,
    ) -> anyhow::Result<Plugin> {
        let preview = self.scanned_marketplace_preview(marketplace, name).await?;
        if expected_check
            .is_some_and(|expected| preview.check_revision.as_deref() != Some(expected))
        {
            bail!("This plugin was checked again; reopen it before installing");
        }
        if !allow_partial && preview.report.status == "partial" {
            bail!("Plugin contains unsupported components; explicitly select partial installation");
        }
        preview.spec.ensure_installable()?;
        self.install_spec(scope, marketplace, name, preview.spec)
    }
}
