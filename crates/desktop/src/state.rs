//! Application state and handler logic.
//!
//! [`AppState`] owns projects, threads, pending approvals, settings, and
//! secrets. Every Tauri command in [`crate::commands`] is a thin wrapper over
//! one of these plain async methods, which take an [`EventSink`] instead of
//! touching Tauri — that keeps them directly drivable from headless tests.

use std::collections::{HashMap, HashSet, VecDeque};
use std::path::{Component, Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::Arc;

use chrono::{DateTime, SecondsFormat, Utc};
use serde::{Deserialize, Serialize};
use themis_core::providers::{ProviderConfig, GO_DEFAULT_MODEL, GO_MODEL_ENV_VAR};
use themis_core::runtime::{run_task_with_stop, CachingApprovals, ConversationTurn, RunEvent};
use themis_core::skills::{materialize_scripts, validate_skill_input};
use themis_core::tools::{boxed_tools, ApprovalHook};

use crate::approvals::{DesktopApprovalHook, PendingMap};
use crate::diff::{
    git_diff_cached, git_diff_head, git_diff_worktree, git_head, git_status_names, git_untracked,
    is_git_repo, untracked_diff_file, MAX_DIFF_FILES,
};
use crate::secrets::{MemoryStore, SecretStore};
use crate::settings::SettingsStore;
use crate::sink::EventSink;
use crate::transcript::{HistoryItem, TranscriptStore};
use crate::types::{
    ApprovalDecision, Automation, AutomationInput, Diagnostics, DiagnosticsError, DiffState,
    MergeResult, ProjectInfo, ProviderKind, ReviewItem, ReviewStatus, RunAutomationNow, RunHandle,
    SecretStatus, Settings, SettingsPatch, Skill, SkillInput, ThreadComment, ThreadEvent,
    ThreadEventEnvelope, ThreadInfo,
};
use crate::worktree;

/// Environment fallback for the Custom provider's base URL (the desktop UI
/// exposes no base-URL field yet).
pub const CUSTOM_BASE_URL_ENV_VAR: &str = "THEMIS_CUSTOM_BASE_URL";

/// Title assigned to threads before their first message.
pub const UNTITLED_THREAD: &str = "New thread";

/// Maximum title length taken from the first message, in chars.
pub const MAX_TITLE_CHARS: usize = 60;

/// Channel capacity for the run-event pump (runs emit a handful of events).
const EVENT_CHANNEL_CAPACITY: usize = 1024;

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
    confirm_reads: bool,
    /// Tool sandbox root: the shared project checkout.
    work_root: PathBuf,
    is_git: bool,
    provider: ProviderKind,
    model: String,
    /// Skills resolved from the thread's `skill_ids` (missing ids are
    /// skipped: every mutation keeps references consistent, so a miss only
    /// follows a hand-edited store file).
    skills: Vec<themis_core::skills::Skill>,
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
struct RegistryEntry {
    id: String,
    title: String,
    project_root: String,
    worktree_path: Option<String>,
    branch: Option<String>,
    base_branch: Option<String>,
    provider: ProviderKind,
    #[serde(default)]
    model: String,
    #[serde(default)]
    was_running: bool,
    #[serde(default)]
    skill_ids: Vec<String>,
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
            was_running: record.running,
            skill_ids: record.skill_ids.clone(),
        }
    }
}

