//! Wire types: an exact serde mirror of `web/src/lib/types.ts`.
//!
//! That file is the contract's source of truth. Every struct here serializes
//! with the same field names (the TS side uses `snake_case` throughout), and
//! [`ThreadEvent`] is internally tagged on `kind` with the same variant names.

use serde::{Deserialize, Serialize};
use themis_core::runtime::ApprovalDecision as CoreApprovalDecision;
use themis_core::runtime::RunEvent;

/// Supported LLM backends (`ProviderKind` in types.ts).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ProviderKind {
    Go,
    #[serde(other)]
    Legacy,
}

impl ProviderKind {
    /// Canonical lowercase name.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Go => "go",
            Self::Legacy => "legacy",
        }
    }
}

/// Tool-action risk classification (`RiskLevel` in types.ts).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum RiskLevel {
    Read,
    Write,
    Execute,
    Network,
    Destructive,
}

impl From<themis_core::tools::RiskLevel> for RiskLevel {
    fn from(value: themis_core::tools::RiskLevel) -> Self {
        match value {
            themis_core::tools::RiskLevel::Read => Self::Read,
            themis_core::tools::RiskLevel::Write => Self::Write,
            themis_core::tools::RiskLevel::Execute => Self::Execute,
            themis_core::tools::RiskLevel::Network => Self::Network,
            themis_core::tools::RiskLevel::Destructive => Self::Destructive,
        }
    }
}

/// Approval outcome (`ApprovalDecision` in types.ts: `once|always|deny`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ApprovalDecision {
    Once,
    Always,
    Deny,
}

impl ApprovalDecision {
    /// Maps to the `themis-core` approval decision.
    #[must_use]
    pub const fn core_approval(self) -> themis_core::tools::Approval {
        match self {
            Self::Once => themis_core::tools::Approval::AllowOnce,
            Self::Always => themis_core::tools::Approval::AllowAlways,
            Self::Deny => themis_core::tools::Approval::Deny,
        }
    }
}

impl From<CoreApprovalDecision> for ApprovalDecision {
    fn from(value: CoreApprovalDecision) -> Self {
        match value {
            CoreApprovalDecision::AllowOnce => Self::Once,
            CoreApprovalDecision::AllowAlways => Self::Always,
            CoreApprovalDecision::Deny => Self::Deny,
        }
    }
}

/// UI theme (`ThemeMode` in types.ts).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ThemeMode {
    #[default]
    Dark,
    Light,
    System,
}

/// An opened project directory (`ProjectInfo` in types.ts).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProjectInfo {
    pub root: String,
    pub name: String,
    pub is_git: bool,
    #[serde(default)]
    pub is_default: bool,
}

/// A conversation thread (`ThreadInfo` in types.ts).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ThreadInfo {
    /// Last persisted conversation event; zero means no activity yet.
    #[serde(default)]
    pub last_activity_seq: i64,
    pub id: String,
    pub title: String,
    pub provider: ProviderKind,
    pub model: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reasoning_effort: Option<String>,
    #[serde(default)]
    pub approval_mode: themis_core::configuration::ApprovalMode,
    pub running: bool,
    /// Worktree backing this thread (`None` for non-git read-only threads).
    pub worktree_path: Option<String>,
    /// Thread branch (`themis/<short-id>`; `None` without a worktree).
    pub branch: Option<String>,
    /// Base revision the branch forked from (`None` without a worktree).
    pub base_branch: Option<String>,
    /// True when the thread was running at shutdown and reconciled on boot.
    pub recovered: bool,
    /// Skill ids attached to this thread (applied to every run).
    #[serde(default)]
    pub skill_ids: Vec<String>,
}

/// Result of `merge_thread` (`MergeResult` in types.ts).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MergeResult {
    /// True when the patch applied cleanly (worktree + branch retired).
    pub applied: bool,
    /// Paths applied to the checkout (empty unless `applied`).
    pub applied_files: Vec<String>,
    /// Conflicting paths; the checkout was left untouched.
    pub conflicts: Vec<String>,
}

/// Handle to a spawned run (`RunHandle` in types.ts).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RunHandle {
    pub run_id: String,
}

