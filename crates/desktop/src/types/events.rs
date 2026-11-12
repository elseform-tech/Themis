//! Streaming thread events, their envelopes and the Tauri event names.

use serde::{Deserialize, Serialize};
use themis_core::runtime::RunEvent;

use super::{ApprovalDecision, AttachedMedia, ChangedFile, Goal, GoalNoteKind};
use super::{MemoryChange, Question, QueuedMessage, ThreadInfo, ToolMedia};

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
        /// A readable name for tools whose wire name is opaque (MCP: "server · tool").
        #[serde(default, skip_serializing_if = "Option::is_none")]
        label: Option<String>,
    },
    ToolFinished {
        tool: String,
        ok: bool,
        output: String,
        /// The screenshot or recording a browser tool saved, so the chat shows it
        /// instead of the raw result.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        media: Option<ToolMedia>,
    },
    ApprovalDecided {
        tool: String,
        decision: ApprovalDecision,
    },
    ContextCompacting,
    ContextUsage {
        metrics: serde_json::Value,
    },
    ContextBudget {
        budget: themis_core::runtime::ContextBudget,
    },
    ContextSuspended {
        progress: String,
    },
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
    /// Readable project files whose content changed during this run, sent
    /// just before the run's terminal event.
    FilesChanged {
        files: Vec<ChangedFile>,
    },
    /// Images, videos or PDFs the agent attached to this run's reply
    /// (`attach_file`), in order.
    MediaAttached {
        files: Vec<AttachedMedia>,
    },
    /// The memory models changed memory after this run's turn: one slim line under the reply,
    /// whose names open Settings › Memory.
    MemoryChanged {
        changes: Vec<MemoryChange>,
    },
    /// Where the memory pass after this run's turn stands, for a shimmering line under the
    /// reply while the memory models work. Live only (never saved): `checking` while a model
    /// reads the turn, `done` when it changed nothing, `waiting` when every model failed and the
    /// job waits to retry. A pass that changed memory ends with [`Self::MemoryChanged`] instead.
    MemoryWorking {
        step: String,
    },
    /// Who controls the chat's built-in browser and whether it is recording
    /// changed. Live only; `run_id` is empty.
    BrowserChanged {
        /// `agent`, `user` or `idle`.
        controller: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        recording: Option<themis_core::browser::session::RecordingInfo>,
    },
    Failed {
        error: String,
    },
    /// A background task of this chat started, finished or was stopped.
    /// Sent outside any run (`run_id` is empty); clients refresh the task feed.
    BackgroundTasksChanged,
    /// A message the user steered into this run reached the model.
    Steered {
        text: String,
    },
    /// The user steered this message in: it shows in the chat now, ahead of
    /// [`Self::Steered`] or [`Self::SteerReturned`]. Display only: the model's
    /// history follows `Steered`.
    SteerShown {
        text: String,
    },
    /// The run ended without reading a steered message: it is back in the queue,
    /// to go out as the next message, so it leaves the chat until then.
    SteerReturned {
        text: String,
    },
    /// The user message that starts this run, from any client, the queue or a
    /// goal turn. Live only: the history already holds the message.
    UserMessage {
        text: String,
        #[serde(default)]
        attachments: Vec<String>,
    },
    /// The chat's queue changed. Live only; `run_id` is empty.
    QueueChanged {
        queue: Vec<QueuedMessage>,
    },
    /// The chat's goal changed. Live only; `run_id` is empty.
    GoalChanged {
        goal: Option<Goal>,
    },
    /// What the goal check (or the goal supervisor) concluded after a turn.
    GoalNote {
        outcome: GoalNoteKind,
        reason: String,
    },
    /// The agent asked the user a question (`ask_user`), or updated one still
    /// open on the same topic (same `id`).
    QuestionAsked {
        question: Question,
    },
    /// The user answered a question.
    QuestionAnswered {
        id: String,
        answer: String,
    },
    /// The agent started a subagent for one part of the task.
    SubagentStarted {
        id: String,
        name: String,
        brief: String,
        model: String,
        /// Whether an OS sandbox confines its shell commands.
        confined: bool,
        /// The project paths it may change; none when it is read-only.
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        write_paths: Vec<String>,
        /// The connections whose MCP tools it may use.
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        connections: Vec<String>,
        /// Whether it has web tools.
        #[serde(default, skip_serializing_if = "std::ops::Not::not")]
        web: bool,
        /// What its shell is limited to, as its profile wrote them; empty is no limit.
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        shell_scopes: Vec<String>,
        /// The name of the subagent that started it; none when the main agent did.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        started_by: Option<String>,
        /// When it was started (RFC 3339); groups finished work by request in Tasks.
        #[serde(default, skip_serializing_if = "String::is_empty")]
        at: String,
    },
    /// The main agent wrote to a subagent after starting it; `resumed` when that picked an
    /// ended one up again.
    SubagentMessage {
        id: String,
        text: String,
        #[serde(default)]
        resumed: bool,
    },
    /// What a running subagent is doing now. Live only.
    SubagentStatus {
        id: String,
        text: String,
        tool_calls: usize,
        /// The model it fell back to, when its own could not run.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        model: Option<String>,
    },
    /// A subagent ended: `finished`, `stopped`, `failed` or `turn_limit`, with its result
    /// (or what it had so far).
    SubagentEnded {
        id: String,
        status: String,
        result: String,
        tool_calls: usize,
        seconds: u64,
        /// The model it fell back to, when its own could not run.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        model: Option<String>,
    },
}

