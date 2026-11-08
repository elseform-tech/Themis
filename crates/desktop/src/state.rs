//! Application state and handler logic.
//!
//! [`AppState`] owns projects, threads, pending approvals, settings, and
//! secrets. Every Tauri command in [`crate::commands`] is a thin wrapper over
//! one of these plain async methods, which take an [`EventSink`] instead of
//! touching Tauri — that keeps them directly drivable from headless tests.

mod attachments;
mod auto_review;
mod automation;
mod background;
mod browser;
mod browser_data;
mod changes;
mod chat_grants;
pub(crate) mod configuration;
pub(crate) mod definition_test;
pub(crate) mod editors;
mod goal;
mod goal_tool;
mod hook_sessions;
mod hook_wakes;
mod integration_tools;
mod keys;
mod mcp;
mod mcp_consent;
mod mcp_file;
mod media_tool;
mod memory;
mod memory_auto;
mod memory_tool;
mod permissions;
mod plugins;
mod preferences;
mod projects;
mod providers;
mod question_tool;
mod questions;
mod queue;
mod reviews;
mod run;
pub(crate) mod searxng;

/// Test support: stops the built-in search server.
#[doc(hidden)]
pub fn searxng_stop_for_test() {
    searxng::stop();
}
mod skills;
mod subagent_tools;
mod subagents;
mod terminal;
mod threads;
mod usage;
mod workspace;
pub use browser::update_payload as browser_update_payload;
pub(crate) use hook_sessions::IDLE_CHECK_EVERY as HOOK_IDLE_CHECK_EVERY;
pub use terminal::TerminalSession;
/// Project path resolution, exposed for the `fuzz/` targets; not a stable API.
#[cfg(feature = "fuzzing")]
#[doc(hidden)]
pub use workspace::resolve_inside;

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
    ApprovalDecision, Automation, AutomationInput, AutomationRemoval, Diagnostics,
    DiagnosticsError, ProjectInfo, ProviderKind, ReviewItem, ReviewSource, ReviewStatus,
    RunAutomationNow, RunHandle, SecretStatus, Settings, SettingsPatch, Skill, SkillInput,
    ThreadComment, ThreadEvent, ThreadEventEnvelope, ThreadInfo,
};

/// Title assigned to threads before their first message.
pub const UNTITLED_THREAD: &str = "New thread";

/// Maximum title length taken from the first message, in chars.
pub const MAX_TITLE_CHARS: usize = 60;

/// Newest removal records kept for the Tasks view.
const REMOVAL_HISTORY_LIMIT: usize = 200;

/// Production scheduler cadence: the background task ticks this often, so a
/// short interval fires within about this many seconds of being due.
const SCHEDULER_TICK_SECS: u64 = 10;

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
    documents: Vec<themis_core::runtime::documents::DocumentSource>,
    approval_timeout_seconds: u32,
    approval_mode: themis_core::configuration::ApprovalMode,
    runtime_configuration: themis_core::configuration::RuntimeConfig,
    /// The sandbox settings a file or override wrote, which YOLO keeps.
    yolo_sandbox: themis_core::sandbox::YoloSandbox,
    /// Tool sandbox root: the shared project checkout.
    work_root: PathBuf,
    is_git: bool,
    model: String,
    /// Skills resolved from the thread's `skill_ids` (missing ids are
    /// skipped: every mutation keeps references consistent, so a miss only
    /// follows a hand-edited store file).
    skills: Vec<themis_core::skills::Skill>,
    plugins: Vec<themis_core::plugins::Plugin>,
    skill_catalog: Vec<themis_core::skills::Skill>,
    /// Short conversational message without attachments: a checkpoint may wait one turn.
    short_reply: bool,
    /// Started by an automation or watch: nobody is necessarily watching.
    automation_run: bool,
    /// The person's own words that started this run; `None` for a goal turn, a hook's wake or
    /// an automation (the memory pass reads only what the person said).
    person_text: Option<String>,
}

