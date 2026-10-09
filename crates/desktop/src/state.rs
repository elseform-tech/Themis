//! Application state and handler logic.
//!
//! [`AppState`] owns projects, threads, pending approvals, settings, and
//! secrets. Every Tauri command in [`crate::commands`] is a thin wrapper over
//! one of these plain async methods, which take an [`EventSink`] instead of
//! touching Tauri — that keeps them directly drivable from headless tests.

mod attachments;
mod automation;
mod changes;
pub(crate) mod configuration;
mod integration_tools;
mod plugins;
mod preferences;
mod projects;
mod reviews;
mod run;
mod skills;
mod threads;

use std::collections::{HashMap, HashSet, VecDeque};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::Arc;

use chrono::{DateTime, SecondsFormat, Utc};
use serde::{Deserialize, Serialize};
use themis_core::providers::GO_DEFAULT_MODEL;
use themis_core::runtime::{ConversationTurn, RunEvent, RunPolicy};

use crate::approvals::PendingMap;
use crate::diff::{git_head, git_status_names, is_git_repo};
use crate::secrets::{MemoryStore, SecretStore};
use crate::settings::SettingsStore;
use crate::sink::EventSink;
use crate::transcript::{HistoryItem, TranscriptStore};
use crate::types::{
    ApprovalDecision, Automation, AutomationInput, Diagnostics, DiagnosticsError, ProjectInfo,
    ProviderKind, ReviewItem, ReviewStatus, RunAutomationNow, RunHandle, SecretStatus, Settings,
    SettingsPatch, Skill, SkillInput, ThreadComment, ThreadEvent, ThreadEventEnvelope, ThreadInfo,
};

/// Title assigned to threads before their first message.
pub const UNTITLED_THREAD: &str = "New thread";

/// Maximum title length taken from the first message, in chars.
pub const MAX_TITLE_CHARS: usize = 60;

/// Production scheduler cadence: the background task ticks this often.
const SCHEDULER_TICK_SECS: u64 = 30;

/// Boot deferral for automations already due at startup (no thundering herd).
const BOOT_DEFERRAL_SECS: i64 = 60;

/// Marker substring of the concurrency-gate rejection, shared by the send
/// path and the automation scheduler (a lost gate race skips without
/// advancing the automation's schedule).
const GATE_SATURATED_MARKER: &str = "concurrency limit reached";

/// Maximum entries kept in the diagnostics error ring ([`AppState::record_error`]).
pub const MAX_RECENT_ERRORS: usize = 50;

/// One conversation thread.
struct ThreadRecord {
    id: String,
    title: String,
    project_root: PathBuf,
    is_git: bool,
    provider: ProviderKind,
    model: String,
    reasoning_effort: Option<String>,
    approval_mode: themis_core::configuration::ApprovalMode,
    running: bool,
    titled: bool,
    preexisting: HashSet<String>,
    /// `HEAD` at thread creation, per the bridge contract. Reserved for
    /// future HEAD-pinned diff baselines; the live diff always tracks git.
    #[allow(dead_code)]
    head: Option<String>,
    accepted: HashSet<String>,
    comments: Vec<ThreadComment>,
    /// Worktree backing this thread (`None` for non-git threads and merged ones).
    worktree_path: Option<PathBuf>,
    /// Thread branch (`themis/<short-id>`; `None` without a worktree).
    branch: Option<String>,
    /// Base revision the branch forked from.
    base_branch: Option<String>,
    /// True when the thread was running at shutdown and reconciled on boot.
    recovered: bool,
    /// Internal: the thread's work was merged and its worktree retired.
    merged: bool,
    /// Internal: the recorded worktree was missing at boot. Kept for
    /// diagnostics; live checks re-probe the directory dynamically.
    #[allow(dead_code)]
    broken: bool,
    /// Skill ids attached to this thread (resolved to skills at run time).
    skill_ids: Vec<String>,
}

/// Snapshot of the thread fields a run needs (taken under one short lock).
struct RunSnapshot {
    reasoning_effort: Option<String>,
    history: Vec<ConversationTurn>,
    approval_timeout_seconds: u32,
    approval_mode: themis_core::configuration::ApprovalMode,
    runtime_configuration: themis_core::configuration::RuntimeConfig,
    /// Tool sandbox root: the shared project checkout.
    work_root: PathBuf,
    is_git: bool,
    provider: ProviderKind,
    model: String,
    /// Skills resolved from the thread's `skill_ids` (missing ids are
    /// skipped: every mutation keeps references consistent, so a miss only
    /// follows a hand-edited store file).
    skills: Vec<themis_core::skills::Skill>,
    plugins: Vec<themis_core::plugins::Plugin>,
    skill_catalog: Vec<themis_core::skills::Skill>,
}

/// One automation run in flight: the completion registry entry that lets the
/// run-event pump attribute terminal events back to their automation.
struct AutomationRunContext {
    automation_id: String,
    thread_id: String,
    /// Thread title at spawn time, reused for the review item.
    title: String,
}

/// One persisted thread (the `threads.json` registry in the app data dir).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub(crate) struct RegistryEntry {
    pub(crate) id: String,
    title: String,
    project_root: String,
    worktree_path: Option<String>,
    branch: Option<String>,
    base_branch: Option<String>,
    provider: ProviderKind,
    #[serde(default)]
    model: String,
    #[serde(default)]
    reasoning_effort: Option<String>,
    #[serde(default)]
    was_running: bool,
    #[serde(default)]
    approval_mode: themis_core::configuration::ApprovalMode,
    #[serde(default)]
    pub(crate) skill_ids: Vec<String>,
}

impl RegistryEntry {
    fn from_record(record: &ThreadRecord) -> Self {
        Self {
            id: record.id.clone(),
            title: record.title.clone(),
            project_root: record.project_root.to_string_lossy().into_owned(),
            worktree_path: record
                .worktree_path
                .as_ref()
                .map(|path| path.to_string_lossy().into_owned()),
            branch: record.branch.clone(),
            base_branch: record.base_branch.clone(),
            provider: record.provider,
            model: record.model.clone(),
            reasoning_effort: record.reasoning_effort.clone(),
            approval_mode: record.approval_mode,
            was_running: record.running,
            skill_ids: record.skill_ids.clone(),
        }
    }
}

struct AppStateInner {
    stop_flags: std::sync::Mutex<HashMap<String, Arc<AtomicBool>>>,
    projects: tokio::sync::RwLock<HashMap<String, ProjectInfo>>,
    default_project_init: tokio::sync::Mutex<()>,
    threads: tokio::sync::RwLock<HashMap<String, ThreadRecord>>,
    pending: PendingMap,
    settings: SettingsStore,
    transcript: TranscriptStore,
    diagnostics: Arc<themis_core::diagnostics::DiagnosticLog>,
    secrets: Arc<dyn SecretStore>,
    go_base_url_override: std::sync::Mutex<Option<String>>,
    go_catalog: tokio::sync::RwLock<Vec<themis_core::providers::GoModel>>,
    worktrees_root: PathBuf,
    registry_path: PathBuf,
    /// Live run count for the global concurrency gate.
    running_count: AtomicUsize,
    /// Boot reconciliation notes (recovered threads, pruned/left dirs).
    reconcile_report: std::sync::Mutex<Vec<String>>,
    /// Skill store (`skills.json` in the app data dir).
    skills: tokio::sync::RwLock<HashMap<String, Skill>>,
    skills_path: PathBuf,
    /// Automation store (`automations.json` in the app data dir).
    automations: tokio::sync::RwLock<HashMap<String, Automation>>,
    automations_path: PathBuf,
    /// Review store (`review_items.json` in the app data dir).
    reviews: tokio::sync::RwLock<HashMap<String, ReviewItem>>,
    reviews_path: PathBuf,
    /// In-flight automation runs (`run_id` -> context). Deliberately
    /// in-memory only: a run killed mid-flight by a shutdown has no mapping
    /// post-boot, so it yields no review item (documented recovery rule).
    automation_runs: std::sync::Mutex<HashMap<String, AutomationRunContext>>,
    /// Sink the background scheduler emits on (registered post-construction;
    /// the scheduler skips ticks until one is set).
    scheduler_sink: std::sync::Mutex<Option<Arc<dyn EventSink>>>,
    /// Recent command failures, oldest-first, capped at [`MAX_RECENT_ERRORS`].
    recent_errors: std::sync::Mutex<VecDeque<DiagnosticsError>>,
}