impl From<RunEvent> for ThreadEvent {
    fn from(event: RunEvent) -> Self {
        match event {
            RunEvent::Started { task, max_turns } => Self::Started { task, max_turns },
            RunEvent::AssistantText(text) => Self::AssistantText { text },
            RunEvent::ToolCallStarted { tool, summary } => Self::ToolStarted {
                tool,
                summary,
                label: None,
            },
            RunEvent::ToolCallFinished { tool, ok, output } => Self::ToolFinished {
                tool,
                ok,
                output,
                media: None,
            },
            RunEvent::ApprovalDecided { tool, decision } => Self::ApprovalDecided {
                tool,
                decision: ApprovalDecision::from(decision),
            },
            RunEvent::ContextCompacting => Self::ContextCompacting,
            RunEvent::ContextUsage { metrics } => Self::ContextUsage { metrics },
            RunEvent::ContextBudget(budget) => Self::ContextBudget { budget },
            RunEvent::ContextSuspended { progress } => Self::ContextSuspended { progress },
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
            RunEvent::Steered { text } => Self::Steered { text },
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

/// Tauri event name carrying [`ThreadEventEnvelope`] payloads.
pub const THREAD_EVENT_NAME: &str = "thread-event";
/// Tauri event name carrying [`ApprovalRequest`] payloads.
pub const APPROVAL_REQUEST_NAME: &str = "approval-request";
/// Event name for [`ApprovalExpired`].
pub const APPROVAL_EXPIRED_NAME: &str = "approval-expired";
/// Tauri event name carrying [`ReviewItem`] payloads.
pub const REVIEW_ITEM_NAME: &str = "review-item-added";
/// Tauri event name carrying [`ThreadChange`] payloads.
pub const THREAD_CHANGED_NAME: &str = "thread-changed";

/// What happened to a thread outside the window that is showing it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ThreadChangeKind {
    /// A thread was created (for example by `themis call create_thread`).
    Created,
    /// Title, approval mode, effort or skills changed.
    Updated,
    /// The thread was discarded.
    Removed,
}

/// `thread-changed` payload (`ThreadChange` in types.ts). Carries metadata
/// only, never prompts or transcripts.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ThreadChange {
    pub kind: ThreadChangeKind,
    pub project_root: String,
    pub thread_id: String,
    /// Current info; absent for `removed`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub thread: Option<ThreadInfo>,
}

/// Tauri event name carrying [`SettingsChange`] payloads.
pub const SETTINGS_CHANGED_NAME: &str = "settings-changed";

/// Which stored settings a `settings-changed` event says to read again.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum SettingsScope {
    /// The app's `settings.json`: defaults for new chats, approvals, limits, appearance.
    App,
    /// The user (or a project) `runtime.jsonc`, including the Settings sections that write
    /// its `browser`, `web`, `subagents`, `memory` and `visions` blocks.
    Runtime,
    /// Stored keys: which ones exist, never their values.
    Secrets,
}

impl SettingsScope {
    /// The settings a successful call to `method` wrote, if any.
    #[must_use]
    pub fn written_by(method: &str) -> Option<Self> {
        match method {
            "update_settings" | "rename_project" | "delete_project" => Some(Self::App),
            "save_runtime_configuration"
            | "ensure_runtime_config"
            | "update_browser_settings"
            | "update_web_settings"
            | "update_subagent_settings"
            | "update_memory_settings"
            | "update_visions_settings" => Some(Self::Runtime),
            "set_key" | "remove_key" | "set_secret" | "clear_secret" => Some(Self::Secrets),
            _ => None,
        }
    }
}

/// `settings-changed` payload (`SettingsChange` in types.ts): settings were written through
/// the shared server. Metadata only; a window reads the settings of that scope again.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct SettingsChange {
    pub scope: SettingsScope,
}

/// Tauri event name carrying [`VisionsChange`] payloads.
pub const VISIONS_CHANGED_NAME: &str = "visions-changed";

/// `visions-changed` payload (`VisionsChange` in types.ts): Visions, channels, a channel's log
/// or which chats Visions own changed. Ids only; a window reads what it shows again. Both
/// absent means read everything again.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct VisionsChange {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub channel_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub vision_id: Option<String>,
}