/// One automation run in flight: the completion registry entry that lets the
/// run-event pump attribute terminal events back to their automation.
struct AutomationRunContext {
    automation_id: String,
    thread_id: String,
    /// Thread title at spawn time, reused for the review item.
    title: String,
    /// Name and origin, copied onto the review item so it outlives deletion.
    source: ReviewSource,
    /// Watch check output hash to record once this run succeeds.
    check_hash: Option<String>,
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

/// A running run's steer inbox, the sink its events go to, and its id.
struct SteerEntry {
    inbox: themis_core::runtime::SteerInbox,
    sink: Arc<dyn EventSink>,
    run_id: String,
}

struct AppStateInner {
    stop_flags: std::sync::Mutex<HashMap<String, Arc<AtomicBool>>>,
    projects: tokio::sync::RwLock<HashMap<String, ProjectInfo>>,
    default_project_init: tokio::sync::Mutex<()>,
    /// Where deleted projects are moved (the user Trash; a temp folder in tests).
    trash_dir: PathBuf,
    /// The home folder personal skills are discovered in (`~/.agents`, `~/.codex`, `~/.claude`);
    /// none in tests, so they behave the same on every machine.
    personal_home: Option<PathBuf>,
    /// Whether runs may use the SearXNG built into Themis; off in tests, which must not depend on
    /// a runtime packed on the developer's machine.
    built_in_search: bool,
    threads: tokio::sync::RwLock<HashMap<String, ThreadRecord>>,
    pending: PendingMap,
    mcp: std::sync::Mutex<mcp::Connections>,
    settings: SettingsStore,
    transcript: TranscriptStore,
    /// Token usage per completed model request (`usage.sqlite3`).
    usage: Arc<crate::usage::UsageStore>,
    diagnostics: Arc<themis_core::diagnostics::DiagnosticLog>,
    secrets: Arc<dyn SecretStore>,
    go_base_url_override: std::sync::Mutex<Option<String>>,
    /// Test seam: home folder for sandbox plans (`None` uses `$HOME`).
    sandbox_home: std::sync::Mutex<Option<PathBuf>>,
    /// Test seam: every sandbox backend counts as missing.
    sandbox_backends_unavailable: AtomicBool,
    go_catalog: tokio::sync::RwLock<Vec<themis_core::providers::GoModel>>,
    /// Model lists read from configured providers' `/models`, by provider id, with the
    /// base URL they came from.
    provider_catalogs:
        tokio::sync::RwLock<HashMap<String, (String, Vec<themis_core::providers::GoModel>)>>,
    worktrees_root: PathBuf,
    registry_path: PathBuf,
    /// Per-chat approval grants (`chat_grants/<chat id>.json` in the app data dir).
    chat_grants_dir: PathBuf,
    /// Serializes agent management calls per plugin.
    plugin_locks: std::sync::Mutex<HashMap<String, Arc<tokio::sync::Mutex<()>>>>,
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
    /// Why automations ended (`automation_removals.json`), newest 200.
    removals: tokio::sync::RwLock<HashMap<String, AutomationRemoval>>,
    removals_path: PathBuf,
    /// Automations the scheduler is processing right now (check or spawn).
    /// Turns hooks have started in each chat since the person last wrote; see `hook_wakes.rs`.
    hook_wakes: std::sync::Mutex<HashMap<String, u32>>,
    /// Stands in for the person's browser in tests; absent, the system opens addresses.
    url_opener: std::sync::Mutex<Option<UrlOpener>>,
    watch_inflight: std::sync::Mutex<HashSet<String>>,
    /// In-flight automation runs (`run_id` -> context). Deliberately
    /// in-memory only: a run killed mid-flight by a shutdown has no mapping
    /// post-boot, so it yields no review item (documented recovery rule).
    automation_runs: std::sync::Mutex<HashMap<String, AutomationRunContext>>,
    /// Sink the background scheduler emits on (registered post-construction;
    /// the scheduler skips ticks until one is set).
    scheduler_sink: std::sync::Mutex<Option<Arc<dyn EventSink>>>,
    /// Background shell tasks of every chat (`background/` in the app data dir).
    background: Arc<themis_core::tools::background::BackgroundRegistry>,
    /// Built-in browsers (`browser/` in the app data dir: profiles, a downloaded Chromium).
    browsers: Arc<themis_core::browser::BrowserHub>,
    /// The server's own CPU and memory use (see `resources`).
    resources: Arc<crate::resources::Monitor>,
    /// Recent command failures, oldest-first, capped at [`MAX_RECENT_ERRORS`].
    recent_errors: std::sync::Mutex<VecDeque<DiagnosticsError>>,
    /// Per-chat prompt queues (`queues/<chat id>.json`), loaded on first use.
    queues: std::sync::Mutex<HashMap<String, Vec<crate::types::QueuedMessage>>>,
    queues_dir: PathBuf,
    /// Serializes queue changes, so a message is sent or edited exactly once.
    queue_lock: std::sync::Mutex<()>,
    /// Steer inboxes of running runs, by chat.
    /// Steer inboxes of running runs, by chat, with the sink of the run (to
    /// announce steers put back in the queue when the run ends).
    steer_inboxes: std::sync::Mutex<HashMap<String, SteerEntry>>,
    /// Each running chat's note channel: memory the memory models change mid-run reaches it.
    live_notes: std::sync::Mutex<HashMap<String, Arc<themis_core::runtime::RunNotes>>>,
    /// One memory job at a time, so a pass reads what the one before it saved (a turn's pass
    /// and a completed goal's would otherwise both save the same thing).
    memory_job_lock: tokio::sync::Mutex<()>,
    /// Per-chat questions to the user (`questions/<chat id>.json`), loaded on first use.
    questions: std::sync::Mutex<HashMap<String, Vec<crate::types::Question>>>,
    questions_dir: PathBuf,
    /// Each chat's subagents, live and recently ended.
    subagents: subagents::SubagentMap,
    /// The person's shells in the Workspace terminal.
    terminals: Arc<terminal::Terminals>,
    /// Serializes question changes.
    question_lock: std::sync::Mutex<()>,
    /// Auto mode: hard-block questions by id, with the chat and the exact action they ask about.
    auto_questions: std::sync::Mutex<std::collections::BTreeMap<String, (String, String)>>,
    /// Auto mode: exact actions the person allowed once after a block, by chat.
    auto_once: std::sync::Mutex<HashMap<String, Vec<String>>>,
    /// Memory jobs waiting for the memory models, and when idle work may run.
    memory_jobs: std::sync::Mutex<memory_auto::MemoryJobs>,
    /// Per-chat goals (`goals/<chat id>.json`), loaded on first use.
    goals: std::sync::Mutex<HashMap<String, Option<crate::types::Goal>>>,
    goals_dir: PathBuf,
    /// Serializes goal changes.
    goal_lock: std::sync::Mutex<()>,
    /// Runs started as goal turns (checked when they end).
    goal_turn_runs: std::sync::Mutex<HashSet<String>>,
    /// Open plugin hook sessions, by chat; see `hook_sessions.rs`.
    hook_sessions: std::sync::Mutex<HashMap<String, hook_sessions::OpenHookSession>>,
    /// Bumped whenever a waiting goal timer must no longer fire.
    goal_generations: std::sync::Mutex<HashMap<String, u64>>,
    /// Runs the server itself sends (queued messages, goal turns), by run id:
    /// the send path announces their user message once it is saved, and marks
    /// `true` when the run was admitted but failed to start (its end step then
    /// handles it, so the sender must not). The sender removes its entry.
    server_sends: std::sync::Mutex<HashMap<String, queue::ServerSend>>,
}

/// What the app takes from the machine it runs on: personal skills from the home folder and the
/// SearXNG packed with Themis. Tests take nothing, so they behave the same on every machine.
pub(crate) struct Machine {
    personal_home: Option<PathBuf>,
    built_in_search: bool,
}

impl Machine {
    fn real() -> Self {
        Self {
            personal_home: themis_core::sandbox::home_dir(),
            built_in_search: true,
        }
    }