/// Shared application state (cheap to clone; everything lives behind one `Arc`).
#[derive(Clone)]
pub struct AppState {
    inner: Arc<AppStateInner>,
}

// Manual `Debug`: the secret store must never appear in debug output.
impl std::fmt::Debug for AppState {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.debug_struct("AppState").finish_non_exhaustive()
    }
}

impl AppState {
    /// Creates state with `settings_path` and `secrets` (production uses the
    /// app data dir plus the OS credential store). The thread registry, the skill /
    /// automation / review stores, and the worktrees root live beside the
    /// settings file; the registry is reconciled synchronously during
    /// construction, and the automation scheduler is spawned on the server
    /// async runtime (ticking every [`SCHEDULER_TICK_SECS`]; it stays quiet
    /// until [`AppState::set_scheduler_sink`] registers a sink).
    #[must_use]
    pub fn new(settings_path: PathBuf, secrets: Arc<dyn SecretStore>) -> Self {
        let app_dir = settings_app_dir(&settings_path);
        let worktrees_root = app_dir.join("worktrees");
        let registry_path = app_dir.join("threads.json");
        let state = Self::new_with_dirs(
            settings_path,
            worktrees_root,
            registry_path,
            secrets,
            PathBuf::from(crate::types::default_projects_directory()),
        );
        Self::spawn_automation_scheduler(state.clone());
        state
    }

    /// Creates state for headless tests: in-memory secrets, explicit settings
    /// path. Unlike [`AppState::new`], this never spawns the scheduler;
    /// tests drive [`AppState::tick_automations_once`] directly.
    #[must_use]
    pub fn new_for_test(settings_path: PathBuf) -> Self {
        let app_dir = settings_app_dir(&settings_path);
        let worktrees_root = app_dir.join("worktrees");
        let registry_path = app_dir.join("threads.json");
        Self::new_with_dirs(
            settings_path,
            worktrees_root,
            registry_path,
            Arc::new(MemoryStore::new()),
            app_dir.join("projects"),
        )
    }

    /// Creates state for headless tests with an explicit worktrees root (the
    /// registry stays beside the settings file). Never spawns the scheduler.
    #[must_use]
    pub fn new_for_test_with_worktrees_root(
        settings_path: PathBuf,
        worktrees_root: PathBuf,
    ) -> Self {
        let app_dir = settings_app_dir(&settings_path);
        let registry_path = app_dir.join("threads.json");
        Self::new_with_dirs(
            settings_path,
            worktrees_root,
            registry_path,
            Arc::new(MemoryStore::new()),
            app_dir.join("projects"),
        )
    }

    fn new_with_dirs(
        settings_path: PathBuf,
        worktrees_root: PathBuf,
        registry_path: PathBuf,
        secrets: Arc<dyn SecretStore>,
        projects_root: PathBuf,
    ) -> Self {
        let app_dir = settings_app_dir(&settings_path);
        let transcript = TranscriptStore::open(&app_dir.join("sessions.sqlite3"))
            .expect("unable to open durable session store");
        let (threads, mut report) = load_and_reconcile(&worktrees_root, &registry_path);
        let skills_path = app_dir.join("skills.json");
        let automations_path = app_dir.join("automations.json");
        let reviews_path = app_dir.join("review_items.json");
        let skills = load_store_file(&skills_path, "skill", &mut report);
        let automations = load_automations_file(&automations_path, &mut report);
        let reviews = load_store_file(&reviews_path, "review item", &mut report);
        let diagnostics = Arc::new(themis_core::diagnostics::DiagnosticLog::new(
            themis_core::diagnostics::directory_for(&app_dir),
        ));
        if let Ok(content) = std::fs::read_to_string(app_dir.join("runtime.jsonc")) {
            if let Ok(config) = themis_core::configuration::resolve(Some(&content), None, None) {
                diagnostics.set_level(&config.config.logging.level);
            }
        }
        Self {
            inner: Arc::new(AppStateInner {
                stop_flags: std::sync::Mutex::new(HashMap::new()),
                projects: tokio::sync::RwLock::new(HashMap::new()),
                default_project_init: tokio::sync::Mutex::new(()),
                threads: tokio::sync::RwLock::new(threads),
                pending: PendingMap::default(),
                settings: SettingsStore::load_with_projects_root(settings_path, projects_root),
                transcript,
                diagnostics,
                secrets,
                go_base_url_override: std::sync::Mutex::new(None),
                go_catalog: tokio::sync::RwLock::new(Vec::new()),
                worktrees_root,
                registry_path,
                running_count: AtomicUsize::new(0),
                reconcile_report: std::sync::Mutex::new(report),
                skills: tokio::sync::RwLock::new(skills),
                skills_path,
                automations: tokio::sync::RwLock::new(automations),
                automations_path,
                reviews: tokio::sync::RwLock::new(reviews),
                reviews_path,
                automation_runs: std::sync::Mutex::new(HashMap::new()),
                scheduler_sink: std::sync::Mutex::new(None),
                recent_errors: std::sync::Mutex::new(VecDeque::new()),
            }),
        }
    }

    /// Spawns the background automation scheduler (production only).
    fn spawn_automation_scheduler(state: AppState) {
        tauri::async_runtime::spawn(async move {
            loop {
                tokio::time::sleep(std::time::Duration::from_secs(SCHEDULER_TICK_SECS)).await;
                if let Some(sink) = state.scheduler_sink() {
                    state.tick_automations_once(&sink).await;
                }
            }
        });
    }