/// Streaming run event (`ThreadEvent` in types.ts).
///
/// Internally tagged on `kind`; variant names match the TS union exactly
/// (`started|assistant_text|tool_started|tool_finished|approval_decided|finished|failed`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum ThreadEvent {
    Started {
        task: String,
        max_turns: usize,
    },
    AssistantText {
        text: String,
    },
    ToolStarted {
        tool: String,
        summary: String,
    },
    ToolFinished {
        tool: String,
        ok: bool,
        output: String,
    },
    ApprovalDecided {
        tool: String,
        decision: ApprovalDecision,
    },
    ContextCompacting,
    ContextCheckpoint {
        summary: String,
    },
    Incomplete {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        model: Option<String>,
        result: String,
    },
    Finished {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        model: Option<String>,
        result: String,
    },
    Failed {
        error: String,
    },
}

impl From<RunEvent> for ThreadEvent {
    fn from(event: RunEvent) -> Self {
        match event {
            RunEvent::Started { task, max_turns } => Self::Started { task, max_turns },
            RunEvent::AssistantText(text) => Self::AssistantText { text },
            RunEvent::ToolCallStarted { tool, summary } => Self::ToolStarted { tool, summary },
            RunEvent::ToolCallFinished { tool, ok, output } => {
                Self::ToolFinished { tool, ok, output }
            }
            RunEvent::ApprovalDecided { tool, decision } => Self::ApprovalDecided {
                tool,
                decision: ApprovalDecision::from(decision),
            },
            RunEvent::ContextCompacting => Self::ContextCompacting,
            RunEvent::ContextCheckpoint { summary } => Self::ContextCheckpoint { summary },
            RunEvent::Incomplete { result } => Self::Incomplete {
                result,
                model: None,
            },
            RunEvent::Finished { result } => Self::Finished {
                result,
                model: None,
            },
            RunEvent::Failed { error } => Self::Failed { error },
        }
    }
}

/// `thread-event` payload (`ThreadEventEnvelope` in types.ts).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ThreadEventEnvelope {
    pub thread_id: String,
    pub run_id: String,
    pub event: ThreadEvent,
}

/// `approval-request` payload (`ApprovalRequest` in types.ts).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ApprovalRequest {
    pub thread_id: String,
    pub approval_id: String,
    pub tool: String,
    pub summary: String,
    pub risk: RiskLevel,
}

/// One diff line kind (`DiffLineKind` in types.ts).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum DiffLineKind {
    Context,
    Add,
    Del,
}

/// One diff line (`DiffLine` in types.ts).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DiffLine {
    pub kind: DiffLineKind,
    pub text: String,
}

/// One hunk (`DiffHunk` in types.ts).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DiffHunk {
    pub old_start: usize,
    pub old_lines: usize,
    pub new_start: usize,
    pub new_lines: usize,
    pub lines: Vec<DiffLine>,
}

/// File change status (`DiffStatus` in types.ts).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum DiffStatus {
    Modified,
    Added,
    Deleted,
    Renamed,
}

/// One changed file (`DiffFile` in types.ts).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DiffFile {
    pub path: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub old_path: Option<String>,
    pub status: DiffStatus,
    pub preexisting: bool,
    pub hunks: Vec<DiffHunk>,
}

/// Diff listing (`DiffState` in types.ts).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DiffState {
    pub available: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
    pub files: Vec<DiffFile>,
}

/// A comment pinned to a file (`ThreadComment` in types.ts).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ThreadComment {
    pub id: String,
    pub path: String,
    pub comment: String,
}

/// A helper script bundled with a skill (`SkillScript` in types.ts).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SkillScript {
    pub name: String,
    pub content: String,
}

/// A skill bundle (`Skill` in types.ts).
///
/// The wire shape is `snake_case` like the rest of the bridge; use
/// [`Skill::core_skill`] to convert to the `themis-core` representation
/// (which serializes `camelCase`) before calling core skill APIs.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Skill {
    pub id: String,
    pub name: String,
    pub description: String,
    pub instructions: String,
    /// Tool-name allowlist. Empty means all tools.
    pub allowed_tools: Vec<String>,
    pub scripts: Vec<SkillScript>,
}

impl Skill {
    /// Converts to the `themis-core` skill representation.
    #[must_use]
    pub fn core_skill(&self) -> themis_core::skills::Skill {
        themis_core::skills::Skill {
            id: self.id.clone(),
            name: self.name.clone(),
            description: self.description.clone(),
            instructions: self.instructions.clone(),
            allowed_tools: self.allowed_tools.clone(),
            scripts: self
                .scripts
                .iter()
                .map(|script| themis_core::skills::SkillScript {
                    name: script.name.clone(),
                    content: script.content.clone(),
                })
                .collect(),
        }
    }
}

