//! Loading and saving the app's JSON stores: threads, skills, automations, removals and reviews.

use super::*;

impl AppState {
    /// Persists the thread registry (best-effort: a registry write failure is
    /// reported to stderr but never fails the user operation).
    pub(super) async fn persist_registry(&self) {
        let entries: Vec<RegistryEntry> = self
            .inner
            .threads
            .read()
            .await
            .values()
            .map(RegistryEntry::from_record)
            .collect();
        if let Err(err) = write_json_file(&self.inner.registry_path, &entries) {
            eprintln!("themis: failed to persist thread registry: {err}");
        }
    }

    /// Persists the skill store (best-effort, same contract as the registry).
    pub(super) async fn persist_skills(&self) {
        let skills: Vec<Skill> = self.inner.skills.read().await.values().cloned().collect();
        if let Err(err) = write_json_file(&self.inner.skills_path, &skills) {
            eprintln!("themis: failed to persist skills: {err}");
        }
    }

    /// Persists the automation store (best-effort, same contract).
    pub(super) async fn persist_automations(&self) {
        let automations: Vec<Automation> = self
            .inner
            .automations
            .read()
            .await
            .values()
            .cloned()
            .collect();
        if let Err(err) = write_json_file(&self.inner.automations_path, &automations) {
            eprintln!("themis: failed to persist automations: {err}");
        }
    }

    /// Persists the removal history, keeping only the newest entries.
    pub(super) async fn persist_removals(&self) {
        let mut removals = self.inner.removals.write().await;
        if removals.len() > REMOVAL_HISTORY_LIMIT {
            let mut order: Vec<(String, String)> = removals
                .values()
                .map(|item| (item.removed_at.clone(), item.id.clone()))
                .collect();
            order.sort();
            for (_, id) in order.iter().take(removals.len() - REMOVAL_HISTORY_LIMIT) {
                removals.remove(id);
            }
        }
        let list: Vec<AutomationRemoval> = removals.values().cloned().collect();
        drop(removals);
        if let Err(err) = write_json_file(&self.inner.removals_path, &list) {
            eprintln!("themis: failed to persist automation removals: {err}");
        }
    }

    /// Persists the review store (best-effort, same contract).
    pub(super) async fn persist_reviews(&self) {
        let reviews: Vec<ReviewItem> = self.inner.reviews.read().await.values().cloned().collect();
        if let Err(err) = write_json_file(&self.inner.reviews_path, &reviews) {
            eprintln!("themis: failed to persist review items: {err}");
        }
    }
}

/// Atomically writes a JSON store file (temp file + rename, so a crash
/// never leaves a half-written file behind). Serves the thread registry and
/// the skill / automation / review stores alike.
pub(super) fn write_json_file<T: Serialize>(path: &Path, value: &T) -> Result<(), String> {
    if let Some(parent) = path.parent() {
        if !parent.as_os_str().is_empty() {
            std::fs::create_dir_all(parent)
                .map_err(|err| format!("failed to create store dir: {err}"))?;
        }
    }
    let text = serde_json::to_string_pretty(value)
        .map_err(|err| format!("failed to encode store: {err}"))?;
    let tmp = path.with_extension("json.tmp");
    std::fs::write(&tmp, &text).map_err(|err| format!("failed to write store: {err}"))?;
    std::fs::rename(&tmp, path).map_err(|err| format!("failed to save store: {err}"))?;
    Ok(())
}

/// Loads a `Vec<T>` store file into an id-keyed map. A missing file yields
/// an empty map; an unreadable or corrupt one reports to `report` and also
/// yields an empty map (mirroring the thread registry's boot behavior).
pub(super) fn load_store_file<T>(
    path: &Path,
    label: &str,
    report: &mut Vec<String>,
) -> HashMap<String, T>
where
    T: for<'de> Deserialize<'de>,
    T: StoreEntry,
{
    let entries: Vec<T> = match std::fs::read_to_string(path) {
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => Vec::new(),
        Err(err) => {
            report.push(format!(
                "{label} store unreadable ({err}): starting with no {label}s"
            ));
            Vec::new()
        }
        Ok(text) => match serde_json::from_str(&text) {
            Ok(entries) => entries,
            Err(err) => {
                report.push(format!(
                    "{label} store corrupt ({err}): starting with no {label}s"
                ));
                Vec::new()
            }
        },
    };
    entries
        .into_iter()
        .map(|entry| (entry.store_id().to_owned(), entry))
        .collect()
}

/// Id accessor for store entries loaded by [`load_store_file`].
pub(super) trait StoreEntry {
    fn store_id(&self) -> &str;
}

impl StoreEntry for Skill {
    fn store_id(&self) -> &str {
        &self.id
    }
}