    /// Registers the sink the background scheduler emits on (production
    /// wires the Tauri sink right after construction).
    pub fn set_scheduler_sink(&self, sink: Arc<dyn EventSink>) {
        *self
            .inner
            .scheduler_sink
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner()) = Some(sink);
    }

    fn scheduler_sink(&self) -> Option<Arc<dyn EventSink>> {
        self.inner
            .scheduler_sink
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .clone()
    }

    /// Worktrees root backing this state (test/debug accessor).
    #[must_use]
    pub fn worktrees_root(&self) -> &Path {
        &self.inner.worktrees_root
    }

    /// Boot reconciliation notes: recovered threads, pruned or left-behind dirs.
    #[must_use]
    pub fn reconcile_report(&self) -> Vec<String> {
        self.inner
            .reconcile_report
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .clone()
    }

    /// Records a command failure in the diagnostics ring (oldest-first,
    /// capped at [`MAX_RECENT_ERRORS`]; the oldest entry is evicted on
    /// overflow). Every fallible Tauri command reports its `Err` here via
    /// the `record_cmd!` helper in [`crate::commands`], attributing its own
    /// command name.
    pub fn record_error(&self, command: &str, message: String) {
        let _ = self.inner.diagnostics.append(
            "server",
            "command_failed",
            "error",
            serde_json::json!({"method": command}),
        );
        let entry = DiagnosticsError {
            at: Utc::now().to_rfc3339_opts(SecondsFormat::Secs, true),
            command: command.to_owned(),
            message,
        };
        let mut ring = self
            .inner
            .recent_errors
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        ring.push_back(entry);
        while ring.len() > MAX_RECENT_ERRORS {
            ring.pop_front();
        }
    }

    /// Returns the recorded command failures, oldest-first (newest last).
    #[must_use]
    pub fn recent_errors(&self) -> Vec<DiagnosticsError> {
        self.inner
            .recent_errors
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .iter()
            .cloned()
            .collect()
    }

    /// Persists the thread registry (best-effort: a registry write failure is
    /// reported to stderr but never fails the user operation).
    async fn persist_registry(&self) {
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
    async fn persist_skills(&self) {
        let skills: Vec<Skill> = self.inner.skills.read().await.values().cloned().collect();
        if let Err(err) = write_json_file(&self.inner.skills_path, &skills) {
            eprintln!("themis: failed to persist skills: {err}");
        }
    }

    /// Persists the automation store (best-effort, same contract).
    async fn persist_automations(&self) {
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

    /// Persists the review store (best-effort, same contract).
    async fn persist_reviews(&self) {
        let reviews: Vec<ReviewItem> = self.inner.reviews.read().await.values().cloned().collect();
        if let Err(err) = write_json_file(&self.inner.reviews_path, &reviews) {
            eprintln!("themis: failed to persist review items: {err}");
        }
    }

    /// Health check: returns the `themis-core` version.
    pub async fn ping(&self) -> String {
        themis_core::version().to_owned()
    }

    /// Creates a thread on `project_root`.
    /// Checks the Go connection and returns the current provider model catalog.
    pub async fn list_go_models(&self) -> Result<Vec<themis_core::providers::GoModel>, String> {
        let key = self.api_key_for(ProviderKind::Go)?;
        let base_url = self
            .inner
            .go_base_url_override
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .clone()
            .unwrap_or_else(|| themis_core::providers::GO_BASE_URL.to_owned());
        let models = themis_core::providers::refresh_go_catalog(&base_url, &key)
            .await
            .map_err(|error| format!("Could not load OpenCode Go models: {error}"))?;
        *self.inner.go_catalog.write().await = models.clone();
        Ok(models)
    }

    /// Stores a comment pinned to `path` and returns it.
    pub async fn add_comment(
        &self,
        thread_id: String,
        path: String,
        comment: String,
    ) -> Result<ThreadComment, String> {
        let mut threads = self.inner.threads.write().await;
        let record = threads
            .get_mut(&thread_id)
            .ok_or_else(|| format!("unknown thread '{thread_id}'"))?;
        let entry = ThreadComment {
            id: uuid::Uuid::new_v4().to_string(),
            path,
            comment,
        };
        record.comments.push(entry.clone());
        Ok(entry)
    }

    pub fn pending_approvals(&self) -> Vec<crate::types::ApprovalRequest> {
        self.inner
            .pending
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .values()
            .map(|entry| entry.request.clone())
            .collect()
    }

    /// Delivers a UI decision to a pending approval dialog.
    pub async fn approve_action(
        &self,
        thread_id: String,
        approval_id: String,
        decision: ApprovalDecision,
    ) -> Result<(), String> {
        {
            let threads = self.inner.threads.read().await;
            if !threads.contains_key(&thread_id) {
                return Err(format!("unknown thread '{thread_id}'"));
            }
        }
        let sender = {
            let mut pending = self
                .inner
                .pending
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            match pending.remove(&approval_id) {
                Some(entry) if entry.thread_id == thread_id => entry.sender,
                Some(_) => {
                    return Err(format!(
                        "approval '{approval_id}' does not belong to thread '{thread_id}'"
                    ));
                }
                None => {
                    return Err(format!("unknown or expired approval '{approval_id}'"));
                }
            }
        };
        sender
            .send(decision.core_approval())
            .map_err(|_| format!("approval '{approval_id}' already expired"))?;
        Ok(())
    }

    /// Opens `path` in the OS default editor (best-effort; the platform
    /// openers take no line argument, so `_line` is accepted and ignored).
    pub async fn open_in_editor(&self, path: String, _line: Option<u32>) -> Result<(), String> {
        if path.trim().is_empty() {
            return Err("path must not be empty".to_owned());
        }
        let target = PathBuf::from(path);
        if !target.exists() {
            return Err(format!("path '{}' does not exist", target.display()));
        }
        spawn_opener(&target)
    }
}

fn thread_info(record: &ThreadRecord, transcript: &TranscriptStore) -> Result<ThreadInfo, String> {
    Ok(ThreadInfo {
        last_activity_seq: transcript.last_activity_seq(&record.id)?,
        id: record.id.clone(),
        title: record.title.clone(),
        provider: record.provider,
        model: record.model.clone(),
        reasoning_effort: record.reasoning_effort.clone(),
        approval_mode: record.approval_mode,
        running: record.running,
        worktree_path: None,
        branch: None,
        base_branch: None,
        recovered: record.recovered,
        skill_ids: record.skill_ids.clone(),
    })
}

/// App data dir holding the settings file (`.` when the path has no parent).
fn settings_app_dir(settings_path: &Path) -> PathBuf {
    settings_path
        .parent()
        .map(Path::to_path_buf)
        .filter(|parent| !parent.as_os_str().is_empty())
        .unwrap_or_else(|| PathBuf::from("."))
}

/// Atomically writes a JSON store file (temp file + rename, so a crash
/// never leaves a half-written file behind). Serves the thread registry and
/// the skill / automation / review stores alike.
fn write_json_file<T: Serialize>(path: &Path, value: &T) -> Result<(), String> {
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
fn load_store_file<T>(path: &Path, label: &str, report: &mut Vec<String>) -> HashMap<String, T>
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
trait StoreEntry {
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

impl StoreEntry for ReviewItem {
    fn store_id(&self) -> &str {
        &self.id
    }
}

/// Loads the automation store, deferring every missed automation
/// (`next_run_at <= now` at boot) by [`BOOT_DEFERRAL_SECS`] so a restart
/// never fires a thundering herd. The deferral is persisted back.
fn load_automations_file(path: &Path, report: &mut Vec<String>) -> HashMap<String, Automation> {
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
fn load_and_reconcile(
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

fn canonical_project_dir(path: &str) -> Result<PathBuf, String> {
    if path.trim().is_empty() {
        return Err("project path must not be empty".to_owned());
    }
    let root = PathBuf::from(path.trim())
        .canonicalize()
        .map_err(|err| format!("cannot open project '{path}': {err}"))?;
    if !root.is_dir() {
        return Err(format!("cannot open project '{path}': not a directory"));
    }
    Ok(root)
}

fn project_name(root: &Path) -> String {
    root.file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_default()
}

fn title_from_message(text: &str) -> String {
    let first = text.lines().next().unwrap_or_default().trim();
    if first.is_empty() {
        return UNTITLED_THREAD.to_owned();
    }
    first.chars().take(MAX_TITLE_CHARS).collect()
}

/// Only the OpenCode Go credential is accepted.
fn secret_key(provider: &str) -> Result<&'static str, String> {
    match provider.trim().to_lowercase().as_str() {
        "go" => Ok("go"),
        other => Err(format!("unknown secret provider '{other}': expected go")),
    }
}

/// Dedups ids while keeping first-seen order.
fn dedup_ids(ids: Vec<String>) -> Vec<String> {
    let mut seen = HashSet::new();
    ids.into_iter()
        .filter(|id| seen.insert(id.clone()))
        .collect()
}

/// Formats `now + minutes` as an RFC3339 UTC timestamp.
fn rfc3339_plus_minutes(now: chrono::DateTime<Utc>, minutes: i64) -> String {
    (now + chrono::Duration::minutes(minutes)).to_rfc3339_opts(SecondsFormat::Secs, true)
}

/// True when `next_run_at` is due at `now`. Unparseable stamps count as due
/// (a corrupt schedule must fire visibly rather than stall silently).
fn next_run_due(next_run_at: &str, now: chrono::DateTime<Utc>) -> bool {
    match DateTime::parse_from_rfc3339(next_run_at) {
        Ok(stamp) => stamp <= now,
        Err(_) => true,
    }
}

#[cfg(target_os = "macos")]
fn spawn_opener(target: &Path) -> Result<(), String> {
    std::process::Command::new("open")
        .arg(target)
        .spawn()
        .map(|_| ())
        .map_err(|err| format!("failed to open '{}': {err}", target.display()))
}

#[cfg(target_os = "linux")]
fn spawn_opener(target: &Path) -> Result<(), String> {
    std::process::Command::new("xdg-open")
        .arg(target)
        .spawn()
        .map(|_| ())
        .map_err(|err| format!("failed to open '{}': {err}", target.display()))
}

#[cfg(target_os = "windows")]
fn spawn_opener(target: &Path) -> Result<(), String> {
    std::process::Command::new("cmd")
        .args(["/C", "start", ""])
        .arg(target)
        .spawn()
        .map(|_| ())
        .map_err(|err| format!("failed to open '{}': {err}", target.display()))
}

#[cfg(not(any(target_os = "macos", target_os = "linux", target_os = "windows")))]
fn spawn_opener(target: &Path) -> Result<(), String> {
    Err(format!(
        "opening '{}' is not supported on this platform",
        target.display()
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sink::TestEvent;
    use crate::types::{SettingsPatch, ThemeMode};

    fn test_state() -> (AppState, tempfile::TempDir) {
        let dir = tempfile::tempdir().expect("tempdir");
        let state = AppState::new_for_test(dir.path().join("settings.json"));
        (state, dir)
    }

    #[tokio::test]
    async fn open_project_rejects_missing_or_file_paths() {
        let (state, dir) = test_state();
        assert!(state.open_project(String::new()).await.is_err());
        assert!(state
            .open_project(dir.path().join("nope").to_string_lossy().into_owned())
            .await
            .is_err());
        let file = dir.path().join("file.txt");
        std::fs::write(&file, "x").expect("write");
        assert!(state
            .open_project(file.to_string_lossy().into_owned())
            .await
            .is_err());
        let info = state
            .open_project(dir.path().to_string_lossy().into_owned())
            .await
            .expect("open");
        assert!(!info.root.is_empty());
        assert!(!info.is_git);
    }

    #[tokio::test]
    async fn unknown_ids_error_clearly() {
        let (state, _dir) = test_state();
        for result in [
            state.list_diff("nope".to_owned()).await.map(|_| ()),
            state.accept_file("nope".to_owned(), "a".to_owned()).await,
            state.discard_file("nope".to_owned(), "a".to_owned()).await,
            state
                .add_comment("nope".to_owned(), "a".to_owned(), "c".to_owned())
                .await
                .map(|_| ()),
            state
                .set_provider("nope".to_owned(), ProviderKind::Go, None)
                .await
                .map(|_| ()),
            state.get_thread("nope").await.map(|_| ()),
            state.merge_thread("nope".to_owned()).await.map(|_| ()),
            state.discard_thread("nope".to_owned()).await,
        ] {
            let err = result.expect_err("unknown thread");
            assert!(err.contains("unknown thread"), "{err}");
        }
    }

    #[tokio::test]
    async fn non_git_threads_have_no_worktree_and_drop_cleanly() {
        let (state, dir) = test_state();
        let project = dir.path().join("plain");
        std::fs::create_dir(&project).expect("mkdir");
        let thread = state
            .create_thread(
                project.to_string_lossy().into_owned(),
                ProviderKind::Go,
                None,
            )
            .await
            .expect("create");
        assert!(thread.worktree_path.is_none());
        assert!(thread.branch.is_none());
        assert!(thread.base_branch.is_none());
        assert!(!thread.recovered);
        let err = state
            .merge_thread(thread.id.clone())
            .await
            .expect_err("non-git merge rejected");
        assert!(err.contains("no git worktree"), "{err}");
        state
            .discard_thread(thread.id.clone())
            .await
            .expect("non-git discard drops");
        assert!(state.get_thread(&thread.id).await.is_err());
    }

    #[tokio::test]
    async fn list_threads_filters_by_project() {
        let (state, dir) = test_state();
        let first = dir.path().join("first");
        let second = dir.path().join("second");
        std::fs::create_dir(&first).expect("mkdir");
        std::fs::create_dir(&second).expect("mkdir");
        let thread_a = state
            .create_thread(first.to_string_lossy().into_owned(), ProviderKind::Go, None)
            .await
            .expect("create");
        let thread_b = state
            .create_thread(
                second.to_string_lossy().into_owned(),
                ProviderKind::Go,
                None,
            )
            .await
            .expect("create");
        let listed = state
            .list_threads(first.to_string_lossy().into_owned())
            .await
            .expect("list");
        assert_eq!(listed.len(), 1);
        assert_eq!(listed[0].id, thread_a.id);
        let listed = state
            .list_threads(second.to_string_lossy().into_owned())
            .await
            .expect("list");
        assert_eq!(listed.len(), 1);
        assert_eq!(listed[0].id, thread_b.id);
        let empty = dir.path().join("empty");
        std::fs::create_dir(&empty).expect("mkdir");
        let listed = state
            .list_threads(empty.to_string_lossy().into_owned())
            .await
            .expect("list");
        assert!(listed.is_empty());
        assert!(state.list_threads(String::new()).await.is_err());
    }

    #[tokio::test]
    async fn open_project_tracks_recent_roots() {
        let (state, dir) = test_state();
        let first = dir.path().join("first");
        let second = dir.path().join("second");
        std::fs::create_dir(&first).expect("mkdir");
        std::fs::create_dir(&second).expect("mkdir");
        state
            .open_project(first.to_string_lossy().into_owned())
            .await
            .expect("open");
        state
            .open_project(second.to_string_lossy().into_owned())
            .await
            .expect("open");
        state
            .open_project(first.to_string_lossy().into_owned())
            .await
            .expect("reopen");
        let settings = state.get_settings().await;
        let first_root = first.canonicalize().expect("canonical");
        let second_root = second.canonicalize().expect("canonical");
        assert_eq!(
            settings.recent_roots,
            vec![
                first_root.to_string_lossy().into_owned(),
                second_root.to_string_lossy().into_owned(),
            ]
        );
    }

    #[tokio::test]
    async fn create_thread_persists_the_registry() {
        let (state, dir) = test_state();
        let thread = state
            .create_thread(
                dir.path().to_string_lossy().into_owned(),
                ProviderKind::Go,
                Some("gpt-x".to_owned()),
            )
            .await
            .expect("create");
        let registry = dir.path().join("threads.json");
        assert!(registry.is_file(), "registry was written");
        let text = std::fs::read_to_string(&registry).expect("read");
        assert!(text.contains(&thread.id), "{text}");
        assert!(text.contains("\"was_running\": false"), "{text}");
        // A reboot from the same dir restores the thread, idle and unrecovered.
        drop(state);
        let rebooted = AppState::new_for_test(dir.path().join("settings.json"));
        let info = rebooted.get_thread(&thread.id).await.expect("restored");
        assert_eq!(info.title, UNTITLED_THREAD);
        assert!(!info.running);
        assert!(!info.recovered);
        assert!(rebooted.reconcile_report().is_empty());
    }

    #[tokio::test]
    async fn thread_effort_follows_catalog_and_survives_restart() {
        let (state, dir) = test_state();
        let thread = state
            .create_thread(
                dir.path().to_string_lossy().into_owned(),
                ProviderKind::Go,
                Some("model-a".to_owned()),
            )
            .await
            .expect("create");
        *state.inner.go_catalog.write().await = vec![themis_core::providers::GoModel {
            id: "model-a".to_owned(),
            effort_levels: vec!["low".to_owned(), "high".to_owned()],
        }];
        assert!(state
            .set_thread_effort(thread.id.clone(), Some("max".to_owned()))
            .await
            .is_err());
        let updated = state
            .set_thread_effort(thread.id.clone(), Some("high".to_owned()))
            .await
            .expect("catalog effort");
        assert_eq!(updated.reasoning_effort.as_deref(), Some("high"));
        drop(state);
        let rebooted = AppState::new_for_test(dir.path().join("settings.json"));
        let restored = rebooted.get_thread(&thread.id).await.expect("restored");
        assert_eq!(restored.reasoning_effort.as_deref(), Some("high"));
        let cleared = rebooted
            .set_thread_effort(thread.id.clone(), Some(String::new()))
            .await
            .expect("default effort");
        assert_eq!(cleared.reasoning_effort, None);
    }

    #[tokio::test]
    async fn send_message_requires_a_stored_key_and_resets_running() {
        let (state, dir) = test_state();
        let thread = state
            .create_thread(
                dir.path().to_string_lossy().into_owned(),
                ProviderKind::Go,
                Some("gpt-x".to_owned()),
            )
            .await
            .expect("create");
        let (sink, _rx) = crate::sink::ChannelSink::channel();
        let err = state
            .send_message(Arc::new(sink), thread.id.clone(), "hi".to_owned())
            .await
            .expect_err("missing key");
        assert!(err.contains("no API key"), "{err}");
        // The failure path resets `running`: a retry fails the same way, not busy.
        let (sink, _rx) = crate::sink::ChannelSink::channel();
        let retry = state
            .send_message(Arc::new(sink), thread.id.clone(), "hi".to_owned())
            .await
            .expect_err("missing key again");
        assert!(retry.contains("no API key"), "{retry}");
        assert!(!state.get_thread(&thread.id).await.expect("info").running);
    }

    #[tokio::test]
    async fn go_uses_default_model() {
        let (state, dir) = test_state();
        let thread = state
            .create_thread(
                dir.path().to_string_lossy().into_owned(),
                ProviderKind::Go,
                None,
            )
            .await
            .expect("create");
        assert_eq!(thread.model, GO_DEFAULT_MODEL);
    }

    #[tokio::test]
    async fn first_message_sets_a_truncated_title() {
        let (state, dir) = test_state();
        state
            .set_secret("go".to_owned(), "synthetic-test-key".to_owned())
            .await
            .unwrap();
        let thread = state
            .create_thread(
                dir.path().to_string_lossy().into_owned(),
                ProviderKind::Go,
                Some("test-model".to_owned()),
            )
            .await
            .expect("create");
        assert_eq!(thread.title, UNTITLED_THREAD);
        // The run may fail without a mock endpoint; title assignment is synchronous.
        let (sink, _rx) = crate::sink::ChannelSink::channel();
        let long = format!("{} and more words after the limit", "w".repeat(80));
        let handle = state
            .send_message(Arc::new(sink), thread.id.clone(), long)
            .await
            .expect("spawn");
        assert!(!handle.run_id.is_empty());
        // The title is set synchronously, before the run resolves.
        let info = state.get_thread(&thread.id).await.expect("info");
        assert_eq!(info.title.chars().count(), MAX_TITLE_CHARS);
        assert!(info.title.chars().all(|char| char == 'w' || char == ' '));
        // Busy-rejection is covered deterministically in tests/bridge_e2e.rs,
        // where the first run blocks on an approval dialog.
    }

    #[tokio::test]
    async fn approve_action_rejects_unknown_ids() {
        let (state, dir) = test_state();
        let thread = state
            .create_thread(
                dir.path().to_string_lossy().into_owned(),
                ProviderKind::Go,
                None,
            )
            .await
            .expect("create");
        let err = state
            .approve_action(
                thread.id.clone(),
                "missing".to_owned(),
                ApprovalDecision::Once,
            )
            .await
            .expect_err("unknown approval");
        assert!(err.contains("unknown or expired"), "{err}");
        let err = state
            .approve_action(
                "missing-thread".to_owned(),
                "missing".to_owned(),
                ApprovalDecision::Deny,
            )
            .await
            .expect_err("unknown thread");
        assert!(err.contains("unknown thread"), "{err}");
    }

    #[tokio::test]
    async fn comments_and_settings_flow() {
        let (state, dir) = test_state();
        let thread = state
            .create_thread(
                dir.path().to_string_lossy().into_owned(),
                ProviderKind::Go,
                None,
            )
            .await
            .expect("create");
        let comment = state
            .add_comment(thread.id.clone(), "a.rs".to_owned(), "look".to_owned())
            .await
            .expect("comment");
        assert_eq!(comment.path, "a.rs");
        assert_eq!(comment.comment, "look");
        assert!(!comment.id.is_empty());
        // `accept_file` records without touching the disk.
        state
            .accept_file(thread.id.clone(), "a.rs".to_owned())
            .await
            .expect("accept");

        let settings = state.get_settings().await;
        assert_eq!(
            settings,
            Settings {
                projects_directory: settings_app_dir(state.inner.settings.path())
                    .join("projects")
                    .to_string_lossy()
                    .into_owned(),
                ..Settings::default()
            }
        );
        let updated = state
            .update_settings(SettingsPatch {
                theme: Some(ThemeMode::Light),
                max_turns: Some(7),
                ..SettingsPatch::default()
            })
            .await
            .expect("update");
        assert_eq!(updated.theme, ThemeMode::Dark);
        assert_eq!(updated.max_turns, 7);
        let bad = state
            .update_settings(SettingsPatch {
                max_turns: Some(0),
                ..SettingsPatch::default()
            })
            .await
            .expect_err("validated");
        assert!(bad.contains("max_turns"), "{bad}");
    }

    #[tokio::test]
    async fn secrets_never_surface_values() {
        let (state, _dir) = test_state();
        let status = state.get_secret_status().await;
        assert_eq!(status, SecretStatus { go: false });
        assert!(state
            .set_secret("go".to_owned(), String::new())
            .await
            .is_err());
        assert!(state
            .set_secret("ollama".to_owned(), "x".to_owned())
            .await
            .is_err());
        state
            .set_secret("go".to_owned(), "sk-test".to_owned())
            .await
            .expect("set");
        let status = state.get_secret_status().await;
        assert!(status.go);
        // The status JSON carries presence only.
        let json = serde_json::to_string(&status).expect("json");
        assert!(!json.contains("sk-test"));
        state.clear_secret("go".to_owned()).await.expect("clear");
        assert!(!state.get_secret_status().await.go);
    }

    #[tokio::test]
    async fn open_in_editor_validates_paths() {
        let (state, dir) = test_state();
        assert!(state.open_in_editor(String::new(), None).await.is_err());
        assert!(state
            .open_in_editor(
                dir.path().join("missing").to_string_lossy().into_owned(),
                Some(3),
            )
            .await
            .is_err());
    }

    #[tokio::test]
    async fn non_git_diff_is_unavailable() {
        let (state, dir) = test_state();
        let thread = state
            .create_thread(
                dir.path().to_string_lossy().into_owned(),
                ProviderKind::Go,
                None,
            )
            .await
            .expect("create");
        let diff = state.list_diff(thread.id).await.expect("diff");
        assert!(!diff.available);
        assert!(diff.reason.is_some());
        assert!(diff.files.is_empty());
    }

    #[tokio::test]
    async fn discard_rejects_escapes() {
        let (state, dir) = test_state();
        let thread = state
            .create_thread(
                dir.path().to_string_lossy().into_owned(),
                ProviderKind::Go,
                None,
            )
            .await
            .expect("create");
        for bad in ["../evil.txt", "/abs/path.txt", "sub/../../evil.txt"] {
            let err = state
                .discard_file(thread.id.clone(), bad.to_owned())
                .await
                .expect_err("escape rejected");
            assert!(err.contains("relative"), "{err}");
        }
    }

    fn skill_input(name: &str) -> SkillInput {
        SkillInput {
            name: name.to_owned(),
            description: format!("{name} description"),
            instructions: "Be helpful.".to_owned(),
            allowed_tools: Vec::new(),
            scripts: Vec::new(),
        }
    }

    fn automation_input(project_root: &str) -> AutomationInput {
        AutomationInput {
            name: "Nightly".to_owned(),
            project_root: project_root.to_owned(),
            provider: ProviderKind::Go,
            model: "test-model".to_owned(),
            reasoning_effort: None,
            target_thread_id: None,
            skill_ids: Vec::new(),
            interval_mins: 60,
            schedule: None,
            task: "Check health.".to_owned(),
            enabled: true,
        }
    }

    fn test_sink() -> (
        Arc<dyn EventSink>,
        tokio::sync::mpsc::UnboundedReceiver<TestEvent>,
    ) {
        let (sink, rx) = crate::sink::ChannelSink::channel();
        (Arc::new(sink), rx)
    }

    #[tokio::test]
    async fn skill_crud_validates_and_persists() {
        let (state, dir) = test_state();
        assert!(state.list_skills().await.is_empty());

        // Unknown tools fail with an error naming the bad tool AND the valid names.
        let mut bad = skill_input("Bad");
        bad.allowed_tools = vec!["bogus_tool".to_owned()];
        let err = state
            .create_skill(bad)
            .await
            .expect_err("unknown tool rejected");
        assert!(err.contains("bogus_tool"), "{err}");
        for valid in ["read_file", "write_file", "shell", "git", "list_dir"] {
            assert!(err.contains(valid), "{valid} missing from: {err}");
        }
        // Empty names and unsafe script names fail too.
        assert!(state.create_skill(skill_input("   ")).await.is_err());
        let mut bad_script = skill_input("BadScript");
        bad_script.scripts = vec![crate::types::SkillScript {
            name: "../escape.sh".to_owned(),
            content: "pwn".to_owned(),
        }];
        let err = state
            .create_skill(bad_script)
            .await
            .expect_err("unsafe script rejected");
        assert!(err.contains("unsafe script name"), "{err}");

        let created = state
            .create_skill(skill_input("Rust"))
            .await
            .expect("create");
        assert!(!created.id.is_empty());
        assert_eq!(created.name, "Rust");
        assert_eq!(state.list_skills().await.len(), 1);

        let mut updated_input = skill_input("Rustacean");
        updated_input.allowed_tools = vec!["read_file".to_owned()];
        let updated = state
            .update_skill(created.id.clone(), updated_input)
            .await
            .expect("update");
        assert_eq!(updated.id, created.id);
        assert_eq!(updated.name, "Rustacean");
        assert_eq!(updated.allowed_tools, vec!["read_file".to_owned()]);
        assert!(state
            .update_skill("missing".to_owned(), skill_input("X"))
            .await
            .expect_err("unknown skill")
            .contains("unknown skill"));
        assert!(state
            .delete_skill("missing".to_owned())
            .await
            .expect_err("unknown skill")
            .contains("unknown skill"));

        // The store round-trips through a reboot.
        drop(state);
        let rebooted = AppState::new_for_test(dir.path().join("settings.json"));
        let skills = rebooted.list_skills().await;
        assert_eq!(skills.len(), 1);
        assert_eq!(skills[0].id, created.id);
        assert_eq!(skills[0].name, "Rustacean");
    }

    #[tokio::test]
    async fn set_thread_skills_validates_ids_and_persists() {
        let (state, dir) = test_state();
        let project = dir.path().to_string_lossy().into_owned();
        let thread = state
            .create_thread(project, ProviderKind::Go, None)
            .await
            .expect("create");
        assert!(thread.skill_ids.is_empty());
        let skill = state
            .create_skill(skill_input("Rust"))
            .await
            .expect("skill");

        // Unknown ids fail, naming the bad id; valid refs are untouched.
        let err = state
            .set_thread_skills(thread.id.clone(), vec!["nope-1".to_owned()])
            .await
            .expect_err("unknown id rejected");
        assert!(err.contains("nope-1"), "{err}");
        let err = state
            .set_thread_skills("missing".to_owned(), vec![skill.id.clone()])
            .await
            .expect_err("unknown thread");
        assert!(err.contains("unknown thread"), "{err}");

        let info = state
            .set_thread_skills(thread.id.clone(), vec![skill.id.clone(), skill.id.clone()])
            .await
            .expect("set");
        assert_eq!(info.skill_ids, vec![skill.id.clone()]);
        assert_eq!(
            state.get_thread(&thread.id).await.expect("info").skill_ids,
            vec![skill.id.clone()]
        );
        // Clearing works too.
        let info = state
            .set_thread_skills(thread.id.clone(), Vec::new())
            .await
            .expect("clear");
        assert!(info.skill_ids.is_empty());

        // Reattach, then reboot: the reference survives.
        state
            .set_thread_skills(thread.id.clone(), vec![skill.id.clone()])
            .await
            .expect("reattach");
        drop(state);
        let rebooted = AppState::new_for_test(dir.path().join("settings.json"));
        assert_eq!(
            rebooted
                .get_thread(&thread.id)
                .await
                .expect("restored")
                .skill_ids,
            vec![skill.id]
        );
    }

    #[tokio::test]
    async fn delete_skill_strips_references_everywhere() {
        let (state, dir) = test_state();
        let project = dir.path().to_string_lossy().into_owned();
        let thread = state
            .create_thread(project.clone(), ProviderKind::Go, None)
            .await
            .expect("create");
        let skill = state
            .create_skill(skill_input("Rust"))
            .await
            .expect("skill");
        state
            .set_thread_skills(thread.id.clone(), vec![skill.id.clone()])
            .await
            .expect("attach");
        let mut input = automation_input(&project);
        input.skill_ids = vec![skill.id.clone()];
        let automation = state.create_automation(input).await.expect("automation");
        assert_eq!(automation.skill_ids, vec![skill.id.clone()]);

        state.delete_skill(skill.id.clone()).await.expect("delete");
        assert!(state.list_skills().await.is_empty());
        assert!(state
            .get_thread(&thread.id)
            .await
            .expect("info")
            .skill_ids
            .is_empty());
        assert!(state
            .list_automations()
            .await
            .iter()
            .all(|automation| automation.skill_ids.is_empty()));
        // No dangling refs after a reboot either.
        drop(state);
        let rebooted = AppState::new_for_test(dir.path().join("settings.json"));
        assert!(rebooted
            .get_thread(&thread.id)
            .await
            .expect("restored")
            .skill_ids
            .is_empty());
        assert!(rebooted
            .list_automations()
            .await
            .iter()
            .all(|automation| automation.skill_ids.is_empty()));
    }

    #[tokio::test]
    async fn automation_input_validation_rejects_bad_shapes() {
        let (state, dir) = test_state();
        let project = dir.path().to_string_lossy().into_owned();

        let mut empty_name = automation_input(&project);
        empty_name.name = "  ".to_owned();
        assert!(state
            .create_automation(empty_name)
            .await
            .expect_err("empty name")
            .contains("name must not be empty"));

        let mut empty_task = automation_input(&project);
        empty_task.task = String::new();
        assert!(state
            .create_automation(empty_task)
            .await
            .expect_err("empty task")
            .contains("task must not be empty"));

        for bad_interval in [0, -5] {
            let mut bad = automation_input(&project);
            bad.interval_mins = bad_interval;
            let err = state
                .create_automation(bad)
                .await
                .expect_err("bad interval rejected");
            assert!(err.contains("interval_mins"), "{err}");
        }

        let missing_path = dir
            .path()
            .join("does-not-exist")
            .to_string_lossy()
            .into_owned();
        let mut missing_root = automation_input(&missing_path);
        missing_root.task = "x".to_owned();
        assert!(state.create_automation(missing_root).await.is_err());

        let mut go_no_model = automation_input(&project);
        go_no_model.provider = ProviderKind::Go;
        go_no_model.model = String::new();
        assert!(state.create_automation(go_no_model).await.is_ok());

        let mut bad_skill = automation_input(&project);
        bad_skill.skill_ids = vec!["ghost".to_owned()];
        let err = state
            .create_automation(bad_skill)
            .await
            .expect_err("unknown skill rejected");
        assert!(err.contains("ghost"), "{err}");

        // A valid automation stores canonical roots and a future next run.
        let created = state
            .create_automation(automation_input(&project))
            .await
            .expect("create");
        assert_eq!(created.run_count, 0);
        assert!(created.last_run_at.is_none());
        assert!(created.enabled);
        let next: chrono::DateTime<chrono::Utc> =
            chrono::DateTime::parse_from_rfc3339(&created.next_run_at)
                .expect("rfc3339")
                .into();
        assert!(next > chrono::Utc::now());

        // Updates keep history but reschedule; unknown ids error clearly.
        let mut update = automation_input(&project);
        update.name = "Renamed".to_owned();
        update.interval_mins = 5;
        let updated = state
            .update_automation(created.id.clone(), update)
            .await
            .expect("update");
        assert_eq!(updated.id, created.id);
        assert_eq!(updated.name, "Renamed");
        assert_eq!(updated.interval_mins, 5);
        assert_eq!(updated.run_count, 0);
        assert!(state
            .update_automation("missing".to_owned(), automation_input(&project))
            .await
            .expect_err("unknown automation")
            .contains("unknown automation"));
        assert!(state
            .delete_automation("missing".to_owned())
            .await
            .expect_err("unknown automation")
            .contains("unknown automation"));
        state
            .delete_automation(created.id.clone())
            .await
            .expect("delete");
        assert_eq!(state.list_automations().await.len(), 1);
    }

    #[tokio::test]
    async fn set_automation_enabled_flips_and_reschedules() {
        let (state, _dir) = test_state();
        let dir = tempfile::tempdir().expect("tempdir");
        let project = dir.path().to_string_lossy().into_owned();
        let created = state
            .create_automation(automation_input(&project))
            .await
            .expect("create");

        let disabled = state
            .set_automation_enabled(created.id.clone(), false)
            .await
            .expect("disable");
        assert!(!disabled.enabled);
        // Disabling keeps the existing schedule.
        assert_eq!(disabled.next_run_at, created.next_run_at);

        let before = chrono::Utc::now();
        let enabled = state
            .set_automation_enabled(created.id.clone(), true)
            .await
            .expect("enable");
        assert!(enabled.enabled);
        let next: chrono::DateTime<chrono::Utc> =
            chrono::DateTime::parse_from_rfc3339(&enabled.next_run_at)
                .expect("rfc3339")
                .into();
        let expected = before + chrono::Duration::minutes(60);
        assert!(
            (next - expected).num_seconds().abs() < 60,
            "enabling reschedules to now + interval: {next} vs {expected}"
        );
        assert!(state
            .set_automation_enabled("missing".to_owned(), true)
            .await
            .expect_err("unknown automation")
            .contains("unknown automation"));
    }

    #[tokio::test]
    async fn tick_skips_when_globally_disabled_or_automation_disabled() {
        let (state, _dir) = test_state();
        let dir = tempfile::tempdir().expect("tempdir");
        let project = dir.path().to_string_lossy().into_owned();
        let (sink, _rx) = test_sink();

        // Disabled automation, due: no thread, schedule untouched.
        let mut input = automation_input(&project);
        input.enabled = false;
        let automation = state.create_automation(input).await.expect("create");
        state
            .set_automation_next_run_for_test(&automation.id, "2001-01-01T00:00:00Z".to_owned())
            .await
            .expect("force due");
        state.tick_automations_once(&sink).await;
        assert!(state
            .list_threads(project.clone())
            .await
            .expect("list")
            .is_empty());
        let untouched = state
            .list_automations()
            .await
            .into_iter()
            .find(|item| item.id == automation.id)
            .expect("still there");
        assert_eq!(untouched.next_run_at, "2001-01-01T00:00:00Z");
        assert_eq!(untouched.run_count, 0);

        // Enabled automation but global kill-switch off: same silence.
        let enabled = state
            .create_automation(automation_input(&project))
            .await
            .expect("create");
        state
            .set_automation_next_run_for_test(&enabled.id, "2001-01-01T00:00:00Z".to_owned())
            .await
            .expect("force due");
        state
            .update_settings(SettingsPatch {
                automations_enabled: Some(false),
                ..SettingsPatch::default()
            })
            .await
            .expect("kill switch off");
        state.tick_automations_once(&sink).await;
        assert!(state
            .list_threads(project.clone())
            .await
            .expect("list")
            .is_empty());
        assert!(state.list_review_items(None).await.is_empty());
    }

    #[tokio::test]
    async fn tick_skips_a_vanished_project_root_without_advancing() {
        let (state, _dir) = test_state();
        let dir = tempfile::tempdir().expect("tempdir");
        let project_dir = dir.path().join("repo");
        std::fs::create_dir(&project_dir).expect("mkdir");
        let project = project_dir.to_string_lossy().into_owned();
        let (sink, _rx) = test_sink();

        let automation = state
            .create_automation(automation_input(&project))
            .await
            .expect("create");
        state
            .set_automation_next_run_for_test(&automation.id, "2001-01-01T00:00:00Z".to_owned())
            .await
            .expect("force due");
        std::fs::remove_dir_all(&project_dir).expect("remove root");

        state.tick_automations_once(&sink).await;
        let skipped = state
            .list_automations()
            .await
            .into_iter()
            .find(|item| item.id == automation.id)
            .expect("still there");
        assert_eq!(skipped.next_run_at, "2001-01-01T00:00:00Z");
        assert_eq!(skipped.run_count, 0);
        assert!(skipped.last_run_at.is_none());
        assert!(state.list_review_items(None).await.is_empty());
    }

    #[tokio::test]
    async fn spawn_failure_advances_schedule_but_owes_no_review() {
        // No key and no custom base URL: the send path fails deterministically
        // without touching the network.
        let (state, _dir) = test_state();
        let dir = tempfile::tempdir().expect("tempdir");
        let project = dir.path().to_string_lossy().into_owned();
        let (sink, _rx) = test_sink();

        let automation = state
            .create_automation(automation_input(&project))
            .await
            .expect("create");
        state
            .set_automation_next_run_for_test(&automation.id, "2001-01-01T00:00:00Z".to_owned())
            .await
            .expect("force due");
        state.tick_automations_once(&sink).await;

        // The thread was still created (named for the automation)...
        let threads = state.list_threads(project).await.expect("list");
        assert_eq!(threads.len(), 1);
        assert!(
            threads[0].title.starts_with("Automation Nightly — "),
            "{}",
            threads[0].title
        );
        assert!(!threads[0].running);
        // ... the schedule advanced so the tick doesn't error-loop ...
        let advanced = state
            .list_automations()
            .await
            .into_iter()
            .find(|item| item.id == automation.id)
            .expect("still there");
        assert_eq!(advanced.run_count, 1);
        assert!(advanced.last_run_at.is_some());
        assert_ne!(advanced.next_run_at, "2001-01-01T00:00:00Z");
        let next: chrono::DateTime<chrono::Utc> =
            chrono::DateTime::parse_from_rfc3339(&advanced.next_run_at)
                .expect("rfc3339")
                .into();
        assert!(next > chrono::Utc::now());
        // ... and no review item exists, because no run completed.
        assert!(state.list_review_items(None).await.is_empty());
    }

    #[tokio::test]
    async fn run_automation_now_rejects_unknown_and_vanished_roots() {
        let (state, _dir) = test_state();
        let dir = tempfile::tempdir().expect("tempdir");
        let project_dir = dir.path().join("repo");
        std::fs::create_dir(&project_dir).expect("mkdir");
        let (sink, _rx) = test_sink();

        let err = state
            .run_automation_now(sink.clone(), "missing".to_owned())
            .await
            .expect_err("unknown automation");
        assert!(err.contains("unknown automation"), "{err}");

        let project = project_dir.to_string_lossy().into_owned();
        let automation = state
            .create_automation(automation_input(&project))
            .await
            .expect("create");
        std::fs::remove_dir_all(&project_dir).expect("remove root");
        let err = state
            .run_automation_now(sink, automation.id.clone())
            .await
            .expect_err("vanished root");
        assert!(err.contains("no longer exists"), "{err}");
    }

    #[tokio::test]
    async fn review_dismiss_and_continue_flow() {
        let (state, dir) = test_state();
        let project = dir.path().to_string_lossy().into_owned();
        let thread = state
            .create_thread(project, ProviderKind::Go, Some("m".to_owned()))
            .await
            .expect("create");
        // Seed one pending review through the store file, then reboot.
        let item = ReviewItem {
            id: "review-1".to_owned(),
            automation_id: "auto-1".to_owned(),
            thread_id: thread.id.clone(),
            created_at: "2026-01-01T00:00:00Z".to_owned(),
            title: "Automation Nightly".to_owned(),
            summary: "done".to_owned(),
            status: ReviewStatus::Pending,
        };
        std::fs::write(
            dir.path().join("review_items.json"),
            serde_json::to_string_pretty(&vec![item]).expect("json"),
        )
        .expect("write");
        drop(state);
        let state = AppState::new_for_test(dir.path().join("settings.json"));

        assert_eq!(state.list_review_items(None).await.len(), 1);
        assert_eq!(
            state
                .list_review_items(Some(ReviewStatus::Pending))
                .await
                .len(),
            1
        );
        assert!(state
            .list_review_items(Some(ReviewStatus::Dismissed))
            .await
            .is_empty());

        let dismissed = state
            .dismiss_review_item("review-1".to_owned())
            .await
            .expect("dismiss");
        assert_eq!(dismissed.status, ReviewStatus::Dismissed);
        assert!(state
            .list_review_items(Some(ReviewStatus::Pending))
            .await
            .is_empty());
        assert!(state
            .dismiss_review_item("missing".to_owned())
            .await
            .expect_err("unknown review")
            .contains("unknown review"));

        // A second review exercises continue: it returns the thread.
        let item = ReviewItem {
            id: "review-2".to_owned(),
            automation_id: "auto-1".to_owned(),
            thread_id: thread.id.clone(),
            created_at: "2026-01-02T00:00:00Z".to_owned(),
            title: "Automation Nightly".to_owned(),
            summary: "done again".to_owned(),
            status: ReviewStatus::Pending,
        };
        std::fs::write(
            dir.path().join("review_items.json"),
            serde_json::to_string_pretty(&vec![dismissed, item]).expect("json"),
        )
        .expect("write");
        drop(state);
        let state = AppState::new_for_test(dir.path().join("settings.json"));
        let info = state
            .continue_review_item("review-2".to_owned())
            .await
            .expect("continue");
        assert_eq!(info.id, thread.id);
        assert_eq!(
            state
                .list_review_items(Some(ReviewStatus::Continued))
                .await
                .len(),
            1
        );
        assert!(state
            .continue_review_item("missing".to_owned())
            .await
            .expect_err("unknown review")
            .contains("unknown review"));
    }

    #[tokio::test]
    async fn continue_review_item_keeps_status_when_thread_is_gone() {
        let (state, dir) = test_state();
        let project = dir.path().to_string_lossy().into_owned();
        let thread = state
            .create_thread(project, ProviderKind::Go, None)
            .await
            .expect("create");
        let item = ReviewItem {
            id: "review-1".to_owned(),
            automation_id: "auto-1".to_owned(),
            thread_id: thread.id.clone(),
            created_at: "2026-01-01T00:00:00Z".to_owned(),
            title: "t".to_owned(),
            summary: "s".to_owned(),
            status: ReviewStatus::Pending,
        };
        std::fs::write(
            dir.path().join("review_items.json"),
            serde_json::to_string_pretty(&vec![item]).expect("json"),
        )
        .expect("write");
        drop(state);
        let state = AppState::new_for_test(dir.path().join("settings.json"));
        state
            .discard_thread(thread.id.clone())
            .await
            .expect("discard");
        let err = state
            .continue_review_item("review-1".to_owned())
            .await
            .expect_err("thread gone");
        assert!(err.contains("unknown thread"), "{err}");
        // The status was NOT flipped.
        assert_eq!(
            state.list_review_items(None).await[0].status,
            ReviewStatus::Pending
        );
    }

    #[tokio::test]
    async fn stores_survive_reboot() {
        let (state, dir) = test_state();
        let project = dir.path().to_string_lossy().into_owned();
        let skill = state
            .create_skill(skill_input("Rust"))
            .await
            .expect("skill");
        let automation = state
            .create_automation(automation_input(&project))
            .await
            .expect("automation");
        let thread = state
            .create_thread(project, ProviderKind::Go, Some("m".to_owned()))
            .await
            .expect("thread");
        let item = ReviewItem {
            id: "review-1".to_owned(),
            automation_id: automation.id.clone(),
            thread_id: thread.id.clone(),
            created_at: "2026-01-01T00:00:00Z".to_owned(),
            title: "t".to_owned(),
            summary: "s".to_owned(),
            status: ReviewStatus::Pending,
        };
        std::fs::write(
            dir.path().join("review_items.json"),
            serde_json::to_string_pretty(&vec![item]).expect("json"),
        )
        .expect("write");
        drop(state);

        let rebooted = AppState::new_for_test(dir.path().join("settings.json"));
        assert_eq!(rebooted.list_skills().await.len(), 1);
        assert_eq!(rebooted.list_skills().await[0].id, skill.id);
        assert_eq!(rebooted.list_automations().await.len(), 1);
        assert_eq!(rebooted.list_automations().await[0].id, automation.id);
        assert_eq!(rebooted.list_review_items(None).await.len(), 1);
        assert!(rebooted.get_thread(&thread.id).await.is_ok());
    }

    #[tokio::test]
    async fn boot_defers_missed_automations_without_herding() {
        let dir = tempfile::tempdir().expect("tempdir");
        let project = dir.path().to_string_lossy().into_owned();
        let missed = Automation {
            id: "missed".to_owned(),
            name: "Missed".to_owned(),
            project_root: project.clone(),
            provider: ProviderKind::Go,
            model: "m".to_owned(),
            reasoning_effort: None,
            target_thread_id: None,
            skill_ids: Vec::new(),
            interval_mins: 30,
            schedule: None,
            task: "t".to_owned(),
            enabled: true,
            last_run_at: None,
            next_run_at: "2001-01-01T00:00:00Z".to_owned(),
            run_count: 4,
        };
        let future = Automation {
            id: "future".to_owned(),
            next_run_at: "2999-01-01T00:00:00Z".to_owned(),
            ..missed.clone()
        };
        std::fs::write(
            dir.path().join("automations.json"),
            serde_json::to_string_pretty(&vec![missed, future]).expect("json"),
        )
        .expect("write");

        let state = AppState::new_for_test(dir.path().join("settings.json"));
        let automations = state.list_automations().await;
        let missed = automations
            .iter()
            .find(|item| item.id == "missed")
            .expect("missed");
        let next: chrono::DateTime<chrono::Utc> =
            chrono::DateTime::parse_from_rfc3339(&missed.next_run_at)
                .expect("rfc3339")
                .into();
        let delta = (next - chrono::Utc::now()).num_seconds();
        assert!(
            (30..=90).contains(&delta),
            "missed run deferred ~60s, got {delta}s"
        );
        assert_eq!(missed.run_count, 4);
        let future = automations
            .iter()
            .find(|item| item.id == "future")
            .expect("future");
        assert_eq!(future.next_run_at, "2999-01-01T00:00:00Z");
        assert!(state
            .reconcile_report()
            .iter()
            .any(|note| note.contains("deferred 1 missed automation")));
        // The deferral was persisted back, so a second boot defers from the
        // new stamp rather than stacking (still ~60s out, not firing).
        drop(state);
        let rebooted = AppState::new_for_test(dir.path().join("settings.json"));
        let missed = rebooted
            .list_automations()
            .await
            .into_iter()
            .find(|item| item.id == "missed")
            .expect("missed");
        let next: chrono::DateTime<chrono::Utc> =
            chrono::DateTime::parse_from_rfc3339(&missed.next_run_at)
                .expect("rfc3339")
                .into();
        assert!(next > chrono::Utc::now());
    }

    #[tokio::test]
    async fn old_settings_file_without_kill_switch_loads_enabled() {
        let dir = tempfile::tempdir().expect("tempdir");
        std::fs::write(
            dir.path().join("settings.json"),
            r#"{"theme":"dark","default_provider":"openai","default_model":"gpt-x","max_turns":7}"#,
        )
        .expect("write");
        let state = AppState::new_for_test(dir.path().join("settings.json"));
        let settings = state.get_settings().await;
        assert!(settings.automations_enabled);
        assert!(!settings.onboarded);
    }

    #[tokio::test]
    async fn corrupt_stores_boot_empty_and_report() {
        let dir = tempfile::tempdir().expect("tempdir");
        std::fs::write(dir.path().join("skills.json"), "{ nope").expect("write");
        std::fs::write(dir.path().join("automations.json"), "[ broken").expect("write");
        std::fs::write(dir.path().join("review_items.json"), "zzz").expect("write");
        let state = AppState::new_for_test(dir.path().join("settings.json"));
        assert!(state.list_skills().await.is_empty());
        assert!(state.list_automations().await.is_empty());
        assert!(state.list_review_items(None).await.is_empty());
        let report = state.reconcile_report();
        assert_eq!(report.len(), 3, "{report:?}");
    }

    #[test]
    fn diagnostics_ring_evicts_oldest_at_cap() {
        let (state, _dir) = test_state();
        assert!(state.recent_errors().is_empty());
        for index in 0..(MAX_RECENT_ERRORS + 5) {
            state.record_error("cmd", format!("error-{index}"));
        }
        let errors = state.recent_errors();
        assert_eq!(errors.len(), MAX_RECENT_ERRORS);
        // Oldest-first: the first five entries were evicted.
        assert_eq!(errors.first().expect("first").message, "error-5");
        assert_eq!(
            errors.last().expect("last").message,
            format!("error-{}", MAX_RECENT_ERRORS + 4)
        );
        for (position, entry) in errors.iter().enumerate() {
            assert_eq!(entry.message, format!("error-{}", position + 5));
        }
    }

    #[tokio::test]
    async fn diagnostics_snapshot_reports_version_os_settings_and_errors() {
        let (state, _dir) = test_state();
        // A genuine handler failure, recorded exactly as the `open_project`
        // command wrapper records it (see `record_cmd!` in commands.rs).
        let err = state
            .open_project("/definitely/not/a/themis/dir".to_owned())
            .await
            .expect_err("missing dir fails");
        state.record_error("open_project", err);
        let diagnostics = state.get_diagnostics().await;
        assert_eq!(diagnostics.app_version, env!("CARGO_PKG_VERSION"));
        assert_eq!(diagnostics.os, std::env::consts::OS);
        assert_eq!(diagnostics.settings, state.get_settings().await);
        assert_eq!(diagnostics.recent_errors.len(), 1);
        let entry = &diagnostics.recent_errors[0];
        assert_eq!(entry.command, "open_project");
        assert!(!entry.message.is_empty());
        // The timestamp parses as RFC3339.
        let parsed: DateTime<Utc> = entry.at.parse().expect("RFC3339 timestamp");
        assert!(parsed <= Utc::now());
    }
}