/// Skill create/update payload (`SkillInput` in types.ts).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SkillInput {
    pub name: String,
    pub description: String,
    pub instructions: String,
    pub allowed_tools: Vec<String>,
    pub scripts: Vec<SkillScript>,
}

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
}

/// Result of `run_automation_now` (`{automation_id, thread_id, run_id}`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RunAutomationNow {
    pub automation_id: String,
    pub thread_id: String,
    pub run_id: String,
}

/// Default `concurrency_limit` (also the serde fallback for settings files
/// written before the field existed; a plain `#[serde(default)]` would yield
/// `0`, which violates the `1..=16` contract).
#[must_use]
pub const fn default_concurrency_limit() -> u32 {
    3
}

/// Default `automations_enabled`: new installs run automations, and settings
/// files written before the field existed load as enabled.
#[must_use]
pub const fn default_automations_enabled() -> bool {
    true
}

/// App settings (`Settings` in types.ts).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Settings {
    #[serde(default = "default_projects_directory")]
    pub projects_directory: String,
    #[serde(default, skip_serializing_if = "std::collections::HashMap::is_empty")]
    pub project_names: std::collections::HashMap<String, String>,
    #[serde(default = "default_text_size")]
    pub text_size: u32,
    #[serde(default = "default_appearance_choice")]
    pub theme_palette: String,
    #[serde(default = "default_appearance_choice")]
    pub font_family: String,
    #[serde(default = "default_automations_enabled")]
    pub sidebar_hover: bool,
    pub theme: ThemeMode,
    pub default_provider: ProviderKind,
    pub default_model: String,
    /// Legacy checkpoint interval, retained for settings/API compatibility; no runtime effect.
    pub max_turns: u32,
    #[serde(default = "default_max_total_turns")]
    pub max_total_turns: u32,
    #[serde(default = "default_context_token_budget")]
    pub context_token_budget: u32,
    #[serde(default = "default_context_messages")]
    pub context_messages: u32,
    #[serde(default = "default_approval_timeout_seconds")]
    pub approval_timeout_seconds: u32,
    #[serde(default)]
    pub confirm_reads: bool,
    /// Recently opened project roots (most recent first).
    #[serde(default)]
    pub recent_roots: Vec<String>,
    /// Max parallel agent runs across all threads (`1..=16`).
    #[serde(default = "default_concurrency_limit")]
    pub concurrency_limit: u32,
    /// Global automation kill-switch (the scheduler no-ops when false).
    #[serde(default = "default_automations_enabled")]
    pub automations_enabled: bool,
    #[serde(default = "default_completion_sound")]
    pub completion_sound: bool,
    #[serde(default)]
    pub completion_haptic: bool,
    /// True once the user completed onboarding (defaults to false so settings
    /// files written before the field existed load as not onboarded).
    #[serde(default)]
    pub onboarded: bool,
}

pub fn default_projects_directory() -> String {
    std::env::var_os("HOME")
        .or_else(|| std::env::var_os("USERPROFILE"))
        .map(|home| {
            std::path::PathBuf::from(home)
                .join("ThemisOS/Projects")
                .to_string_lossy()
                .into_owned()
        })
        .unwrap_or_default()
}

fn default_appearance_choice() -> String {
    "system".to_owned()
}

const fn default_text_size() -> u32 {
    13
}

pub const fn default_context_messages() -> u32 {
    20
}

const fn default_completion_sound() -> bool {
    true
}

pub const fn default_max_total_turns() -> u32 {
    200
}
pub const fn default_context_token_budget() -> u32 {
    200_000
}

pub const fn default_approval_timeout_seconds() -> u32 {
    300
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            projects_directory: default_projects_directory(),
            project_names: std::collections::HashMap::new(),
            text_size: default_text_size(),
            theme_palette: default_appearance_choice(),
            font_family: default_appearance_choice(),
            sidebar_hover: true,
            theme: ThemeMode::Dark,
            default_provider: ProviderKind::Go,
            default_model: themis_core::providers::GO_DEFAULT_MODEL.to_owned(),
            max_turns: 20,
            max_total_turns: default_max_total_turns(),
            context_token_budget: default_context_token_budget(),
            context_messages: default_context_messages(),
            approval_timeout_seconds: default_approval_timeout_seconds(),
            confirm_reads: false,
            recent_roots: Vec::new(),
            concurrency_limit: default_concurrency_limit(),
            automations_enabled: default_automations_enabled(),
            completion_sound: default_completion_sound(),
            completion_haptic: false,
            onboarded: false,
        }
    }
}

