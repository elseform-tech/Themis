//! Projects, threads and run handles.

use super::automations::is_zero;
use super::{GoalStatus, ProviderKind};
use serde::{Deserialize, Serialize};

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
    /// How the chat's last run ended (`done`, `incomplete`, `failed`, `stopped`), so the sidebar
    /// can show it without loading the chat's history.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_outcome: Option<String>,
    /// Unanswered question boxes in the chat, for the sidebar's "Waiting for you".
    #[serde(default, skip_serializing_if = "is_zero")]
    pub open_questions: u32,
    /// Where the chat's goal stands, when it has one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub goal_status: Option<GoalStatus>,
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
    /// The Vision running this chat (a work chat it started); null for the person's own.
    #[serde(default)]
    pub vision: Option<ThreadVision>,
}

/// The Vision running a chat, as plain chat marks it (`ThreadVision` in types.ts).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ThreadVision {
    pub id: String,
    pub name: String,
    pub shape: String,
    pub color: String,
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

/// One memory a memory job saved, updated or removed.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MemoryChange {
    /// `saved`, `updated` or `removed`.
    pub action: String,
    /// `project` or `global`.
    pub scope: String,
    /// The memory's name (its file, `memory/<name>.md`).
    pub name: String,
    /// Its one-line description (empty for a removed one the job never read).
    pub description: String,
    /// The memory model that made the change, and its place in `memory.models` (from 0).
    pub model: String,
    pub model_index: usize,
    /// The project a project memory belongs to.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub project_root: Option<String>,
}