    pub(crate) fn none() -> Self {
        Self {
            personal_home: None,
            built_in_search: false,
        }
    }
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

/// A fixed category for a failed command's message, so the diagnostics log says why a
/// command failed without recording the message itself (it can name paths or hold user
/// text). Unknown methods mean a window and the server come from different builds.
fn failure_category(message: &str) -> &'static str {
    let lower = message.to_ascii_lowercase();
    let has = |words: &[&str]| words.iter().any(|word| lower.contains(word));
    if lower.starts_with("unknown server method") {
        "unknown_method"
    } else if has(&["exceeds", "too large", "size limit"]) {
        "too_large"
    } else if lower.starts_with("invalid ") || has(&["missing field", "must be"]) {
        "invalid_request"
    } else if has(&["timed out", "timeout", "deadline"]) {
        "timeout"
    } else if has(&[
        "not found",
        "no such",
        "cannot find",
        "does not exist",
        "unknown thread",
        "unknown skill",
        "unknown secret",
    ]) {
        "not_found"
    } else if has(&[
        "revision",
        "already exists",
        "already running",
        "conflict",
        "is running",
        "busy",
    ]) {
        "conflict"
    } else if has(&["denied", "not allowed", "permission", "requires approval"]) {
        "denied"
    } else if has(&[
        "http",
        "connect",
        "dns",
        "network",
        "request failed",
        "unavailable",
        "offline",
    ]) {
        "network"
    } else if has(&["api key", "keychain", "credential", "unauthorized"]) {
        "credentials"
    } else {
        "other"
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
            std::env::var_os("HOME").map_or_else(
                || app_dir.join("trash"),
                |home| PathBuf::from(home).join(".Trash"),
            ),
            Machine::real(),
        );
        Self::spawn_automation_scheduler(state.clone());
        // Memory jobs run in the app; headless tests turn them on when they test them.
        state
            .inner
            .memory_jobs
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .enabled = true;
        state.inner.browsers.allow_engine_download();
        state
    }