/// Partial settings update (`Partial<Settings>` in types.ts).
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct SettingsPatch {
    pub projects_directory: Option<String>,
    pub text_size: Option<u32>,
    pub theme_palette: Option<String>,
    pub font_family: Option<String>,
    pub sidebar_hover: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub theme: Option<ThemeMode>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub default_provider: Option<ProviderKind>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub default_model: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    /// Legacy checkpoint interval; accepted for compatibility, no runtime effect.
    pub max_turns: Option<i64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_total_turns: Option<i64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub context_token_budget: Option<i64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub context_messages: Option<i64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub approval_timeout_seconds: Option<i64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub confirm_reads: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub recent_roots: Option<Vec<String>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub concurrency_limit: Option<i64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub automations_enabled: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub completion_sound: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub completion_haptic: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub onboarded: Option<bool>,
}

/// Which providers have a key stored (`SecretStatus` in types.ts).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SecretStatus {
    pub go: bool,
}

/// One recorded command failure (`DiagnosticsError` in types.ts).
///
/// `at` is an RFC3339 timestamp; `command` is the Tauri command name
/// (`snake_case`, as invoked); `message` is the error text.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DiagnosticsError {
    pub at: String,
    pub command: String,
    pub message: String,
}

/// App diagnostics snapshot (`Diagnostics` in types.ts).
///
/// `settings` is a plain settings clone, which by construction never holds
/// secret values. `recent_errors` is oldest-first (newest last).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Diagnostics {
    pub app_version: String,
    pub os: String,
    pub settings: Settings,
    pub recent_errors: Vec<DiagnosticsError>,
    #[serde(default)]
    pub persistence: Vec<crate::doctor::Check>,
    #[serde(default)]
    pub recovery_notes: usize,
}

/// Updater outcome (`UpdateState` in types.ts).
///
/// Kebab-case on the wire: `disabled|up-to-date|available|error`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum UpdateState {
    Disabled,
    UpToDate,
    Available,
    Error,
}

/// Result of `check_for_updates` (`UpdateStatus` in types.ts).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct UpdateStatus {
    pub state: UpdateState,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub version: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub notes: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub message: Option<String>,
}

impl UpdateStatus {
    /// Updater not configured (no endpoint+pubkey in `plugins.updater`).
    #[must_use]
    pub fn disabled(message: impl Into<String>) -> Self {
        Self {
            state: UpdateState::Disabled,
            version: None,
            notes: None,
            message: Some(message.into()),
        }
    }
}