struct AppStateInner {
    stop_flags: std::sync::Mutex<HashMap<String, Arc<AtomicBool>>>,
    projects: tokio::sync::RwLock<HashMap<String, ProjectInfo>>,
    threads: tokio::sync::RwLock<HashMap<String, ThreadRecord>>,
    pending: PendingMap,
    settings: SettingsStore,
    transcript: TranscriptStore,
    secrets: Arc<dyn SecretStore>,
    custom_base_url_override: std::sync::Mutex<Option<String>>,
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
    /// app data dir plus session credentials). The thread registry, the skill /
    /// automation / review stores, and the worktrees root live beside the
    /// settings file; the registry is reconciled synchronously during
    /// construction, and the automation scheduler is spawned on the Tauri
    /// async runtime (ticking every [`SCHEDULER_TICK_SECS`]; it stays quiet
    /// until [`AppState::set_scheduler_sink`] registers a sink).
    #[must_use]
    pub fn new(settings_path: PathBuf, secrets: Arc<dyn SecretStore>) -> Self {
        let app_dir = settings_app_dir(&settings_path);
        let worktrees_root = app_dir.join("worktrees");
        let registry_path = app_dir.join("threads.json");
        let state = Self::new_with_dirs(settings_path, worktrees_root, registry_path, secrets);
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
        )
    }

    /// Creates state for headless tests with an explicit worktrees root (the
    /// registry stays beside the settings file). Never spawns the scheduler.
    #[must_use]
    pub fn new_for_test_with_worktrees_root(
        settings_path: PathBuf,
        worktrees_root: PathBuf,
    ) -> Self {
        let registry_path = settings_app_dir(&settings_path).join("threads.json");
        Self::new_with_dirs(
            settings_path,
            worktrees_root,
            registry_path,
            Arc::new(MemoryStore::new()),
        )
    }

    fn new_with_dirs(
        settings_path: PathBuf,
        worktrees_root: PathBuf,
        registry_path: PathBuf,
        secrets: Arc<dyn SecretStore>,
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
        Self {
            inner: Arc::new(AppStateInner {
                stop_flags: std::sync::Mutex::new(HashMap::new()),
                projects: tokio::sync::RwLock::new(HashMap::new()),
                threads: tokio::sync::RwLock::new(threads),
                pending: PendingMap::default(),
                settings: SettingsStore::load(settings_path),
                transcript,
                secrets,
                custom_base_url_override: std::sync::Mutex::new(None),
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

    /// Overrides the Custom provider base URL (test hook so headless runs can
    /// point at a mock server; production falls back to
    /// [`CUSTOM_BASE_URL_ENV_VAR`]).
    pub fn set_custom_base_url_override(&self, url: Option<String>) {
        *self
            .inner
            .custom_base_url_override
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner()) = url;
    }

    /// Health check: returns the `themis-core` version.
    pub async fn ping(&self) -> String {
        themis_core::version().to_owned()
    }

    /// Creates a new project without touching any existing directory.
    pub async fn create_project(
        &self,
        name: String,
        directory: Option<String>,
    ) -> Result<ProjectInfo, String> {
        let name = name.trim();
        if name.is_empty()
            || name.len() > 80
            || name.starts_with('.')
            || name.ends_with('.')
            || !name
                .chars()
                .all(|c| c.is_alphanumeric() || matches!(c, ' ' | '-' | '_'))
        {
            return Err(
                "Use 1–80 letters, numbers, spaces, hyphens or underscores for the project name"
                    .to_owned(),
            );
        }
        let base = directory.unwrap_or(self.inner.settings.get().await.projects_directory);
        let base = PathBuf::from(base.trim());
        if !base.is_absolute()
            || base
                .components()
                .any(|c| matches!(c, std::path::Component::ParentDir))
        {
            return Err("Choose an absolute projects folder in Settings".to_owned());
        }
        std::fs::create_dir_all(&base)
            .map_err(|err| format!("Could not create projects folder: {err}"))?;
        let root = base
            .canonicalize()
            .map_err(|err| err.to_string())?
            .join(name);
        std::fs::create_dir(&root).map_err(|err| {
            if err.kind() == std::io::ErrorKind::AlreadyExists { "A project with this name already exists. Choose another name or open the existing folder.".to_owned() }
            else { format!("Could not create project: {err}") }
        })?;
        // A first commit establishes the project diff baseline.
        let initialized = (|| {
            for args in [
                vec!["init", "-q", "-b", "main"],
                vec![
                    "-c",
                    "user.name=Themis",
                    "-c",
                    "user.email=themis@localhost",
                    "-c",
                    "commit.gpgsign=false",
                    "-c",
                    "core.hooksPath=/dev/null",
                    "commit",
                    "--allow-empty",
                    "-qm",
                    "Initialize project",
                ],
            ] {
                let output = std::process::Command::new("git")
                    .env_remove("OPENCODE_KEY")
                    .current_dir(&root)
                    .args(args)
                    .output()
                    .map_err(|err| err.to_string())?;
                if !output.status.success() {
                    return Err(String::from_utf8_lossy(&output.stderr).into_owned());
                }
            }
            Ok::<(), String>(())
        })();
        if let Err(error) = initialized {
            return Err(format!(
                "Project folder created at {}, but Git setup failed: {error}",
                root.display()
            ));
        }
        self.open_project(root.to_string_lossy().into_owned()).await
    }

    /// Opens a project directory by typed path and reports its git status.
    pub async fn open_project(&self, path: String) -> Result<ProjectInfo, String> {
        let root = canonical_project_dir(&path)?;
        let name = root
            .file_name()
            .map(|name| name.to_string_lossy().into_owned())
            .unwrap_or_default();
        let info = ProjectInfo {
            root: root.to_string_lossy().into_owned(),
            name,
            is_git: is_git_repo(&root),
        };
        self.inner
            .projects
            .write()
            .await
            .insert(info.root.clone(), info.clone());
        // Track recency for the project picker (best-effort).
        let mut recent = self.inner.settings.get().await.recent_roots;
        recent.retain(|root| root != &info.root);
        recent.insert(0, info.root.clone());
        recent.truncate(crate::settings::MAX_RECENT_ROOTS);
        let _ = self
            .inner
            .settings
            .update(SettingsPatch {
                recent_roots: Some(recent),
                ..SettingsPatch::default()
            })
            .await;
        Ok(info)
    }

    /// Creates a thread on `project_root`.
    ///
    /// Threads share the project checkout, including existing uncommitted files.
    pub async fn create_thread(
        &self,
        project_root: String,
        provider: ProviderKind,
        model: Option<String>,
    ) -> Result<ThreadInfo, String> {
        let root = canonical_project_dir(&project_root)?;
        let is_git = is_git_repo(&root);
        let id = uuid::Uuid::new_v4().to_string();
        let preexisting = git_status_names(&root).unwrap_or_default();
        let head = git_head(&root);
        let record = ThreadRecord {
            id,
            title: UNTITLED_THREAD.to_owned(),
            project_root: root.clone(),
            is_git,
            provider,
            model: model.unwrap_or_default(),
            running: false,
            titled: false,
            preexisting,
            head,
            accepted: HashSet::new(),
            comments: Vec::new(),
            worktree_path: None,
            branch: None,
            base_branch: None,
            recovered: false,
            merged: false,
            broken: false,
            skill_ids: Vec::new(),
        };
        let info = thread_info(&record);
        self.inner
            .threads
            .write()
            .await
            .insert(record.id.clone(), record);
        // Auto-register so a thread can be created on any valid directory,
        // even if `open_project` was skipped.
        let key = root.to_string_lossy().into_owned();
        self.inner
            .projects
            .write()
            .await
            .entry(key.clone())
            .or_insert_with(|| ProjectInfo {
                root: key,
                name: project_name(&root),
                is_git,
            });
        self.persist_registry().await;
        Ok(info)
    }

    /// Returns the current info for `thread_id`.
    pub async fn get_thread(&self, thread_id: &str) -> Result<ThreadInfo, String> {
        let threads = self.inner.threads.read().await;
        threads
            .get(thread_id)
            .map(thread_info)
            .ok_or_else(|| format!("unknown thread '{thread_id}'"))
    }

    pub async fn get_thread_history(&self, thread_id: &str) -> Result<Vec<HistoryItem>, String> {
        self.get_thread(thread_id).await?;
        self.inner.transcript.history(thread_id)
    }

    pub async fn import_legacy_history(
        &self,
        thread_id: &str,
        messages: &[serde_json::Value],
    ) -> Result<(), String> {
        self.get_thread(thread_id).await?;
        self.inner.transcript.import_legacy(thread_id, messages)
    }

    /// Lists the threads on `project_root` (empty when the project has none).
    pub async fn list_threads(&self, project_root: String) -> Result<Vec<ThreadInfo>, String> {
        let root = canonical_project_dir(&project_root)?;
        let threads = self.inner.threads.read().await;
        let mut infos: Vec<ThreadInfo> = threads
            .values()
            .filter(|record| record.project_root == root)
            .map(thread_info)
            .collect();
        infos.sort_by(|left, right| left.id.cmp(&right.id));
        Ok(infos)
    }

    /// Sends a message: rejects busy threads and a saturated global run gate,
    /// resolves the provider, and spawns the run on the async runtime,
    /// returning its handle immediately.
    pub async fn send_message(
        &self,
        sink: Arc<dyn EventSink>,
        thread_id: String,
        text: String,
    ) -> Result<RunHandle, String> {
        let run_id = uuid::Uuid::new_v4().to_string();
        self.send_message_with_id(sink, thread_id, text, run_id)
            .await
    }

    /// Sends a message with a caller-chosen `run_id` (the automation
    /// scheduler pre-registers its completion mapping under this id before
    /// spawning, so even an instantly-failing run is attributed).
    async fn send_message_with_id(
        &self,
        sink: Arc<dyn EventSink>,
        thread_id: String,
        text: String,
        run_id: String,
    ) -> Result<RunHandle, String> {
        self.send_message_with_options(sink, thread_id, text, run_id, None)
            .await
    }

    /// Sends a user message with an optional provider reasoning effort.
    pub async fn send_message_with_effort(
        &self,
        sink: Arc<dyn EventSink>,
        thread_id: String,
        text: String,
        effort: Option<String>,
    ) -> Result<RunHandle, String> {
        self.send_message_with_options(
            sink,
            thread_id,
            text,
            uuid::Uuid::new_v4().to_string(),
            effort,
        )
        .await
    }

    async fn send_message_with_options(
        &self,
        sink: Arc<dyn EventSink>,
        thread_id: String,
        text: String,
        run_id: String,
        reasoning_effort: Option<String>,
    ) -> Result<RunHandle, String> {
        let settings = self.inner.settings.get().await;
        let history = self
            .inner
            .transcript
            .context(&thread_id, settings.context_messages.clamp(1, 100) as usize)?;
        let snapshot = {
            let mut threads = self.inner.threads.write().await;
            let record = threads
                .get_mut(&thread_id)
                .ok_or_else(|| format!("unknown thread '{thread_id}'"))?;
            if record.running {
                return Err(format!(
                    "thread '{thread_id}' is busy: a run is already in progress"
                ));
            }
            let limit = usize::try_from(settings.concurrency_limit).unwrap_or(usize::MAX);
            // The counter check runs under the state lock, so concurrent sends
            // serialize here and the gate cannot be overrun.
            if self.inner.running_count.load(Ordering::SeqCst) >= limit {
                return Err(format!(
                    "concurrency limit reached ({}): wait for a run to finish and retry",
                    settings.concurrency_limit
                ));
            }
            self.inner.running_count.fetch_add(1, Ordering::SeqCst);
            record.running = true;
            self.inner
                .stop_flags
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .insert(thread_id.clone(), Arc::new(AtomicBool::new(false)));
            if !record.titled {
                record.title = title_from_message(&text);
                record.titled = true;
            }
            // Resolve the thread's skills to core skills for the run. This
            // nests the skills read lock under the threads write lock — the
            // only such nesting, and no path nests the reverse, so lock order
            // stays acyclic.
            let skills = self.inner.skills.read().await;
            let resolved: Vec<themis_core::skills::Skill> = record
                .skill_ids
                .iter()
                .filter_map(|id| skills.get(id))
                .map(Skill::core_skill)
                .collect();
            RunSnapshot {
                reasoning_effort,
                history,
                approval_timeout_seconds: settings.approval_timeout_seconds.clamp(30, 600),
                confirm_reads: settings.confirm_reads,
                work_root: record.project_root.clone(),
                is_git: record.is_git,
                provider: record.provider,
                model: record.model.clone(),
                skills: resolved,
            }
        };
        if let Err(error) = self
            .inner
            .transcript
            .append_user(&thread_id, &run_id, &text)
        {
            self.finish_run(&thread_id).await;
            return Err(error);
        }
        self.persist_registry().await;
        // Clamp: updates are validated, but the file may predate validation.
        let max_turns = usize::try_from(settings.max_turns)
            .unwrap_or(crate::settings::MAX_TURNS_MAX as usize)
            .clamp(1, crate::settings::MAX_TURNS_MAX as usize);
        if let Err(error) = self
            .spawn_run(&sink, &thread_id, &run_id, snapshot, text, max_turns)
            .await
        {
            if let Err(save_error) = self.inner.transcript.append_event(&ThreadEventEnvelope {
                thread_id: thread_id.clone(),
                run_id: run_id.clone(),
                event: ThreadEvent::Failed {
                    error: error.clone(),
                },
            }) {
                self.record_error("save_thread_event", save_error);
            }
            self.finish_run(&thread_id).await;
            return Err(error);
        }
        Ok(RunHandle { run_id })
    }

    /// Renames a thread without changing its files or worktree.
    pub async fn rename_thread(
        &self,
        thread_id: String,
        title: String,
    ) -> Result<ThreadInfo, String> {
        let title = title.trim();
        if title.is_empty() || title.chars().count() > 120 || title.chars().any(char::is_control) {
            return Err("Use a thread title of 1–120 characters on one line".to_owned());
        }
        let info = {
            let mut threads = self.inner.threads.write().await;
            let record = threads
                .get_mut(&thread_id)
                .ok_or_else(|| "Thread not found".to_owned())?;
            record.title = title.to_owned();
            record.titled = true;
            thread_info(record)
        };
        self.persist_registry().await;
        Ok(info)
    }

    /// Requests a stop at the next safe boundary; existing actions finish first.
    pub async fn stop_thread(&self, thread_id: String) -> Result<(), String> {
        let flags = self
            .inner
            .stop_flags
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        let flag = flags
            .get(&thread_id)
            .ok_or_else(|| "This thread has no active run".to_owned())?;
        flag.store(true, Ordering::SeqCst);
        let mut pending = self.inner.pending.lock().unwrap_or_else(|e| e.into_inner());
        pending.retain(|_, approval| {
            if approval.thread_id == thread_id {
                let _ = approval.sender.send(themis_core::tools::Approval::Deny);
                false
            } else {
                true
            }
        });
        Ok(())
    }

    /// Checks the Go connection and returns the current provider model catalog.
    pub async fn list_go_models(&self) -> Result<Vec<String>, String> {
        let key = self.api_key_for(ProviderKind::Go)?;
        themis_core::providers::refresh_go_models(themis_core::providers::GO_BASE_URL, &key)
            .await
            .map_err(|error| format!("Could not load OpenCode Go models: {error}"))
    }

    /// Lists the shared project checkout diff (git repos only).
    pub async fn list_diff(&self, thread_id: String) -> Result<DiffState, String> {
        let (root, preexisting) = {
            let threads = self.inner.threads.read().await;
            let record = threads
                .get(&thread_id)
                .ok_or_else(|| format!("unknown thread '{thread_id}'"))?;
            match diff_root_for(record) {
                Ok(root) => (root, record.preexisting.clone()),
                Err(reason) => {
                    return Ok(DiffState {
                        available: false,
                        reason: Some(reason),
                        files: Vec::new(),
                    });
                }
            }
        };
        if !is_git_repo(&root) {
            return Ok(DiffState {
                available: false,
                reason: Some("project is not a git repository".to_owned()),
                files: Vec::new(),
            });
        }
        let mut files = tracked_diff_files(&root)?;
        let mut seen: HashSet<String> = files.iter().map(|file| file.path.clone()).collect();
        for relative in git_untracked(&root)? {
            if seen.insert(relative.clone()) {
                files.push(untracked_diff_file(&root, &relative));
            }
        }
        for file in &mut files {
            file.preexisting = preexisting.contains(&file.path)
                || file
                    .old_path
                    .as_deref()
                    .is_some_and(|old| preexisting.contains(old));
        }
        let total = files.len();
        let reason = if total > MAX_DIFF_FILES {
            files.truncate(MAX_DIFF_FILES);
            Some(format!(
                "truncated: showing {MAX_DIFF_FILES} of {total} changed files"
            ))
        } else {
            None
        };
        Ok(DiffState {
            available: true,
            reason,
            files,
        })
    }

    /// Records acceptance of a changed file (in-memory; the disk already
    /// carries the change).
    pub async fn accept_file(&self, thread_id: String, path: String) -> Result<(), String> {
        let mut threads = self.inner.threads.write().await;
        let record = threads
            .get_mut(&thread_id)
            .ok_or_else(|| format!("unknown thread '{thread_id}'"))?;
        record.accepted.insert(path);
        Ok(())
    }

    /// Discards a thread-made change: tracked files are restored from `HEAD`,
    /// added files are deleted. Preexisting files are refused.
    pub async fn discard_file(&self, thread_id: String, path: String) -> Result<(), String> {
        ensure_root_relative(&path)?;
        let root = {
            let threads = self.inner.threads.read().await;
            let record = threads
                .get(&thread_id)
                .ok_or_else(|| format!("unknown thread '{thread_id}'"))?;
            if record.preexisting.contains(&path) {
                return Err(format!(
                    "refusing to discard '{path}': it had preexisting changes \
                     before the thread started"
                ));
            }
            diff_root_for(record).map_err(|reason| format!("cannot discard: {reason}"))?
        };
        if !is_git_repo(&root) {
            return Err("cannot discard: project is not a git repository".to_owned());
        }
        if git_untracked(&root)?.iter().any(|name| name == &path) {
            std::fs::remove_file(root.join(&path))
                .map_err(|err| format!("cannot delete '{path}': {err}"))?;
        } else if git_head(&root).is_none() {
            // No commits yet: "tracked" means staged-new, so unstage and delete.
            let _ = std::process::Command::new("git")
                .arg("-C")
                .arg(&root)
                .args(["rm", "--cached", "-q", "--", &path])
                .output();
            std::fs::remove_file(root.join(&path))
                .map_err(|err| format!("cannot delete '{path}': {err}"))?;
        } else {
            let out = std::process::Command::new("git")
                .arg("-C")
                .arg(&root)
                .args(["checkout", "HEAD", "--", &path])
                .output()
                .map_err(|err| format!("failed to run git checkout: {err}"))?;
            if !out.status.success() {
                let stderr = String::from_utf8_lossy(&out.stderr);
                return Err(format!(
                    "git checkout failed for '{path}': {}",
                    stderr.trim()
                ));
            }
        }
        if let Some(record) = self.inner.threads.write().await.get_mut(&thread_id) {
            record.accepted.remove(&path);
        }
        Ok(())
    }

    /// Merges a thread's worktree changes into the user's checkout, apply-based.
    ///
    /// Computes the full working-tree-vs-base patch (tracked changes plus
    /// untracked files), dry-runs it with `git apply --check`, and only then
    /// applies it to the checkout's working tree — uncommitted, for the user
    /// to review. History is never mutated. On conflicts nothing is touched
    /// and the conflicting paths are returned. On success the worktree and
    /// branch are retired; the thread itself stays (marked merged internally).
    pub async fn merge_thread(&self, thread_id: String) -> Result<MergeResult, String> {
        let (checkout, worktree, branch, base) = {
            let threads = self.inner.threads.read().await;
            let record = threads
                .get(&thread_id)
                .ok_or_else(|| format!("unknown thread '{thread_id}'"))?;
            if record.running {
                return Err(format!(
                    "thread '{thread_id}' is busy: wait for the run to finish before merging"
                ));
            }
            let Some(worktree) = record.worktree_path.clone() else {
                if record.merged {
                    return Err(format!("thread '{thread_id}' is already merged"));
                }
                return Err(format!(
                    "cannot merge thread '{thread_id}': it has no git worktree (non-git project)"
                ));
            };
            let branch = record.branch.clone().ok_or_else(|| {
                format!("cannot merge thread '{thread_id}': no thread branch recorded")
            })?;
            let base = record.base_branch.clone().ok_or_else(|| {
                format!("cannot merge thread '{thread_id}': no base revision recorded")
            })?;
            (record.project_root.clone(), worktree, branch, base)
        };
        if !worktree.is_dir() {
            return Err(format!(
                "cannot merge thread '{thread_id}': worktree missing \
                 (expected at '{}')",
                worktree.display()
            ));
        }
        let (patch, files) = worktree::merge_patch(&worktree, &base, &branch)?;
        if files.is_empty() {
            // Nothing to apply; still retire the worktree and branch.
            self.retire_worktree(&thread_id, &checkout, &worktree, &branch)
                .await;
            return Ok(MergeResult {
                applied: true,
                applied_files: Vec::new(),
                conflicts: Vec::new(),
            });
        }
        let conflicts = worktree::apply_check(&checkout, &patch, &files)?;
        if !conflicts.is_empty() {
            return Ok(MergeResult {
                applied: false,
                applied_files: Vec::new(),
                conflicts,
            });
        }
        if let Err(err) = worktree::apply_patch(&checkout, &patch) {
            // The checkout moved between check and apply: re-check to report
            // conflicts precisely (`git apply` is atomic, so nothing landed).
            let recheck = worktree::apply_check(&checkout, &patch, &files).unwrap_or_default();
            if !recheck.is_empty() {
                return Ok(MergeResult {
                    applied: false,
                    applied_files: Vec::new(),
                    conflicts: recheck,
                });
            }
            return Err(err);
        }
        self.retire_worktree(&thread_id, &checkout, &worktree, &branch)
            .await;
        Ok(MergeResult {
            applied: true,
            applied_files: files,
            conflicts: Vec::new(),
        })
    }

    /// Removes an idle conversation, preserving shared and legacy workspace files.
    pub async fn discard_thread(&self, thread_id: String) -> Result<(), String> {
        let record = {
            let mut threads = self.inner.threads.write().await;
            match threads.get(&thread_id) {
                Some(record) if record.running => {
                    return Err(format!(
                        "thread '{thread_id}' is busy: wait for the run to finish \
                         before discarding"
                    ));
                }
                Some(_) => {
                    self.inner.transcript.delete_thread(&thread_id)?;
                    threads.remove(&thread_id).expect("checked above")
                }
                None => return Err(format!("unknown thread '{thread_id}'")),
            }
        };
        drop(record);
        self.persist_registry().await;
        Ok(())
    }

    /// Retires a merged thread's worktree and branch (best-effort git cleanup;
    /// the thread record itself is always updated).
    async fn retire_worktree(
        &self,
        thread_id: &str,
        checkout: &Path,
        worktree_path: &Path,
        branch: &str,
    ) {
        let _ = worktree::remove_worktree(checkout, worktree_path);
        if worktree_path.exists() && worktree_path.starts_with(&self.inner.worktrees_root) {
            let _ = std::fs::remove_dir_all(worktree_path);
        }
        worktree::prune_worktrees(checkout);
        let _ = worktree::delete_branch(checkout, branch);
        if let Some(record) = self.inner.threads.write().await.get_mut(thread_id) {
            record.merged = true;
            record.worktree_path = None;
            record.branch = None;
        }
        self.persist_registry().await;
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

    /// Switches a thread's provider/model (rejected while a run is active).
    pub async fn set_provider(
        &self,
        thread_id: String,
        provider: ProviderKind,
        model: Option<String>,
    ) -> Result<ThreadInfo, String> {
        let info = {
            let mut threads = self.inner.threads.write().await;
            let record = threads
                .get_mut(&thread_id)
                .ok_or_else(|| format!("unknown thread '{thread_id}'"))?;
            if record.running {
                return Err(format!(
                    "thread '{thread_id}' is busy: cannot switch provider mid-run"
                ));
            }
            record.provider = provider;
            record.model = model.unwrap_or_default();
            thread_info(record)
        };
        self.persist_registry().await;
        Ok(info)
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

    /// Returns the current settings.
    pub async fn get_settings(&self) -> Settings {
        self.inner.settings.get().await
    }

    /// Returns the diagnostics snapshot: crate version, OS, a settings clone
    /// (which holds no secret values), and recent command failures.
    pub async fn get_diagnostics(&self) -> Diagnostics {
        Diagnostics {
            app_version: env!("CARGO_PKG_VERSION").to_owned(),
            os: std::env::consts::OS.to_owned(),
            settings: self.inner.settings.get().await,
            recent_errors: self.recent_errors(),
        }
    }

    /// Applies a settings patch (validated) and returns the new settings.
    pub async fn update_settings(&self, patch: SettingsPatch) -> Result<Settings, String> {
        self.inner.settings.update(patch).await
    }

    /// Reports which providers have a key stored (values never leave the store).
    pub async fn get_secret_status(&self) -> SecretStatus {
        SecretStatus {
            go: self.inner.secrets.has("go"),
            openai: self.inner.secrets.has("openai"),
            anthropic: self.inner.secrets.has("anthropic"),
        }
    }

    /// Stores the API key for `provider`.
    pub async fn set_secret(&self, provider: String, value: String) -> Result<(), String> {
        let key = secret_key(&provider)?;
        if value.trim().is_empty() {
            return Err("secret value must not be empty".to_owned());
        }
        self.inner.secrets.set(key, &value)
    }

    /// Deletes the API key for `provider` (missing keys are a no-op success).
    pub async fn clear_secret(&self, provider: String) -> Result<(), String> {
        let key = secret_key(&provider)?;
        self.inner.secrets.clear(key)
    }

    /// Resolves the provider, builds tools, and spawns the run task.
    async fn spawn_run(
        &self,
        sink: &Arc<dyn EventSink>,
        thread_id: &str,
        run_id: &str,
        snapshot: RunSnapshot,
        task: String,
        max_turns: usize,
    ) -> Result<(), String> {
        let core_kind = snapshot.provider.core_kind();
        let api_key = self.api_key_for(snapshot.provider)?;
        if core_kind == themis_core::providers::ProviderKind::Go {
            check_go_model(&snapshot.model)?;
        }
        let mut config = ProviderConfig::new(core_kind, api_key);
        config.reasoning_effort = snapshot.reasoning_effort;
        config.session_id = Some(thread_id.to_owned());
        if !snapshot.model.trim().is_empty() {
            config = config.with_model(snapshot.model.clone());
        }
        if core_kind == themis_core::providers::ProviderKind::Custom {
            match self.custom_base_url() {
                Some(url) => config = config.with_base_url(url),
                None => {
                    return Err(format!(
                        "provider 'custom' needs a base URL: set the \
                         {CUSTOM_BASE_URL_ENV_VAR} environment variable"
                    ));
                }
            }
        }
        let llm = themis_core::providers::resolve(&config)
            .await
            .map_err(|err| {
                format!(
                    "failed to resolve provider '{}': {err:#}",
                    snapshot.provider.as_str()
                )
            })?;
        let hook = Arc::new(
            DesktopApprovalHook::new(
                thread_id.to_owned(),
                snapshot.is_git,
                Arc::clone(sink),
                Arc::clone(&self.inner.pending),
            )
            .with_timeout(std::time::Duration::from_secs(u64::from(
                snapshot.approval_timeout_seconds,
            )))
            .with_read_approval(snapshot.confirm_reads),
        );
        let approvals: Arc<dyn ApprovalHook> = CachingApprovals::wrap(hook);
        // Materialize skill scripts into the run workroot BEFORE building
        // tools: a failure aborts the spawn (the caller resets `running`),
        // so a run never starts half-skilled.
        if let Err(err) = materialize_scripts(&snapshot.skills, &snapshot.work_root) {
            return Err(format!("failed to materialize skill scripts: {err:#}"));
        }
        let tools = boxed_tools(&snapshot.work_root, Arc::clone(&approvals))
            .map_err(|err| format!("failed to build tools: {err:#}"))?;
        let skills = snapshot.skills;
        let stopped = self
            .inner
            .stop_flags
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .get(thread_id)
            .cloned()
            .ok_or_else(|| "Run is no longer active".to_owned())?;

        let state = self.clone();
        let thread_id = thread_id.to_owned();
        let run_id = run_id.to_owned();
        let sink = Arc::clone(sink);
        tauri::async_runtime::spawn(async move {
            let (events_tx, mut events_rx) =
                tokio::sync::mpsc::channel::<RunEvent>(EVENT_CHANNEL_CAPACITY);
            let pump_sink = Arc::clone(&sink);
            let pump_thread = thread_id.clone();
            let pump_run = run_id.clone();
            let pump_state = state.clone();
            let pump_thread_id = thread_id.clone();
            let terminated = Arc::new(AtomicBool::new(false));
            let pump_terminated = Arc::clone(&terminated);
            let pump = tokio::spawn(async move {
                while let Some(event) = events_rx.recv().await {
                    // A terminal event means the run is over: mark the thread
                    // idle BEFORE emitting, so observers (merge/discard,
                    // provider switch) never see `Finished` on a busy thread.
                    let terminal = matches!(
                        event,
                        themis_core::runtime::RunEvent::Finished { .. }
                            | themis_core::runtime::RunEvent::Failed { .. }
                    );
                    let envelope = ThreadEventEnvelope {
                        thread_id: pump_thread.clone(),
                        run_id: pump_run.clone(),
                        event: ThreadEvent::from(event.clone()),
                    };
                    if let Err(error) = pump_state.inner.transcript.append_event(&envelope) {
                        pump_state.record_error("save_thread_event", error);
                    }
                    if terminal {
                        pump_state.finish_run(&pump_thread_id).await;
                        pump_terminated.store(true, Ordering::SeqCst);
                        // Automation attribution runs while the thread is
                        // idle but BEFORE the terminal event goes out: the
                        // idle-before-terminal invariant is preserved, and by
                        // the time observers see the terminal event the review
                        // item (if any) already exists.
                        pump_state
                            .complete_automation_run(&pump_sink, &pump_run, &event)
                            .await;
                    }
                    pump_sink.emit_thread_event(&envelope);
                }
            });
            // A panicking run must neither stick the thread busy nor leave the
            // UI waiting: join the run, then synthesize the terminal event.
            let run_outcome = tokio::spawn(async move {
                run_task_with_stop(
                    llm,
                    themis_core::skills::filter_tools(tools, &skills),
                    themis_core::skills::compose_task(&task, &skills),
                    snapshot.history,
                    approvals,
                    max_turns,
                    events_tx,
                    stopped,
                )
                .await
            })
            .await;
            let _pump = pump.await;
            if !terminated.load(Ordering::SeqCst) {
                state.finish_run(&thread_id).await;
            }
            // Synthesize the terminal event only if the run died without
            // emitting one (a panic after a terminal send must not double it).
            if let Err(join_error) = run_outcome {
                if terminated.load(Ordering::SeqCst) {
                    return;
                }
                let error = format!("agent run panicked: {join_error}");
                // The pump never saw a terminal event, so attribute here; the
                // take-once registry makes a double count impossible.
                state
                    .finish_automation_run(&sink, &run_id, format!("failed: {error}"))
                    .await;
                let envelope = ThreadEventEnvelope {
                    thread_id: thread_id.clone(),
                    run_id: run_id.clone(),
                    event: ThreadEvent::Failed { error },
                };
                if let Err(error) = state.inner.transcript.append_event(&envelope) {
                    state.record_error("save_thread_event", error);
                }
                sink.emit_thread_event(&envelope);
            }
        });
        Ok(())
    }

    /// Looks up the API key for `provider` (Ollama needs none).
    fn api_key_for(&self, provider: ProviderKind) -> Result<String, String> {
        if provider == ProviderKind::Ollama {
            return Ok(String::new());
        }
        let name = provider.as_str();
        self.inner
            .secrets
            .get(name)
            .filter(|key| !key.trim().is_empty())
            .ok_or_else(|| {
                format!("no API key stored for provider '{name}': add one in Settings, then retry")
            })
    }

    /// Custom base URL: test override first, then the environment fallback.
    fn custom_base_url(&self) -> Option<String> {
        if let Some(url) = self
            .inner
            .custom_base_url_override
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .clone()
        {
            return Some(url);
        }
        std::env::var(CUSTOM_BASE_URL_ENV_VAR)
            .ok()
            .filter(|url| !url.trim().is_empty())
    }

    /// Marks `thread_id` idle, releases its concurrency permit, and persists.
    ///
    /// Every run path ends here: normal completion, run failure, and
    /// spawn-time errors (via the `send_message` failure branch).
    async fn finish_run(&self, thread_id: &str) {
        self.inner
            .stop_flags
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .remove(thread_id);
        // Idempotent: the terminal-event pump and the run tail both call this,
        // but the concurrency permit is released exactly once.
        let was_running = self
            .inner
            .threads
            .write()
            .await
            .get_mut(thread_id)
            .map(|record| std::mem::replace(&mut record.running, false))
            .unwrap_or(false);
        if was_running {
            let _ = self.inner.running_count.fetch_update(
                Ordering::SeqCst,
                Ordering::SeqCst,
                |count| count.checked_sub(1),
            );
        }
        self.persist_registry().await;
    }

    /// Lists all skills, ordered by id.
    pub async fn list_skills(&self) -> Vec<Skill> {
        let skills = self.inner.skills.read().await;
        let mut out: Vec<Skill> = skills.values().cloned().collect();
        out.sort_by(|left, right| left.id.cmp(&right.id));
        out
    }

    /// Creates a skill after validating the input with
    /// `themis-core` (unknown tool names fail with an error listing every
    /// valid name).
    pub async fn create_skill(&self, input: SkillInput) -> Result<Skill, String> {
        validate_skill_shapes(&input)?;
        let skill = Skill {
            id: uuid::Uuid::new_v4().to_string(),
            name: input.name,
            description: input.description,
            instructions: input.instructions,
            allowed_tools: input.allowed_tools,
            scripts: input.scripts,
        };
        self.inner
            .skills
            .write()
            .await
            .insert(skill.id.clone(), skill.clone());
        self.persist_skills().await;
        Ok(skill)
    }

    /// Replaces the skill `skill_id` with `input` (the id is kept).
    pub async fn update_skill(&self, skill_id: String, input: SkillInput) -> Result<Skill, String> {
        validate_skill_shapes(&input)?;
        let mut skills = self.inner.skills.write().await;
        let skill = skills
            .get_mut(&skill_id)
            .ok_or_else(|| format!("unknown skill '{skill_id}'"))?;
        skill.name = input.name;
        skill.description = input.description;
        skill.instructions = input.instructions;
        skill.allowed_tools = input.allowed_tools;
        skill.scripts = input.scripts;
        let updated = skill.clone();
        drop(skills);
        self.persist_skills().await;
        Ok(updated)
    }

    /// Deletes a skill and strips its id from every thread and automation,
    /// so no dangling references survive.
    pub async fn delete_skill(&self, skill_id: String) -> Result<(), String> {
        {
            let mut skills = self.inner.skills.write().await;
            if skills.remove(&skill_id).is_none() {
                return Err(format!("unknown skill '{skill_id}'"));
            }
        }
        {
            let mut threads = self.inner.threads.write().await;
            for record in threads.values_mut() {
                record.skill_ids.retain(|id| id != &skill_id);
            }
        }
        {
            let mut automations = self.inner.automations.write().await;
            for automation in automations.values_mut() {
                automation.skill_ids.retain(|id| id != &skill_id);
            }
        }
        self.persist_skills().await;
        self.persist_registry().await;
        self.persist_automations().await;
        Ok(())
    }

    /// Attaches `skill_ids` to an idle thread (every id must exist).
    pub async fn set_thread_skills(
        &self,
        thread_id: String,
        skill_ids: Vec<String>,
    ) -> Result<ThreadInfo, String> {
        self.check_skill_ids(&skill_ids).await?;
        let info = {
            let mut threads = self.inner.threads.write().await;
            let record = threads
                .get_mut(&thread_id)
                .ok_or_else(|| format!("unknown thread '{thread_id}'"))?;
            if record.running {
                return Err(format!(
                    "thread '{thread_id}' is busy: wait for the run to finish \
                     before changing its skills"
                ));
            }
            record.skill_ids = dedup_ids(skill_ids);
            thread_info(record)
        };
        self.persist_registry().await;
        Ok(info)
    }

    /// Rejects unknown skill ids, naming every bad one.
    async fn check_skill_ids(&self, skill_ids: &[String]) -> Result<(), String> {
        let skills = self.inner.skills.read().await;
        let bad: Vec<&str> = skill_ids
            .iter()
            .map(String::as_str)
            .filter(|id| !skills.contains_key(*id))
            .collect();
        if bad.is_empty() {
            Ok(())
        } else {
            Err(format!("unknown skill id(s): {}", bad.join(", ")))
        }
    }

    /// Lists all automations, ordered by id.
    pub async fn list_automations(&self) -> Vec<Automation> {
        let automations = self.inner.automations.read().await;
        let mut out: Vec<Automation> = automations.values().cloned().collect();
        out.sort_by(|left, right| left.id.cmp(&right.id));
        out
    }

    /// Creates an automation after validating the input. The project root
    /// must exist (stored canonicalized) and `next_run_at` starts at
    /// now + `interval_mins`.
    pub async fn create_automation(&self, input: AutomationInput) -> Result<Automation, String> {
        self.check_automation_input(&input).await?;
        let root = canonical_project_dir(&input.project_root)?;
        let now = Utc::now();
        let interval_mins = u32::try_from(input.interval_mins).map_err(|_| {
            format!(
                "interval_mins is out of range (got {})",
                input.interval_mins
            )
        })?;
        let automation = Automation {
            id: uuid::Uuid::new_v4().to_string(),
            name: input.name,
            project_root: root.to_string_lossy().into_owned(),
            provider: input.provider,
            model: input.model,
            skill_ids: dedup_ids(input.skill_ids),
            interval_mins,
            task: input.task,
            enabled: input.enabled,
            last_run_at: None,
            next_run_at: rfc3339_plus_minutes(now, i64::from(interval_mins)),
            run_count: 0,
        };
        self.inner
            .automations
            .write()
            .await
            .insert(automation.id.clone(), automation.clone());
        self.persist_automations().await;
        Ok(automation)
    }

    /// Replaces the automation `automation_id` with `input`, keeping its id,
    /// history (`last_run_at`, `run_count`), and rescheduling `next_run_at`
    /// to now + the new interval.
    pub async fn update_automation(
        &self,
        automation_id: String,
        input: AutomationInput,
    ) -> Result<Automation, String> {
        self.check_automation_input(&input).await?;
        let root = canonical_project_dir(&input.project_root)?;
        let interval_mins = u32::try_from(input.interval_mins).map_err(|_| {
            format!(
                "interval_mins is out of range (got {})",
                input.interval_mins
            )
        })?;
        let now = Utc::now();
        let mut automations = self.inner.automations.write().await;
        let automation = automations
            .get_mut(&automation_id)
            .ok_or_else(|| format!("unknown automation '{automation_id}'"))?;
        automation.name = input.name;
        automation.project_root = root.to_string_lossy().into_owned();
        automation.provider = input.provider;
        automation.model = input.model;
        automation.skill_ids = dedup_ids(input.skill_ids);
        automation.interval_mins = interval_mins;
        automation.task = input.task;
        automation.enabled = input.enabled;
        automation.next_run_at = rfc3339_plus_minutes(now, i64::from(interval_mins));
        let updated = automation.clone();
        drop(automations);
        self.persist_automations().await;
        Ok(updated)
    }

    /// Deletes an automation. Threads it created stay as ordinary threads
    /// and its review items stay as history.
    pub async fn delete_automation(&self, automation_id: String) -> Result<(), String> {
        let mut automations = self.inner.automations.write().await;
        if automations.remove(&automation_id).is_none() {
            return Err(format!("unknown automation '{automation_id}'"));
        }
        drop(automations);
        self.persist_automations().await;
        Ok(())
    }

    /// Flips an automation's `enabled` flag. Enabling reschedules
    /// `next_run_at` to now + the interval.
    pub async fn set_automation_enabled(
        &self,
        automation_id: String,
        enabled: bool,
    ) -> Result<Automation, String> {
        let now = Utc::now();
        let mut automations = self.inner.automations.write().await;
        let automation = automations
            .get_mut(&automation_id)
            .ok_or_else(|| format!("unknown automation '{automation_id}'"))?;
        automation.enabled = enabled;
        if enabled {
            automation.next_run_at = rfc3339_plus_minutes(now, i64::from(automation.interval_mins));
        }
        let updated = automation.clone();
        drop(automations);
        self.persist_automations().await;
        Ok(updated)
    }

    /// Validates automation input: non-empty name and task, an existing
    /// project dir, a usable Go model, existing skill ids, and
    /// `interval_mins >= 1`.
    async fn check_automation_input(&self, input: &AutomationInput) -> Result<(), String> {
        if input.name.trim().is_empty() {
            return Err("automation name must not be empty".to_owned());
        }
        if input.task.trim().is_empty() {
            return Err("automation task must not be empty".to_owned());
        }
        if input.interval_mins < 1 {
            return Err(format!(
                "interval_mins must be at least 1 (got {})",
                input.interval_mins
            ));
        }
        canonical_project_dir(&input.project_root)?;
        if input.provider.core_kind() == themis_core::providers::ProviderKind::Go {
            check_go_model(&input.model)?;
        }
        self.check_skill_ids(&input.skill_ids).await?;
        Ok(())
    }

    /// Test hook: forcibly reschedules an automation (lets tick tests make
    /// one due without waiting out its interval).
    pub async fn set_automation_next_run_for_test(
        &self,
        automation_id: &str,
        next_run_at: String,
    ) -> Result<Automation, String> {
        let mut automations = self.inner.automations.write().await;
        let automation = automations
            .get_mut(automation_id)
            .ok_or_else(|| format!("unknown automation '{automation_id}'"))?;
        automation.next_run_at = next_run_at;
        let updated = automation.clone();
        drop(automations);
        self.persist_automations().await;
        Ok(updated)
    }

    /// Manually triggers one automation run. Unlike the scheduler tick, this
    /// works even when the automation is disabled or the global
    /// `automations_enabled` kill-switch is off — but the concurrency gate
    /// still applies.
    pub async fn run_automation_now(
        &self,
        sink: Arc<dyn EventSink>,
        automation_id: String,
    ) -> Result<RunAutomationNow, String> {
        let automation = {
            let automations = self.inner.automations.read().await;
            automations
                .get(&automation_id)
                .cloned()
                .ok_or_else(|| format!("unknown automation '{automation_id}'"))?
        };
        if !Path::new(&automation.project_root).is_dir() {
            return Err(format!(
                "automation '{}' cannot run: project_root '{}' no longer exists",
                automation.id, automation.project_root
            ));
        }
        let (thread_id, run_id) = self.fire_automation(&sink, &automation).await?;
        Ok(RunAutomationNow {
            automation_id: automation.id,
            thread_id,
            run_id,
        })
    }

    /// Runs one scheduler pass: fires every due automation (enabled, with
    /// `next_run_at <= now`). A saturated concurrency gate or a vanished
    /// project dir skips the automation WITHOUT advancing its schedule, so
    /// it is retried on the next tick. No-op while the global
    /// `automations_enabled` kill-switch is off.
    ///
    /// Firing only ever creates a thread plus a run; there is deliberately
    /// no code path from the scheduler to [`AppState::merge_thread`] —
    /// automation output always waits for human review.
    pub async fn tick_automations_once(&self, sink: &Arc<dyn EventSink>) {
        if !self.inner.settings.get().await.automations_enabled {
            return;
        }
        let now = Utc::now();
        let due: Vec<Automation> = {
            let automations = self.inner.automations.read().await;
            let mut due: Vec<Automation> = automations
                .values()
                .filter(|automation| {
                    automation.enabled && next_run_due(&automation.next_run_at, now)
                })
                .cloned()
                .collect();
            due.sort_by(|left, right| left.id.cmp(&right.id));
            due
        };
        for automation in due {
            if !Path::new(&automation.project_root).is_dir() {
                eprintln!(
                    "themis: automation '{}' skipped: project_root '{}' no longer exists",
                    automation.id, automation.project_root
                );
                continue;
            }
            if let Err(err) = self.fire_automation(sink, &automation).await {
                eprintln!("themis: automation '{}' did not fire: {err}", automation.id);
            }
        }
    }

    /// Fires one automation: creates its thread (provider/model/skills from
    /// the automation, titled `Automation <name> — <timestamp>`) and sends
    /// the automation task as the run.
    async fn fire_automation(
        &self,
        sink: &Arc<dyn EventSink>,
        automation: &Automation,
    ) -> Result<(String, String), String> {
        // Gate pre-check (the send path re-checks atomically under the state
        // lock; this avoids minting a stray empty thread in the common
        // saturated case).
        let limit = usize::try_from(self.inner.settings.get().await.concurrency_limit)
            .unwrap_or(usize::MAX);
        if self.inner.running_count.load(Ordering::SeqCst) >= limit {
            return Err(format!(
                "concurrency limit reached: automation '{}' stays due for the next tick",
                automation.id
            ));
        }
        let now = Utc::now();
        let info = self
            .create_thread(
                automation.project_root.clone(),
                automation.provider,
                Some(automation.model.clone()),
            )
            .await?;
        let title = format!(
            "Automation {} — {}",
            automation.name,
            now.to_rfc3339_opts(SecondsFormat::Secs, true)
        );
        {
            let mut threads = self.inner.threads.write().await;
            let record = threads
                .get_mut(&info.id)
                .ok_or_else(|| format!("automation thread '{}' vanished", info.id))?;
            record.title = title.clone();
            record.titled = true;
            record.skill_ids = automation.skill_ids.clone();
        }
        self.persist_registry().await;
        let run_id = uuid::Uuid::new_v4().to_string();
        // Register BEFORE spawning: the pump attributes terminal events via
        // this map, and registering after the spawn could miss a fast failure.
        self.inner
            .automation_runs
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .insert(
                run_id.clone(),
                AutomationRunContext {
                    automation_id: automation.id.clone(),
                    thread_id: info.id.clone(),
                    title,
                },
            );
        match self
            .send_message_with_id(
                Arc::clone(sink),
                info.id.clone(),
                automation.task.clone(),
                run_id.clone(),
            )
            .await
        {
            Ok(_) => Ok((info.id, run_id)),
            Err(err) => {
                self.inner
                    .automation_runs
                    .lock()
                    .unwrap_or_else(|poisoned| poisoned.into_inner())
                    .remove(&run_id);
                if err.contains(GATE_SATURATED_MARKER) {
                    // Lost a gate race with another run: skip WITHOUT
                    // advancing; the automation stays due for the next tick.
                    return Err(err);
                }
                // No run exists, so no review item is owed — but advance the
                // schedule so a persistently broken automation (missing key,
                // bad model) doesn't error-loop every tick.
                self.record_automation_spawn_failure(&automation.id).await;
                Err(err)
            }
        }
    }

    /// Advances an automation's schedule after a spawn failure (no run, so no
    /// review item and no `last_run_at` run record beyond the timestamp).
    async fn record_automation_spawn_failure(&self, automation_id: &str) {
        let now = Utc::now();
        {
            let mut automations = self.inner.automations.write().await;
            if let Some(automation) = automations.get_mut(automation_id) {
                automation.last_run_at = Some(now.to_rfc3339_opts(SecondsFormat::Secs, true));
                automation.next_run_at =
                    rfc3339_plus_minutes(now, i64::from(automation.interval_mins));
                automation.run_count += 1;
            }
        }
        self.persist_automations().await;
    }

    /// Attributes a terminal run event to its automation (a no-op for
    /// ordinary runs). Called from the run-event pump while the thread is
    /// already idle but before the terminal event is emitted.
    async fn complete_automation_run(
        &self,
        sink: &Arc<dyn EventSink>,
        run_id: &str,
        event: &RunEvent,
    ) {
        let summary = match event {
            RunEvent::Finished { result } => result.clone(),
            RunEvent::Failed { error } => format!("failed: {error}"),
            _ => return,
        };
        self.finish_automation_run(sink, run_id, summary).await;
    }

    /// Records one finished automation run: creates the pending review item,
    /// emits `review-item-added`, and advances the automation's schedule
    /// (`last_run_at`/`next_run_at`/`run_count`) on success and failure
    /// alike. The take-once registry makes double counting impossible; an
    /// automation deleted mid-run keeps its review item as history with no
    /// schedule to advance.
    async fn finish_automation_run(
        &self,
        sink: &Arc<dyn EventSink>,
        run_id: &str,
        summary: String,
    ) {
        let context = self
            .inner
            .automation_runs
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .remove(run_id);
        let Some(context) = context else {
            return;
        };
        let now = Utc::now();
        let item = ReviewItem {
            id: uuid::Uuid::new_v4().to_string(),
            automation_id: context.automation_id.clone(),
            thread_id: context.thread_id,
            created_at: now.to_rfc3339_opts(SecondsFormat::Secs, true),
            title: context.title,
            summary,
            status: ReviewStatus::Pending,
        };
        {
            let mut reviews = self.inner.reviews.write().await;
            reviews.insert(item.id.clone(), item.clone());
        }
        self.persist_reviews().await;
        sink.emit_review_item(&item);
        {
            let mut automations = self.inner.automations.write().await;
            if let Some(automation) = automations.get_mut(&context.automation_id) {
                automation.last_run_at = Some(now.to_rfc3339_opts(SecondsFormat::Secs, true));
                automation.next_run_at =
                    rfc3339_plus_minutes(now, i64::from(automation.interval_mins));
                automation.run_count += 1;
            }
        }
        self.persist_automations().await;
    }

    /// Lists review items, optionally filtered by status, ordered by
    /// (`created_at`, `id`).
    pub async fn list_review_items(&self, status: Option<ReviewStatus>) -> Vec<ReviewItem> {
        let reviews = self.inner.reviews.read().await;
        let mut out: Vec<ReviewItem> = reviews
            .values()
            .filter(|item| status.is_none_or(|wanted| item.status == wanted))
            .cloned()
            .collect();
        out.sort_by(|left, right| {
            left.created_at
                .cmp(&right.created_at)
                .then_with(|| left.id.cmp(&right.id))
        });
        out
    }

    /// Dismisses a review item, returning the updated item.
    pub async fn dismiss_review_item(&self, review_id: String) -> Result<ReviewItem, String> {
        let mut reviews = self.inner.reviews.write().await;
        let item = reviews
            .get_mut(&review_id)
            .ok_or_else(|| format!("unknown review item '{review_id}'"))?;
        item.status = ReviewStatus::Dismissed;
        let updated = item.clone();
        drop(reviews);
        self.persist_reviews().await;
        Ok(updated)
    }

    /// Marks a review item continued and returns its thread. The thread is
    /// resolved first, so continuing a review whose thread was discarded
    /// errors WITHOUT flipping the status.
    pub async fn continue_review_item(&self, review_id: String) -> Result<ThreadInfo, String> {
        let thread_id = {
            let reviews = self.inner.reviews.read().await;
            reviews
                .get(&review_id)
                .map(|item| item.thread_id.clone())
                .ok_or_else(|| format!("unknown review item '{review_id}'"))?
        };
        let info = self.get_thread(&thread_id).await?;
        {
            let mut reviews = self.inner.reviews.write().await;
            if let Some(item) = reviews.get_mut(&review_id) {
                item.status = ReviewStatus::Continued;
            }
        }
        self.persist_reviews().await;
        Ok(info)
    }
}

fn thread_info(record: &ThreadRecord) -> ThreadInfo {
    ThreadInfo {
        id: record.id.clone(),
        title: record.title.clone(),
        provider: record.provider,
        model: record.model.clone(),
        running: record.running,
        worktree_path: None,
        branch: None,
        base_branch: None,
        recovered: record.recovered,
        skill_ids: record.skill_ids.clone(),
    }
}

/// App data dir holding the settings file (`.` when the path has no parent).
fn settings_app_dir(settings_path: &Path) -> PathBuf {
    settings_path
        .parent()
        .map(Path::to_path_buf)
        .filter(|parent| !parent.as_os_str().is_empty())
        .unwrap_or_else(|| PathBuf::from("."))
}

/// Diff and discard operations use the same shared checkout as agent tools.
fn diff_root_for(record: &ThreadRecord) -> Result<PathBuf, String> {
    Ok(record.project_root.clone())
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
    for automation in automations.values_mut() {
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

/// Rejects everything but `go|openai|anthropic|custom` secret slots.
fn secret_key(provider: &str) -> Result<&'static str, String> {
    match provider.trim().to_lowercase().as_str() {
        "go" => Ok("go"),
        "openai" => Ok("openai"),
        "anthropic" => Ok("anthropic"),
        "custom" => Ok("custom"),
        other => Err(format!(
            "unknown secret provider '{other}': expected go, openai, anthropic, or custom"
        )),
    }
}

/// Validates skill input with `themis-core` (unknown tool names fail with
/// an error listing every valid name).
fn validate_skill_shapes(input: &SkillInput) -> Result<(), String> {
    let scripts: Vec<themis_core::skills::SkillScript> = input
        .scripts
        .iter()
        .map(|script| themis_core::skills::SkillScript {
            name: script.name.clone(),
            content: script.content.clone(),
        })
        .collect();
    validate_skill_input(
        &input.name,
        &input.instructions,
        &input.allowed_tools,
        &scripts,
    )
    .map_err(|err| err.to_string())
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

/// Mirrors the CLI rule: Go never runs on a missing or placeholder model.
fn check_go_model(model: &str) -> Result<(), String> {
    let effective = if model.trim().is_empty() {
        std::env::var(GO_MODEL_ENV_VAR)
            .ok()
            .filter(|name| !name.trim().is_empty())
    } else {
        Some(model.to_owned())
    };
    match effective {
        None => Err(format!(
            "provider 'go' requires an explicit model: set one for the thread \
             or set {GO_MODEL_ENV_VAR}"
        )),
        Some(name) if name == GO_DEFAULT_MODEL => Err(format!(
            "refusing to use the '{GO_DEFAULT_MODEL}' placeholder model: set a real model ID"
        )),
        Some(_) => Ok(()),
    }
}

/// Tracked-file diffs, with a staged+unstaged fallback for repos without `HEAD`.
fn tracked_diff_files(root: &Path) -> Result<Vec<crate::types::DiffFile>, String> {
    if git_head(root).is_some() {
        return git_diff_head(root);
    }
    let mut seen = HashSet::new();
    let mut files = Vec::new();
    for file in git_diff_cached(root)?
        .into_iter()
        .chain(git_diff_worktree(root)?)
    {
        if seen.insert(file.path.clone()) {
            files.push(file);
        }
    }
    Ok(files)
}

/// Rejects absolute paths and `..` escapes (diff paths must stay in-root).
fn ensure_root_relative(path: &str) -> Result<(), String> {
    let candidate = Path::new(path);
    if candidate.is_absolute()
        || candidate
            .components()
            .any(|component| matches!(component, Component::ParentDir))
    {
        return Err(format!(
            "invalid path '{path}': must be relative to the project root"
        ));
    }
    Ok(())
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
                ProviderKind::OpenAI,
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
    async fn send_message_requires_a_stored_key_and_resets_running() {
        let (state, dir) = test_state();
        let thread = state
            .create_thread(
                dir.path().to_string_lossy().into_owned(),
                ProviderKind::OpenAI,
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
    async fn go_requires_an_explicit_model() {
        let (state, dir) = test_state();
        state
            .set_secret("go".to_owned(), "key".to_owned())
            .await
            .expect("store key");
        let thread = state
            .create_thread(
                dir.path().to_string_lossy().into_owned(),
                ProviderKind::Go,
                None,
            )
            .await
            .expect("create");
        let (sink, _rx) = crate::sink::ChannelSink::channel();
        let err = state
            .send_message(Arc::new(sink), thread.id.clone(), "hi".to_owned())
            .await
            .expect_err("model required");
        assert!(err.contains("explicit model"), "{err}");
    }

    #[tokio::test]
    async fn first_message_sets_a_truncated_title() {
        let (state, dir) = test_state();
        let thread = state
            .create_thread(
                dir.path().to_string_lossy().into_owned(),
                ProviderKind::Ollama,
                None,
            )
            .await
            .expect("create");
        assert_eq!(thread.title, UNTITLED_THREAD);
        // Ollama needs no key, so the run spawns (and fails fast with no
        // server); the title is set synchronously before that.
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
        assert_eq!(settings, Settings::default());
        let updated = state
            .update_settings(SettingsPatch {
                theme: Some(ThemeMode::Light),
                max_turns: Some(7),
                ..SettingsPatch::default()
            })
            .await
            .expect("update");
        assert_eq!(updated.theme, ThemeMode::Light);
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
        assert_eq!(
            status,
            SecretStatus {
                go: false,
                openai: false,
                anthropic: false,
            }
        );
        assert!(state
            .set_secret("openai".to_owned(), String::new())
            .await
            .is_err());
        assert!(state
            .set_secret("ollama".to_owned(), "x".to_owned())
            .await
            .is_err());
        state
            .set_secret("openai".to_owned(), "sk-test".to_owned())
            .await
            .expect("set");
        let status = state.get_secret_status().await;
        assert!(status.openai && !status.go);
        // The status JSON carries presence only.
        let json = serde_json::to_string(&status).expect("json");
        assert!(!json.contains("sk-test"));
        state
            .clear_secret("openai".to_owned())
            .await
            .expect("clear");
        assert!(!state.get_secret_status().await.openai);
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
            provider: ProviderKind::Custom,
            model: "test-model".to_owned(),
            skill_ids: Vec::new(),
            interval_mins: 60,
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
        let err = state
            .create_automation(go_no_model)
            .await
            .expect_err("go model required");
        assert!(err.contains("explicit model"), "{err}");

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
        assert!(state.list_automations().await.is_empty());
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
            .create_thread(project, ProviderKind::Custom, Some("m".to_owned()))
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
            .create_thread(project, ProviderKind::Custom, Some("m".to_owned()))
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
            provider: ProviderKind::Custom,
            model: "m".to_owned(),
            skill_ids: Vec::new(),
            interval_mins: 30,
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