    /// Creates state for headless tests: in-memory secrets, explicit settings
    /// path. Unlike [`AppState::new`], this never spawns the scheduler;
    /// tests drive [`AppState::tick_automations_once`] directly.
    #[must_use]
    pub fn new_for_test(settings_path: PathBuf) -> Self {
        Self::new_for_test_with_secret_store(settings_path, Arc::new(MemoryStore::new()))
    }

    /// Creates isolated headless state with an injected credential-store fixture.
    #[must_use]
    pub fn new_for_test_with_secret_store(
        settings_path: PathBuf,
        secrets: Arc<dyn SecretStore>,
    ) -> Self {
        seed_test_settings(&settings_path);
        let app_dir = settings_app_dir(&settings_path);
        let worktrees_root = app_dir.join("worktrees");
        let registry_path = app_dir.join("threads.json");
        Self::new_with_dirs(
            settings_path,
            worktrees_root,
            registry_path,
            secrets,
            app_dir.join("projects"),
            app_dir.join("trash"),
            Machine::none(),
        )
    }

    /// Creates state for headless tests with an explicit worktrees root (the
    /// registry stays beside the settings file). Never spawns the scheduler.
    #[must_use]
    pub fn new_for_test_with_worktrees_root(
        settings_path: PathBuf,
        worktrees_root: PathBuf,
    ) -> Self {
        seed_test_settings(&settings_path);
        let app_dir = settings_app_dir(&settings_path);
        let registry_path = app_dir.join("threads.json");
        Self::new_with_dirs(
            settings_path,
            worktrees_root,
            registry_path,
            Arc::new(MemoryStore::new()),
            app_dir.join("projects"),
            app_dir.join("trash"),
            Machine::none(),
        )
    }

