//! Automations, watches, review items and the per-chat task feed.

use super::ProviderKind;
use serde::{Deserialize, Serialize};

/// Calendar recurrence evaluated in an IANA time zone, including DST.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum AutomationRepeat {
    Daily,
    Weekdays,
    Weekly,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AutomationSchedule {
    pub repeat: AutomationRepeat,
    /// Local wall-clock time, strictly HH:MM.
    pub time: String,
    pub timezone: String,
    /// Monday = 0 through Sunday = 6; used only for weekly schedules.
    #[serde(default)]
    pub weekday: u8,
}

/// Who created an automation (`AutomationOrigin` in types.ts).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum AutomationOrigin {
    /// Created from the Automations screen or CLI (also the default for old records).
    #[default]
    User,
    /// Created by the agent through `manage_automations`.
    Agent,
}

impl AutomationOrigin {
    pub(crate) fn is_user(&self) -> bool {
        *self == Self::User
    }
}

/// Limits and change detection for short-interval watches. All optional, so
/// ordinary automations serialize exactly as before.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct WatchSettings {
    /// RFC3339 instant after which the automation is removed ("expired").
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub expires_at: Option<String>,
    /// Stop after this many executions, including checks that start no model run.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_runs: Option<u64>,
    /// Short plain-language goal shown in Tasks ("until status.txt says DONE").
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub stop_when: Option<String>,
    /// Argv run before each tick without a model call; the model only runs when
    /// its output changed.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub check_command: Option<Vec<String>>,
}

/// Scheduler bookkeeping for watches; never supplied by clients.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct WatchState {
    /// Failed runs or checks in a row; three stop the automation.
    #[serde(default, skip_serializing_if = "is_zero")]
    pub consecutive_failures: u32,
    /// Hash of the last `check_command` output that was handed to the model.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_check_hash: Option<String>,
    /// The exact `check_command` argv a user approved for this automation
    /// (interactively in the creating run, or by editing it themselves). It
    /// never covers a different argv and is not a run-scoped grant.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub approved_check: Option<Vec<String>>,
    /// Why an automation was paused (failure backstop, or a check command the
    /// approval policy no longer allows).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub stopped_reason: Option<String>,
}

pub(super) fn is_zero(value: &u32) -> bool {
    *value == 0
}

/// Name and origin copied onto a review item so a finished run keeps a stable
/// title after its automation is deleted.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ReviewSource {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub automation_name: Option<String>,
    #[serde(default, skip_serializing_if = "AutomationOrigin::is_user")]
    pub automation_origin: AutomationOrigin,
}

/// An automation that no longer exists, kept so Tasks can say why.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AutomationRemoval {
    pub id: String,
    pub automation_id: String,
    pub name: String,
    #[serde(default, skip_serializing_if = "AutomationOrigin::is_user")]
    pub origin: AutomationOrigin,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub thread_id: Option<String>,
    pub removed_at: String,
    /// "completed by agent", "expired", "run limit", "failed 3 times", ...
    pub reason: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub stop_when: Option<String>,
    pub run_count: u64,
}

/// A scheduled automation (`Automation` in types.ts).
///
/// Timestamps are RFC3339 strings. `schedule` uses local calendar time;
/// when absent, `interval_mins` retains the legacy fixed-minute recurrence.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Automation {
    pub id: String,
    pub name: String,
    pub project_root: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub target_thread_id: Option<String>,
    pub provider: ProviderKind,
    pub model: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reasoning_effort: Option<String>,
    pub skill_ids: Vec<String>,
    pub interval_mins: u32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub schedule: Option<AutomationSchedule>,
    pub task: String,
    pub enabled: bool,
    pub last_run_at: Option<String>,
    pub next_run_at: String,
    pub run_count: u64,
    /// Omitted from JSON when `user`, so existing records and clients are unchanged.
    #[serde(default, skip_serializing_if = "AutomationOrigin::is_user")]
    pub origin: AutomationOrigin,
    #[serde(flatten)]
    pub watch: WatchSettings,
    #[serde(flatten)]
    pub watch_state: WatchState,
}