impl StoreEntry for Automation {
    fn store_id(&self) -> &str {
        &self.id
    }
}

impl StoreEntry for AutomationRemoval {
    fn store_id(&self) -> &str {
        &self.id
    }
}

impl StoreEntry for ReviewItem {
    fn store_id(&self) -> &str {
        &self.id
    }
}

/// Loads the automation store, deferring every missed automation
/// (`next_run_at <= now` at boot) by [`BOOT_DEFERRAL_SECS`] so a restart
/// never fires a thundering herd. The deferral is persisted back.
pub(super) fn load_automations_file(
    path: &Path,
    report: &mut Vec<String>,
) -> HashMap<String, Automation> {
    let mut automations: HashMap<String, Automation> = load_store_file(path, "automation", report);
    if automations.is_empty() {
        return automations;
    }
    let now = Utc::now();
    let mut deferred = 0;
    let mut retired = 0;
    for automation in automations.values_mut() {
        if automation.provider != ProviderKind::Go && automation.enabled {
            automation.enabled = false;
            retired += 1;
        }
        if next_run_due(&automation.next_run_at, now) {
            automation.next_run_at = (now + chrono::Duration::seconds(BOOT_DEFERRAL_SECS))
                .to_rfc3339_opts(SecondsFormat::Secs, true);
            deferred += 1;
        }
    }
    if deferred > 0 {
        report.push(format!(
            "deferred {deferred} missed automation(s) by {BOOT_DEFERRAL_SECS}s"
        ));
    }
    if retired > 0 {
        report.push(format!(
            "disabled {retired} automation(s) using retired providers"
        ));
    }
    if deferred > 0 || retired > 0 {
        let entries: Vec<Automation> = automations.values().cloned().collect();
        if let Err(err) = write_json_file(path, &entries) {
            report.push(format!("failed to persist deferred automations: {err}"));
        }
    }
    automations
}

/// Loads the registry and reconciles it with the world: threads that were
/// running become idle + `recovered`. Legacy worktree metadata and files are
/// preserved for recovery; all future runs use the project checkout.
pub(super) fn load_and_reconcile(
    worktrees_root: &Path,
    registry_path: &Path,
) -> (HashMap<String, ThreadRecord>, Vec<String>) {
    let mut report = Vec::new();
    let entries: Vec<RegistryEntry> = match std::fs::read_to_string(registry_path) {
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => Vec::new(),
        Err(err) => {
            report.push(format!(
                "thread registry unreadable ({err}): starting with no threads"
            ));
            Vec::new()
        }
        Ok(text) => match serde_json::from_str(&text) {
            Ok(entries) => entries,
            Err(err) => {
                report.push(format!(
                    "thread registry corrupt ({err}): starting with no threads"
                ));
                Vec::new()
            }
        },
    };
    let mut threads = HashMap::with_capacity(entries.len());
    for entry in &entries {
        let worktree_path = entry.worktree_path.as_deref().map(PathBuf::from);
        let broken = worktree_path.as_deref().is_some_and(|path| !path.is_dir());
        if entry.was_running {
            report.push(format!(
                "thread '{}' was running at shutdown: marked idle and recovered",
                entry.id
            ));
        }
        if broken {
            report.push(format!(
                "thread '{}': worktree '{}' is missing",
                entry.id,
                entry.worktree_path.as_deref().unwrap_or_default()
            ));
        }
        let project_root = PathBuf::from(&entry.project_root);
        threads.insert(
            entry.id.clone(),
            ThreadRecord {
                id: entry.id.clone(),
                title: entry.title.clone(),
                is_git: is_git_repo(&project_root),
                project_root: project_root.clone(),
                provider: entry.provider,
                model: entry.model.clone(),
                reasoning_effort: entry.reasoning_effort.clone(),
                approval_mode: entry.approval_mode,
                running: false,
                // The persisted title stands; never retitle a restored thread.
                titled: true,
                preexisting: git_status_names(&project_root).unwrap_or_default(),
                head: git_head(&project_root),
                accepted: HashSet::new(),
                comments: Vec::new(),
                worktree_path,
                branch: entry.branch.clone(),
                base_branch: entry.base_branch.clone(),
                recovered: entry.was_running,
                merged: false,
                broken,
                skill_ids: entry.skill_ids.clone(),
            },
        );
    }
    if !entries.is_empty() {
        let reconciled: Vec<RegistryEntry> =
            threads.values().map(RegistryEntry::from_record).collect();
        if let Err(err) = write_json_file(registry_path, &reconciled) {
            report.push(format!("failed to persist reconciled registry: {err}"));
        }
    }
    // Legacy worktrees may contain user work; never prune them automatically.
    let _ = worktrees_root;
    (threads, report)
}