    fn new_with_dirs(
        settings_path: PathBuf,
        worktrees_root: PathBuf,
        registry_path: PathBuf,
        secrets: Arc<dyn SecretStore>,
        projects_root: PathBuf,
        trash_dir: PathBuf,
        machine: Machine,
    ) -> Self {
        let app_dir = settings_app_dir(&settings_path);
        // MCP sign-ins (OAuth tokens) live beside the other secrets: the Keychain in the app.
        themis_core::plugins::oauth::set_store(Arc::new(mcp::SecretTokens(Arc::clone(&secrets))));
        let transcript = TranscriptStore::open(&app_dir.join("sessions.sqlite3"))
            .expect("unable to open durable session store");
        let (threads, mut report) = load_and_reconcile(&worktrees_root, &registry_path);
        let skills_path = app_dir.join("skills.json");
        let automations_path = app_dir.join("automations.json");
        let reviews_path = app_dir.join("review_items.json");
        let removals_path = app_dir.join("automation_removals.json");
        let skills = load_store_file(&skills_path, "skill", &mut report);
        let automations = load_automations_file(&automations_path, &mut report);
        let reviews = load_store_file(&reviews_path, "review item", &mut report);
        let removals = load_store_file(&removals_path, "automation removal", &mut report);
        let diagnostics = Arc::new(themis_core::diagnostics::DiagnosticLog::new(
            themis_core::diagnostics::directory_for(&app_dir),
        ));
        let usage = Arc::new(crate::usage::UsageStore::open(
            &app_dir.join("usage.sqlite3"),
            Arc::clone(&diagnostics),
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
                trash_dir,
                personal_home: machine.personal_home,
                built_in_search: machine.built_in_search,
                threads: tokio::sync::RwLock::new(threads),
                pending: PendingMap::default(),
                mcp: std::sync::Mutex::new(HashMap::new()),
                settings: SettingsStore::load_with_projects_root(settings_path, projects_root),
                transcript,
                usage,
                diagnostics,
                secrets,
                go_base_url_override: std::sync::Mutex::new(None),
                sandbox_home: std::sync::Mutex::new(None),
                sandbox_backends_unavailable: AtomicBool::new(false),
                go_catalog: tokio::sync::RwLock::new(Vec::new()),
                provider_catalogs: tokio::sync::RwLock::new(HashMap::new()),
                worktrees_root,
                registry_path,
                chat_grants_dir: app_dir.join(chat_grants::CHAT_GRANTS_DIR),
                plugin_locks: std::sync::Mutex::new(HashMap::new()),
                running_count: AtomicUsize::new(0),
                reconcile_report: std::sync::Mutex::new(report),
                skills: tokio::sync::RwLock::new(skills),
                skills_path,
                automations: tokio::sync::RwLock::new(automations),
                automations_path,
                reviews: tokio::sync::RwLock::new(reviews),
                reviews_path,
                removals: tokio::sync::RwLock::new(removals),
                removals_path,
                url_opener: std::sync::Mutex::new(None),
                hook_wakes: std::sync::Mutex::new(HashMap::new()),
                watch_inflight: std::sync::Mutex::new(HashSet::new()),
                automation_runs: std::sync::Mutex::new(HashMap::new()),
                scheduler_sink: std::sync::Mutex::new(None),
                background: themis_core::tools::background::BackgroundRegistry::open(Some(
                    themis_core::tools::background::storage_dir(&app_dir),
                )),
                browsers: themis_core::browser::BrowserHub::new(
                    themis_core::browser::BrowserConfig {
                        root: app_dir.join("browser"),
                        executable: None,
                        headless: true,
                    },
                ),
                resources: Arc::default(),
                recent_errors: std::sync::Mutex::new(VecDeque::new()),
                queues: std::sync::Mutex::new(HashMap::new()),
                queues_dir: app_dir.join(queue::QUEUES_DIR),
                queue_lock: std::sync::Mutex::new(()),
                steer_inboxes: std::sync::Mutex::new(HashMap::new()),
                live_notes: std::sync::Mutex::new(HashMap::new()),
                memory_job_lock: tokio::sync::Mutex::new(()),
                questions: std::sync::Mutex::new(HashMap::new()),
                questions_dir: app_dir.join(questions::QUESTIONS_DIR),
                subagents: std::sync::Mutex::new(HashMap::new()),
                terminals: Arc::default(),
                question_lock: std::sync::Mutex::new(()),
                auto_questions: std::sync::Mutex::default(),
                auto_once: std::sync::Mutex::default(),
                memory_jobs: std::sync::Mutex::new(memory_auto::MemoryJobs::new(false)),
                goals: std::sync::Mutex::new(HashMap::new()),
                goals_dir: app_dir.join(goal::GOALS_DIR),
                goal_lock: std::sync::Mutex::new(()),
                goal_turn_runs: std::sync::Mutex::new(HashSet::new()),
                hook_sessions: std::sync::Mutex::new(HashMap::new()),
                goal_generations: std::sync::Mutex::new(HashMap::new()),
                server_sends: std::sync::Mutex::new(HashMap::new()),
            }),
        }
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