/// Automation create/update payload (`AutomationInput` in types.ts).
///
/// `interval_mins` is `i64` (rather than `u32`) so values below 1 fail with
/// the bridge's own validation error instead of a serde type error.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AutomationInput {
    pub name: String,
    pub project_root: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub target_thread_id: Option<String>,
    pub provider: ProviderKind,
    pub model: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reasoning_effort: Option<String>,
    pub skill_ids: Vec<String>,
    pub interval_mins: i64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub schedule: Option<AutomationSchedule>,
    pub task: String,
    pub enabled: bool,
    #[serde(flatten)]
    pub watch: WatchSettings,
}

/// Review-item status (`ReviewStatus` in types.ts).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ReviewStatus {
    Pending,
    Continued,
    Dismissed,
}

/// One completed automation run awaiting review (`ReviewItem` in types.ts).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ReviewItem {
    pub id: String,
    pub automation_id: String,
    pub thread_id: String,
    pub created_at: String,
    pub title: String,
    pub summary: String,
    pub status: ReviewStatus,
    #[serde(flatten)]
    pub source: ReviewSource,
}

/// Live run state of one chat inside [`ThreadTasks`].
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ThreadRunTask {
    pub running: bool,
    pub awaiting_approval: bool,
}

/// An automation that continues a chat, as shown in the Tasks view.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ThreadTaskAutomation {
    pub id: String,
    pub name: String,
    pub enabled: bool,
    pub origin: AutomationOrigin,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub schedule: Option<AutomationSchedule>,
    pub interval_mins: u32,
    pub next_run_at: String,
    pub last_run_at: Option<String>,
    pub run_count: u64,
    /// True while one of its runs is in flight in this chat.
    pub running: bool,
    #[serde(flatten)]
    pub watch: WatchSettings,
    /// Set when a backstop paused a user-created automation.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub stopped_reason: Option<String>,
}

/// One finished automation run in this chat (`seen` once reviewed or dismissed).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ThreadTaskRun {
    pub review_id: String,
    pub automation_id: String,
    pub name: String,
    pub finished_at: String,
    pub ok: bool,
    pub summary: String,
    pub seen: bool,
    /// Kept after the automation is deleted.
    pub origin: AutomationOrigin,
}

/// Per-chat task feed (`ThreadTasks` in types.ts).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ThreadTasks {
    pub thread_id: String,
    pub run: ThreadRunTask,
    pub automations: Vec<ThreadTaskAutomation>,
    /// Newest first, capped at [`THREAD_TASKS_RECENT_LIMIT`].
    pub recent: Vec<ThreadTaskRun>,
    /// Programs the agent started in the background, oldest first.
    #[serde(default)]
    pub background_tasks: Vec<ThreadTaskBackground>,
    /// Automations of this chat that ended, newest first, with the reason.
    #[serde(default)]
    pub removed: Vec<AutomationRemoval>,
}

/// One background shell task in the chat's task feed (`ThreadTaskBackground` in types.ts).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ThreadTaskBackground {
    pub id: String,
    /// The run that started it, when known.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub run_id: Option<String>,
    /// One-line, length-bounded command.
    pub command: String,
    pub cwd: String,
    pub state: themis_core::tools::background::TaskState,
    pub exit_code: Option<i32>,
    pub pid: u32,
    /// RFC 3339 UTC.
    pub started_at: String,
    pub ended_at: Option<String>,
    pub timeout_secs: u64,
    /// Most recent output lines.
    pub last_lines: Vec<String>,
    /// `last_lines` is the whole output, so "Full log" would repeat it.
    #[serde(default)]
    pub last_lines_complete: bool,
    pub log_bytes: u64,
}

/// One page of a background task's rolling log (`BackgroundTaskLog` in types.ts).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct BackgroundTaskLog {
    pub id: String,
    pub state: themis_core::tools::background::TaskState,
    pub text: String,
    pub offset: u64,
    pub next_offset: u64,
    pub total_bytes: u64,
    /// Output before this byte offset rotated out of the bounded log.
    pub oldest_retained_offset: u64,
}

/// Maximum finished runs returned in [`ThreadTasks::recent`].
pub const THREAD_TASKS_RECENT_LIMIT: usize = 10;

/// Result of `run_automation_now` (`{automation_id, thread_id, run_id}`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RunAutomationNow {
    pub automation_id: String,
    pub thread_id: String,
    pub run_id: String,
}