/// Tauri event name carrying [`ThreadEventEnvelope`] payloads.
pub const THREAD_EVENT_NAME: &str = "thread-event";
/// Tauri event name carrying [`ApprovalRequest`] payloads.
pub const APPROVAL_REQUEST_NAME: &str = "approval-request";
/// Tauri event name carrying [`ReviewItem`] payloads.
pub const REVIEW_ITEM_NAME: &str = "review-item-added";

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::{json, Value};

    fn to_value<T: Serialize>(value: &T) -> Value {
        serde_json::to_value(value).expect("serializable")
    }

    #[test]
    fn thread_events_serialize_exactly_like_ts() {
        let cases: Vec<(ThreadEvent, Value)> = vec![
            (
                ThreadEvent::Started {
                    task: "do it".to_owned(),
                    max_turns: 20,
                },
                json!({"kind": "started", "task": "do it", "max_turns": 20}),
            ),
            (
                ThreadEvent::AssistantText {
                    text: "hi".to_owned(),
                },
                json!({"kind": "assistant_text", "text": "hi"}),
            ),
            (
                ThreadEvent::ToolStarted {
                    tool: "shell".to_owned(),
                    summary: "run".to_owned(),
                },
                json!({"kind": "tool_started", "tool": "shell", "summary": "run"}),
            ),
            (
                ThreadEvent::ToolFinished {
                    tool: "shell".to_owned(),
                    ok: true,
                    output: "result".to_owned(),
                },
                json!({"kind": "tool_finished", "tool": "shell", "ok": true, "output": "result"}),
            ),
            (
                ThreadEvent::ApprovalDecided {
                    tool: "shell".to_owned(),
                    decision: ApprovalDecision::Once,
                },
                json!({"kind": "approval_decided", "tool": "shell", "decision": "once"}),
            ),
            (
                ThreadEvent::ContextCompacting,
                json!({"kind": "context_compacting"}),
            ),
            (
                ThreadEvent::Finished {
                    model: None,
                    result: "done".to_owned(),
                },
                json!({"kind": "finished", "result": "done"}),
            ),
            (
                ThreadEvent::Failed {
                    error: "boom".to_owned(),
                },
                json!({"kind": "failed", "error": "boom"}),
            ),
        ];
        for (event, expected) in cases {
            assert_eq!(to_value(&event), expected, "{event:?}");
        }
        // The remaining decision spellings.
        assert_eq!(to_value(&ApprovalDecision::Always), json!("always"));
        assert_eq!(to_value(&ApprovalDecision::Deny), json!("deny"));
        assert_eq!(
            serde_json::from_value::<ApprovalDecision>(json!("once")).unwrap(),
            ApprovalDecision::Once
        );
    }

    #[test]
    fn run_event_mapping_covers_every_variant() {
        assert_eq!(
            ThreadEvent::from(RunEvent::Started {
                task: "t".to_owned(),
                max_turns: 3,
            }),
            ThreadEvent::Started {
                task: "t".to_owned(),
                max_turns: 3,
            }
        );
        assert_eq!(
            ThreadEvent::from(RunEvent::AssistantText("a".to_owned())),
            ThreadEvent::AssistantText {
                text: "a".to_owned()
            }
        );
        assert_eq!(
            ThreadEvent::from(RunEvent::ToolCallStarted {
                tool: "t".to_owned(),
                summary: "s".to_owned(),
            }),
            ThreadEvent::ToolStarted {
                tool: "t".to_owned(),
                summary: "s".to_owned(),
            }
        );
        assert_eq!(
            ThreadEvent::from(RunEvent::ToolCallFinished {
                tool: "t".to_owned(),
                ok: false,
                output: "failed".to_owned(),
            }),
            ThreadEvent::ToolFinished {
                tool: "t".to_owned(),
                ok: false,
                output: "failed".to_owned(),
            }
        );
        for (core, wire) in [
            (CoreApprovalDecision::AllowOnce, ApprovalDecision::Once),
            (CoreApprovalDecision::AllowAlways, ApprovalDecision::Always),
            (CoreApprovalDecision::Deny, ApprovalDecision::Deny),
        ] {
            assert_eq!(
                ThreadEvent::from(RunEvent::ApprovalDecided {
                    tool: "t".to_owned(),
                    decision: core,
                }),
                ThreadEvent::ApprovalDecided {
                    tool: "t".to_owned(),
                    decision: wire,
                }
            );
        }
        assert_eq!(
            ThreadEvent::from(RunEvent::ContextCompacting),
            ThreadEvent::ContextCompacting
        );
        assert_eq!(
            ThreadEvent::from(RunEvent::Finished {
                result: "r".to_owned()
            }),
            ThreadEvent::Finished {
                model: None,
                result: "r".to_owned()
            }
        );
        assert_eq!(
            ThreadEvent::from(RunEvent::Failed {
                error: "e".to_owned()
            }),
            ThreadEvent::Failed {
                error: "e".to_owned()
            }
        );
    }

    #[test]
    fn envelopes_use_snake_case_keys() {
        let envelope = ThreadEventEnvelope {
            thread_id: "t".to_owned(),
            run_id: "r".to_owned(),
            event: ThreadEvent::Finished {
                model: None,
                result: "ok".to_owned(),
            },
        };
        assert_eq!(
            to_value(&envelope),
            json!({
                "thread_id": "t",
                "run_id": "r",
                "event": {"kind": "finished", "result": "ok"},
            })
        );
        let request = ApprovalRequest {
            thread_id: "t".to_owned(),
            approval_id: "a".to_owned(),
            tool: "shell".to_owned(),
            summary: "run".to_owned(),
            risk: RiskLevel::Execute,
        };
        assert_eq!(
            to_value(&request),
            json!({
                "thread_id": "t",
                "approval_id": "a",
                "tool": "shell",
                "summary": "run",
                "risk": "execute",
            })
        );
    }

    #[test]
    fn diff_shapes_match_ts_and_omit_absent_optionals() {
        let hunk = DiffHunk {
            old_start: 1,
            old_lines: 2,
            new_start: 1,
            new_lines: 3,
            lines: vec![
                DiffLine {
                    kind: DiffLineKind::Context,
                    text: "same".to_owned(),
                },
                DiffLine {
                    kind: DiffLineKind::Del,
                    text: "old".to_owned(),
                },
                DiffLine {
                    kind: DiffLineKind::Add,
                    text: "new".to_owned(),
                },
            ],
        };
        assert_eq!(
            to_value(&hunk),
            json!({
                "old_start": 1,
                "old_lines": 2,
                "new_start": 1,
                "new_lines": 3,
                "lines": [
                    {"kind": "context", "text": "same"},
                    {"kind": "del", "text": "old"},
                    {"kind": "add", "text": "new"},
                ],
            })
        );
        let file = DiffFile {
            path: "a.rs".to_owned(),
            old_path: None,
            status: DiffStatus::Modified,
            preexisting: false,
            hunks: vec![hunk],
        };
        let json = to_value(&file);
        assert_eq!(json["path"], "a.rs");
        assert_eq!(json["status"], "modified");
        assert!(json.get("old_path").is_none());
        let renamed = DiffFile {
            old_path: Some("old.rs".to_owned()),
            status: DiffStatus::Renamed,
            ..file.clone()
        };
        assert_eq!(to_value(&renamed)["old_path"], "old.rs");
        assert_eq!(to_value(&DiffStatus::Added), json!("added"));
        assert_eq!(to_value(&DiffStatus::Deleted), json!("deleted"));
        let unavailable = DiffState {
            available: false,
            reason: Some("not a git repo".to_owned()),
            files: vec![],
        };
        assert_eq!(
            to_value(&unavailable),
            json!({"available": false, "reason": "not a git repo", "files": []})
        );
        let no_reason = DiffState {
            available: true,
            reason: None,
            files: vec![],
        };
        assert!(to_value(&no_reason).get("reason").is_none());
    }

    #[test]
    fn provider_serialization_roundtrips() {
        assert_eq!(to_value(&ProviderKind::Go), json!("go"));
        assert_eq!(
            serde_json::from_value::<ProviderKind>(json!("go")).unwrap(),
            ProviderKind::Go
        );
        assert_eq!(
            serde_json::from_value::<ProviderKind>(json!("retired-provider")).unwrap(),
            ProviderKind::Legacy
        );
    }

    #[test]
    fn thread_info_and_merge_result_match_ts() {
        let info = ThreadInfo {
            last_activity_seq: 42,
            id: "t".to_owned(),
            title: "hi".to_owned(),
            provider: ProviderKind::Go,
            model: "m".to_owned(),
            reasoning_effort: None,
            approval_mode: Default::default(),
            running: false,
            worktree_path: Some("/tmp/wt".to_owned()),
            branch: Some("themis/abc".to_owned()),
            base_branch: Some("main".to_owned()),
            recovered: true,
            skill_ids: vec!["skill-1".to_owned()],
        };
        assert_eq!(
            to_value(&info),
            json!({
                "last_activity_seq": 42,
                "id": "t",
                "title": "hi",
                "provider": "go",
                "model": "m",
                "approval_mode": "custom",
                "running": false,
                "worktree_path": "/tmp/wt",
                "branch": "themis/abc",
                "base_branch": "main",
                "recovered": true,
                "skill_ids": ["skill-1"],
            })
        );
        // Absent worktrees serialize as `null`, exactly like the TS `string | null`.
        let plain = ThreadInfo {
            worktree_path: None,
            branch: None,
            base_branch: None,
            recovered: false,
            ..info
        };
        let json = to_value(&plain);
        assert_eq!(json["worktree_path"], Value::Null);
        assert_eq!(json["branch"], Value::Null);
        assert_eq!(json["base_branch"], Value::Null);
        assert_eq!(json["recovered"], json!(false));

        let merged = MergeResult {
            applied: true,
            applied_files: vec!["a.txt".to_owned()],
            conflicts: Vec::new(),
        };
        assert_eq!(
            to_value(&merged),
            json!({"applied": true, "applied_files": ["a.txt"], "conflicts": []})
        );
        let conflicted = MergeResult {
            applied: false,
            applied_files: Vec::new(),
            conflicts: vec!["b.txt".to_owned()],
        };
        assert_eq!(
            to_value(&conflicted),
            json!({"applied": false, "applied_files": [], "conflicts": ["b.txt"]})
        );
    }

    #[test]
    fn settings_defaults_and_shapes_match_ts() {
        let settings = Settings::default();
        assert_eq!(settings.theme, ThemeMode::Dark);
        assert_eq!(settings.default_provider, ProviderKind::Go);
        assert_eq!(settings.default_model, "muse-spark-1.3-contributor");
        assert_eq!(settings.max_turns, 20);
        assert!(settings.recent_roots.is_empty());
        assert_eq!(settings.concurrency_limit, 3);
        assert!(settings.automations_enabled);
        assert!(settings.completion_sound);
        assert!(!settings.completion_haptic);
        assert!(!settings.onboarded);
        assert_eq!(
            to_value(&settings),
            json!({
                "projects_directory": default_projects_directory(),
                "text_size": 13,
                "theme_palette": "system",
                "font_family": "system",
                "sidebar_hover": true,
                "theme": "dark",
                "default_provider": "go",
                "default_model": "muse-spark-1.3-contributor",
                "max_turns": 20,
                "max_total_turns": 200,
                "context_token_budget": 200000,
                "context_messages": 20,
                "approval_timeout_seconds": 300,
                "confirm_reads": false,
                "recent_roots": [],
                "concurrency_limit": 3,
                "automations_enabled": true,
                "completion_sound": true,
                "completion_haptic": false,
                "onboarded": false,
            })
        );
        // A TS-style partial patch deserializes with the rest defaulted.
        let patch: SettingsPatch =
            serde_json::from_value(json!({"theme": "dark", "max_turns": 42})).unwrap();
        assert_eq!(patch.theme, Some(ThemeMode::Dark));
        assert_eq!(patch.max_turns, Some(42));
        assert!(patch.default_provider.is_none());
        assert!(patch.default_model.is_none());
        assert!(patch.recent_roots.is_none());
        assert!(patch.concurrency_limit.is_none());
        assert!(patch.automations_enabled.is_none());
        assert!(patch.onboarded.is_none());
        let patch: SettingsPatch =
            serde_json::from_value(json!({"automations_enabled": false})).unwrap();
        assert_eq!(patch.automations_enabled, Some(false));
        let patch: SettingsPatch = serde_json::from_value(json!({"onboarded": true})).unwrap();
        assert_eq!(patch.onboarded, Some(true));
        // Settings files written before the new fields existed still load,
        // with automations enabled and onboarding not done.
        let legacy: Settings = serde_json::from_value(json!({
            "theme": "dark",
            "default_provider": "openai",
            "default_model": "gpt-x",
            "max_turns": 7,
        }))
        .unwrap();
        assert_eq!(legacy.theme, ThemeMode::Dark);
        assert!(legacy.recent_roots.is_empty());
        assert_eq!(legacy.concurrency_limit, 3);
        assert!(legacy.automations_enabled);
        assert!(legacy.completion_sound);
        assert!(!legacy.completion_haptic);
        assert!(!legacy.onboarded);
        let status = SecretStatus { go: true };
        assert_eq!(to_value(&status), json!({"go": true}));
    }

    #[test]
    fn skill_shapes_match_ts_and_convert_to_core() {
        let skill = Skill {
            id: "skill-1".to_owned(),
            name: "Rust".to_owned(),
            description: "clippy-clean code".to_owned(),
            instructions: "Run clippy.".to_owned(),
            allowed_tools: vec!["read_file".to_owned()],
            scripts: vec![SkillScript {
                name: "lint.sh".to_owned(),
                content: "echo hi".to_owned(),
            }],
        };
        assert_eq!(
            to_value(&skill),
            json!({
                "id": "skill-1",
                "name": "Rust",
                "description": "clippy-clean code",
                "instructions": "Run clippy.",
                "allowed_tools": ["read_file"],
                "scripts": [{"name": "lint.sh", "content": "echo hi"}],
            })
        );
        let input = SkillInput {
            name: "Rust".to_owned(),
            description: "clippy-clean code".to_owned(),
            instructions: "Run clippy.".to_owned(),
            allowed_tools: vec![],
            scripts: vec![],
        };
        let parsed: SkillInput = serde_json::from_value(to_value(&input)).unwrap();
        assert_eq!(parsed, input);
        // The core conversion preserves every field.
        let core = skill.core_skill();
        assert_eq!(core.id, "skill-1");
        assert_eq!(core.name, "Rust");
        assert_eq!(core.description, "clippy-clean code");
        assert_eq!(core.instructions, "Run clippy.");
        assert_eq!(core.allowed_tools, vec!["read_file".to_owned()]);
        assert_eq!(core.scripts.len(), 1);
        assert_eq!(core.scripts[0].name, "lint.sh");
        assert_eq!(core.scripts[0].content, "echo hi");
    }

    #[test]
    fn automation_and_review_shapes_match_ts() {
        let automation = Automation {
            id: "a".to_owned(),
            name: "Nightly".to_owned(),
            project_root: "/tmp/repo".to_owned(),
            provider: ProviderKind::Go,
            model: "m".to_owned(),
            reasoning_effort: None,
            target_thread_id: None,
            skill_ids: vec!["s".to_owned()],
            interval_mins: 60,
            schedule: None,
            task: "check health".to_owned(),
            enabled: true,
            last_run_at: None,
            next_run_at: "2026-01-01T00:00:00Z".to_owned(),
            run_count: 0,
        };
        assert_eq!(
            to_value(&automation),
            json!({
                "id": "a",
                "name": "Nightly",
                "project_root": "/tmp/repo",
                "provider": "go",
                "model": "m",
                "skill_ids": ["s"],
                "interval_mins": 60,
                "task": "check health",
                "enabled": true,
                "last_run_at": null,
                "next_run_at": "2026-01-01T00:00:00Z",
                "run_count": 0,
            })
        );
        let with_effort = Automation {
            reasoning_effort: Some("medium".to_owned()),
            ..automation
        };
        assert_eq!(to_value(&with_effort)["reasoning_effort"], "medium");
        let item = ReviewItem {
            id: "r".to_owned(),
            automation_id: "a".to_owned(),
            thread_id: "t".to_owned(),
            created_at: "2026-01-01T01:00:00Z".to_owned(),
            title: "Automation Nightly".to_owned(),
            summary: "done".to_owned(),
            status: ReviewStatus::Pending,
        };
        assert_eq!(
            to_value(&item),
            json!({
                "id": "r",
                "automation_id": "a",
                "thread_id": "t",
                "created_at": "2026-01-01T01:00:00Z",
                "title": "Automation Nightly",
                "summary": "done",
                "status": "pending",
            })
        );
        assert_eq!(to_value(&ReviewStatus::Continued), json!("continued"));
        assert_eq!(to_value(&ReviewStatus::Dismissed), json!("dismissed"));
        assert_eq!(
            serde_json::from_value::<ReviewStatus>(json!("pending")).unwrap(),
            ReviewStatus::Pending
        );
        let now = RunAutomationNow {
            automation_id: "a".to_owned(),
            thread_id: "t".to_owned(),
            run_id: "r".to_owned(),
        };
        assert_eq!(
            to_value(&now),
            json!({"automation_id": "a", "thread_id": "t", "run_id": "r"})
        );
        assert_eq!(REVIEW_ITEM_NAME, "review-item-added");
    }

    #[test]
    fn diagnostics_and_update_status_match_ts() {
        // `UpdateState` is kebab-case on the wire (`up-to-date`).
        for (state, wire) in [
            (UpdateState::Disabled, "disabled"),
            (UpdateState::UpToDate, "up-to-date"),
            (UpdateState::Available, "available"),
            (UpdateState::Error, "error"),
        ] {
            assert_eq!(to_value(&state), json!(wire));
            assert_eq!(
                serde_json::from_value::<UpdateState>(json!(wire)).unwrap(),
                state
            );
        }
        // Absent optionals are omitted, exactly like the TS optional fields.
        assert_eq!(
            to_value(&UpdateStatus::disabled("updates not configured")),
            json!({"state": "disabled", "message": "updates not configured"})
        );
        assert_eq!(
            to_value(&UpdateStatus {
                state: UpdateState::UpToDate,
                version: None,
                notes: None,
                message: None,
            }),
            json!({"state": "up-to-date"})
        );
        assert_eq!(
            to_value(&UpdateStatus {
                state: UpdateState::Available,
                version: Some("0.2.0".to_owned()),
                notes: Some("fixes".to_owned()),
                message: None,
            }),
            json!({"state": "available", "version": "0.2.0", "notes": "fixes"})
        );
        let diagnostics = Diagnostics {
            persistence: Vec::new(),
            recovery_notes: 0,
            app_version: "0.1.0".to_owned(),
            os: "macos".to_owned(),
            settings: Settings::default(),
            recent_errors: vec![DiagnosticsError {
                at: "2026-01-01T00:00:00Z".to_owned(),
                command: "open_project".to_owned(),
                message: "no such directory".to_owned(),
            }],
        };
        let json = to_value(&diagnostics);
        assert_eq!(json["app_version"], "0.1.0");
        assert_eq!(json["os"], "macos");
        assert_eq!(json["settings"]["onboarded"], false);
        assert_eq!(
            json["recent_errors"],
            json!([{
                "at": "2026-01-01T00:00:00Z",
                "command": "open_project",
                "message": "no such directory",
            }])
        );
        let roundtrip: Diagnostics = serde_json::from_value(json).unwrap();
        assert_eq!(roundtrip, diagnostics);
    }
}