    /// Logs one desktop notification attempt (sent by the app, which has no log
    /// of its own). Metadata only: the chat id, what it reported and how it ended.
    pub fn record_notification(
        &self,
        record: crate::notifications::NotificationRecord,
    ) -> Result<(), String> {
        use crate::notifications::NotificationOutcome;
        if !chat_grants::safe_chat_id(&record.thread_id) {
            return Err("invalid chat id".to_owned());
        }
        let level = if record.outcome == NotificationOutcome::Sent {
            let (kind, message) = match record.kind {
                crate::notifications::GoalNotice::Reached => ("goal_reached", "Goal reached"),
                crate::notifications::GoalNotice::Blocked => ("goal_blocked", "Goal blocked"),
                crate::notifications::GoalNotice::Waiting => {
                    ("goal_waiting", "Goal waiting for you")
                }
                crate::notifications::GoalNotice::Quota => {
                    ("goal_quota", "Goal paused: quota used up")
                }
            };
            self.notify_session_hooks(&record.thread_id, kind, message);
            "info"
        } else {
            "warn"
        };
        self.inner
            .diagnostics
            .append(
                "notifications",
                "goal_notification",
                level,
                serde_json::json!({
                    "thread_id": record.thread_id,
                    "kind": record.kind,
                    "outcome": record.outcome,
                }),
            )
            .map_err(|error| error.to_string())
    }

    /// Records a command failure in the diagnostics ring (oldest-first,
    /// capped at [`MAX_RECENT_ERRORS`]; the oldest entry is evicted on
    /// overflow). Every fallible Tauri command reports its `Err` here via
    /// the `record_cmd!` helper in [`crate::commands`], attributing its own
    /// command name.
    pub fn record_error(&self, command: &str, message: String) {
        // The message can hold paths or user text, so the log keeps only its category.
        let _ = self.inner.diagnostics.append(
            "server",
            "command_failed",
            "error",
            serde_json::json!({"method": command, "category": failure_category(&message)}),
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

    /// Health check: returns the `themis-core` version.
    pub async fn ping(&self) -> String {
        themis_core::version().to_owned()
    }

    /// Checks the Go connection and returns (and keeps) the current OpenCode Go catalog.
    pub(super) async fn refresh_go_catalog(
        &self,
    ) -> Result<Vec<themis_core::providers::GoModel>, String> {
        let key = self.api_key_for(ProviderKind::Go).await?;
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

    /// Whether the Go catalog says `model` offers `effort`, loading the
    /// catalog first when it is empty. The one effort check for runs,
    /// automations and queued messages.
    pub(super) async fn effort_offered(&self, model: &str, effort: &str) -> bool {
        let _ = self.ensure_go_catalog().await;
        offers_effort(&self.inner.go_catalog.read().await, model, effort)
    }

    async fn ensure_go_catalog(&self) -> Result<(), String> {
        let unloaded = self.inner.go_catalog.read().await.is_empty();
        if unloaded {
            self.refresh_go_catalog().await?;
        }
        Ok(())
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
}

fn thread_info(record: &ThreadRecord, transcript: &TranscriptStore) -> Result<ThreadInfo, String> {
    Ok(ThreadInfo {
        last_activity_seq: transcript.last_activity_seq(&record.id)?,
        last_outcome: transcript.last_outcome(&record.id)?.map(str::to_owned),
        open_questions: 0,
        goal_status: None,
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

/// Whether `model` in `catalog` offers reasoning `effort`.
fn offers_effort(catalog: &[themis_core::providers::GoModel], model: &str, effort: &str) -> bool {
    catalog
        .iter()
        .any(|entry| entry.id == model && entry.effort_levels.iter().any(|level| level == effort))
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

/// Only the OpenCode Go credential is accepted (kept in the Settings › Secrets secret
/// chosen for it, `OPENCODE_KEY` by default).
fn known_provider(provider: &str) -> Result<(), String> {
    match provider.trim().to_lowercase().as_str() {
        "go" => Ok(()),
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

mod external;
mod scheduler;
mod stores;
mod titles;
pub use external::UrlOpener;
use external::*;
use scheduler::*;
use stores::*;
use titles::*;

#[cfg(test)]
mod tests;

/// Headless test state with no settings file yet starts new chats in Custom, where every
/// approval is put to the person, so tests see each prompt; the app's own default is Auto
/// (`default_new_chat_mode`). A test that wants another mode sets it on the chat.
fn seed_test_settings(settings_path: &Path) {
    if settings_path.exists() {
        return;
    }
    if let Some(parent) = settings_path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    let settings = crate::types::Settings {
        default_approval_mode: themis_core::configuration::ApprovalMode::Custom,
        ..Default::default()
    };
    if let Ok(text) = serde_json::to_vec_pretty(&settings) {
        let _ = std::fs::write(settings_path, text);
    }
}
