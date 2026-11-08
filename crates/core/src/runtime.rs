//! Thread runtime: single-task ReAct loop with streaming events and approvals.
//!
//! [`run_task`] drives one agent task to completion: it sends the task to the
//! LLM, executes any requested tool calls (gated by an [`ApprovalHook`]),
//! feeds results back, and repeats until the model answers with plain text or
//! `max_turns` is exhausted. Progress is reported through [`RunEvent`] values
//! on a `tokio::mpsc` channel; the enum is serde-tagged so the Tauri bridge can
//! forward it to the desktop UI unchanged.
//!
//! Approval semantics: every tool call is checked against the caller's hook
//! (wrapped in [`CachingApprovals`], which remembers `AllowAlways` per tool
//! name). A denial is *not* fatal: it becomes a tool error delivered back to
//! the model and the run continues.

use std::collections::HashSet;
use std::fmt;
use std::hash::{BuildHasher, Hash, Hasher};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

use anyhow::anyhow;
use autoagents::core::tool::{to_llm_tool, ToolT};
pub use autoagents::llm::chat::ChatRole;
use autoagents::llm::chat::{ChatMessage, MessageType, SamplingOverrides, StreamChunk, Tool};
pub use autoagents::llm::LLMProvider;
use autoagents::llm::{FunctionCall, ToolCall};
use futures_util::StreamExt;
use serde::{Deserialize, Serialize};

mod approval_decision;
pub use approval_decision::ApprovalDecision;
pub mod documents;
#[cfg(feature = "eval")]
pub mod eval;
mod ledger;
mod maintenance;
mod notes;
mod pending;
mod summary_evidence;
mod summary_job;
mod turn_end;

use crate::skills::{compose_task, filter_tools, Skill};
use crate::tools::{Approval, ApprovalHook, RiskLevel, ToolAction};

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ConversationRole {
    User,
    Assistant,
    /// Runtime-written durable checkpoint, including its serialized recent messages.
    Checkpoint,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConversationTurn {
    pub role: ConversationRole,
    pub text: String,
}

#[derive(Debug, Clone, Copy)]
pub struct RunPolicy {
    pub total_turns: usize,
    pub context_token_budget: usize,
    pub recent_messages: usize,
}

/// Streaming lifecycle events for one [`run_task`] execution.
///
/// Adjacently tagged (`{kind, data}`) for straightforward TypeScript decoding.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", content = "data", rename_all = "snake_case")]
pub enum RunEvent {
    /// The run started.
    Started {
        /// The task prompt handed to the agent.
        task: String,
        /// Maximum LLM turns before the run fails.
        max_turns: usize,
    },
    /// The assistant produced text (may accompany tool calls).
    AssistantText(String),
    /// A tool call is about to be approval-checked and executed.
    ToolCallStarted {
        /// Tool name, e.g. `"shell"`.
        tool: String,
        /// One-line summary of the call.
        summary: String,
    },
    /// A tool call finished (`ok == false` covers denials, unknown tools,
    /// bad arguments, and execution failures; details went back to the model).
    ToolCallFinished {
        /// Tool name.
        tool: String,
        /// Whether the call succeeded.
        ok: bool,
        /// Bounded tool result for the conversation display.
        output: String,
    },
    /// An approval checkpoint was decided (including cached allow-always).
    ApprovalDecided {
        /// Tool name.
        tool: String,
        /// The decision applied.
        decision: ApprovalDecision,
    },
    /// Context summarization has started.
    ContextCompacting,
    /// Content-free accounting snapshot persisted with this run.
    ContextUsage { metrics: serde_json::Value },
    /// Estimated context distribution before a bounded model request.
    ContextBudget(ContextBudget),
    /// Bounded historical action receipts after a failed checkpoint; never a summary or resume command.
    ContextSuspended { progress: String },
    /// Durable summary saved at a safe boundary between model requests.
    ContextCheckpoint { summary: String },
    /// The run stopped with a saved handoff and remaining work.
    Incomplete { result: String },
    /// The model answered with plain text; the run is complete.
    Finished {
        /// The final answer.
        result: String,
    },
    /// The run failed (LLM error or `max_turns` exhausted).
    Failed {
        /// Human-readable failure reason.
        error: String,
    },
    /// A message the user steered into this run was handed to the model.
    Steered {
        /// The user's text, as written.
        text: String,
    },
}

/// Byte-estimated context accounting for the composer meter. Counts are
/// approximate (serialized bytes / 4) and never include message content.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ContextBudget {
    pub budget_tokens: usize,
    /// Message space after tool definitions, the reply reserve and the safety margin.
    pub usable_tokens: usize,
    pub used_tokens: usize,
    pub tool_definition_tokens: usize,
    pub output_reserve_tokens: usize,
    pub safety_margin_tokens: usize,
    pub maintenance_at_tokens: usize,
    pub compaction_at_tokens: usize,
    /// The observed growth projects a checkpoint within about two interactions.
    pub compaction_soon: bool,
    pub breakdown: ContextBreakdown,
    /// Uncorrected byte estimate behind `used_tokens`; absent in older events.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub raw_estimated_tokens: Option<usize>,
    /// Provider-reported input tokens of the previous request, when reported.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub provider_input_tokens: Option<u64>,
    /// Correction ratio in thousandths (1000 = estimate was exact).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub calibration_ratio_milli: Option<u32>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ContextBreakdown {
    pub system: usize,
    pub checkpoint: usize,
    pub attachments: usize,
    pub conversation: usize,
    pub tool_activity: usize,
    /// A thinking model's own reasoning, sent back with its tool calls and earlier answers.
    #[serde(default)]
    pub reasoning: usize,
    /// The memory block in the system message (index and relevant memories), taken out of
    /// `system`.
    #[serde(default)]
    pub memory: usize,
    /// The goal note (continuation or "objective updated") the latest goal run carries: from
    /// where it starts in its user message to the end, taken out of `conversation`.
    #[serde(default)]
    pub goal: usize,
}

/// [`ApprovalHook`] wrapper that caches `AllowAlways` per tool name.
///
/// The first `AllowAlways` for a tool is remembered; later actions from the
/// same tool are approved without consulting the inner hook. `AllowOnce` and
/// `Deny` are never cached. Share one instance (via [`CachingApprovals::wrap`])
/// between [`run_task`] and the tools built by `boxed_tools` so an interactive
/// prompt answers once per tool instead of once per call.
pub struct CachingApprovals {
    inner: Arc<dyn ApprovalHook>,
    /// Grant keys; see [`ToolAction::grant_key`].
    allowed: Mutex<HashSet<String>>,
}

impl fmt::Debug for CachingApprovals {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let cached: Vec<String> = self
            .allowed
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .iter()
            .cloned()
            .collect();
        f.debug_struct("CachingApprovals")
            .field("cached_tools", &cached)
            .finish_non_exhaustive()
    }
}

impl CachingApprovals {
    /// Wraps `inner` with per-tool allow-always caching.
    pub fn new(inner: Arc<dyn ApprovalHook>) -> Self {
        Self {
            inner,
            allowed: Mutex::new(HashSet::new()),
        }
    }

    /// Wraps `inner` and returns a shared handle.
    pub fn wrap(inner: Arc<dyn ApprovalHook>) -> Arc<Self> {
        Arc::new(Self::new(inner))
    }

    /// Returns true when `tool` was previously allow-always'd (for `shell`,
    /// pass the grant key from [`ToolAction::grant_key`]).
    pub fn is_cached(&self, tool: &str) -> bool {
        self.allowed
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .contains(tool)
    }
}

impl ApprovalHook for CachingApprovals {
    fn denied_by_rule(&self, action: &ToolAction) -> bool {
        self.inner.denied_by_rule(action)
    }

    fn sensitive_access(
        &self,
        tool: &str,
        args: &serde_json::Value,
    ) -> Option<crate::tools::sensitive::SensitiveAccess> {
        self.inner.sensitive_access(tool, args)
    }

    /// Never answered from the tool-level cache: a grant on `git add` is not one on `.env`.
    fn approve_sensitive(
        &self,
        action: &ToolAction,
        access: &crate::tools::sensitive::SensitiveAccess,
    ) -> Approval {
        self.inner.approve_sensitive(action, access)
    }

    fn would_ask(&self, action: &ToolAction) -> bool {
        !self.is_cached(&action.grant_key()) && self.inner.would_ask(action)
    }

    fn approve(&self, action: &ToolAction) -> Approval {
        let key = action.grant_key();
        if self.is_cached(&key) {
            return Approval::AllowAlways;
        }
        let decision = self.inner.approve(action);
        if decision == Approval::AllowAlways {
            self.allowed
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner())
                .insert(key);
        }
        decision
    }
}

/// Classifies a tool call for the approval checkpoint.
fn risk_for(tool: &str) -> RiskLevel {
    if tool.starts_with("mcp_") {
        return RiskLevel::Network;
    }
    match tool {
        // attach_file only shows project files under the reply.
        "read_file" | "list_dir" | "search_file" | "recall" | "attach_file" => RiskLevel::Read,
        "shell" => RiskLevel::Execute,
        _ => RiskLevel::Write,
    }
}

/// Builds the one-line `tool (args)` summary, whitespace-folded and capped.
fn take_char_prefix(input: &str, limit: usize) -> (String, bool) {
    let mut chars = input.chars();
    let prefix = chars.by_ref().take(limit).collect();
    (prefix, chars.next().is_some())
}

/// Arguments that say what a call acts on: listed first in its summary, so a long `content`
/// cannot push the path out of the cut-off summary that the chat, the steps record and evals see.
const SUMMARY_FIRST_KEYS: [&str; 7] = [
    "file_path",
    "path",
    "source",
    "destination",
    "directory",
    "command",
    "args",
];

fn summary_arguments(args: &str) -> String {
    let Ok(serde_json::Value::Object(map)) = serde_json::from_str::<serde_json::Value>(args) else {
        return args.to_owned();
    };
    let first = SUMMARY_FIRST_KEYS
        .iter()
        .filter_map(|key| map.get_key_value(*key));
    let rest = map
        .iter()
        .filter(|(key, _)| !SUMMARY_FIRST_KEYS.contains(&key.as_str()));
    let fields: Vec<String> = first
        .chain(rest)
        .map(|(key, value)| format!("{}:{value}", serde_json::Value::String(key.clone())))
        .collect();
    format!("{{{}}}", fields.join(","))
}

fn summarize_call(name: &str, args: &str) -> String {
    const MAX_CHARS: usize = 200;
    let args = summary_arguments(args);
    let single_line = args.split_whitespace().collect::<Vec<_>>().join(" ");
    let full = format!("call `{name}` with {single_line}");
    let (mut summary, truncated) = take_char_prefix(&full, MAX_CHARS);
    if truncated {
        summary.push('…');
    }
    summary
}

/// Runs one agent task to completion with a ReAct loop.
///
/// - `llm` is the resolved provider handle (see `providers::resolve`).
/// - `tools` are the boxed tools the model may call (see `tools::boxed_tools`).
/// - `task` is the user task prompt.
/// - `approvals` gates every tool call; it is wrapped in [`CachingApprovals`]
///   so `AllowAlways` sticks per tool name. Denials become tool errors fed
///   back to the model and the run continues.
/// - `max_turns` caps LLM round-trips; exhaustion emits
///   [`RunEvent::Failed`] and returns an error naming the limit.
/// - `events` receives the [`RunEvent`] stream; a closed receiver never fails
///   the run (sends are best-effort).
///
/// Returns the model's final text on [`RunEvent::Finished`].
pub async fn run_task(
    llm: Arc<dyn LLMProvider>,
    tools: Vec<Box<dyn ToolT>>,
    task: String,
    approvals: Arc<dyn ApprovalHook>,
    max_turns: usize,
    events: tokio::sync::mpsc::Sender<RunEvent>,
) -> anyhow::Result<String> {
    run_task_with_stop(
        llm,
        tools,
        task,
        Vec::new(),
        approvals,
        max_turns,
        events,
        Arc::new(AtomicBool::new(false)),
    )
    .await
}

/// Stops between model requests and tools. An action already executing finishes first.
#[expect(
    clippy::too_many_arguments,
    reason = "run inputs match the runtime boundary"
)]
pub async fn run_task_with_stop(
    llm: Arc<dyn LLMProvider>,
    tools: Vec<Box<dyn ToolT>>,
    task: String,
    history: Vec<ConversationTurn>,
    approvals: Arc<dyn ApprovalHook>,
    max_turns: usize,
    events: tokio::sync::mpsc::Sender<RunEvent>,
    stopped: Arc<AtomicBool>,
) -> anyhow::Result<String> {
    run_task_with_policy(
        llm,
        tools,
        task,
        history,
        approvals,
        RunPolicy {
            total_turns: max_turns,
            context_token_budget: usize::MAX,
            recent_messages: 20,
        },
        events,
        stopped,
    )
    .await
}

#[expect(
    clippy::too_many_arguments,
    reason = "run inputs match the runtime boundary"
)]
pub async fn run_task_with_policy(
    llm: Arc<dyn LLMProvider>,
    tools: Vec<Box<dyn ToolT>>,
    task: String,
    history: Vec<ConversationTurn>,
    approvals: Arc<dyn ApprovalHook>,
    policy: RunPolicy,
    events: tokio::sync::mpsc::Sender<RunEvent>,
    stopped: Arc<AtomicBool>,
) -> anyhow::Result<String> {
    run_task_with_policy_and_catalog(
        llm,
        tools,
        task,
        history,
        approvals,
        policy,
        events,
        stopped,
        String::new(),
    )
    .await
}

/// Run with an immutable metadata-only skill catalog in the system message.
#[allow(clippy::too_many_arguments)]
pub async fn run_task_with_policy_and_catalog(
    llm: Arc<dyn LLMProvider>,
    tools: Vec<Box<dyn ToolT>>,
    task: String,
    history: Vec<ConversationTurn>,
    approvals: Arc<dyn ApprovalHook>,
    policy: RunPolicy,
    events: tokio::sync::mpsc::Sender<RunEvent>,
    stopped: Arc<AtomicBool>,
    skill_catalog: String,
) -> anyhow::Result<String> {
    run_task_with_evidence(
        llm,
        tools,
        task,
        history,
        approvals,
        policy,
        events,
        stopped,
        skill_catalog,
        None,
    )
    .await
}

/// Runs with durable project-local originals accessible to the existing file/shell tools.
#[allow(clippy::too_many_arguments)]
pub async fn run_task_with_evidence(
    llm: Arc<dyn LLMProvider>,
    tools: Vec<Box<dyn ToolT>>,
    task: String,
    history: Vec<ConversationTurn>,
    approvals: Arc<dyn ApprovalHook>,
    policy: RunPolicy,
    events: tokio::sync::mpsc::Sender<RunEvent>,
    stopped: Arc<AtomicBool>,
    skill_catalog: String,
    evidence_directory: Option<std::path::PathBuf>,
) -> anyhow::Result<String> {
    let budget = effective_context_budget(policy.context_token_budget, llm.model());
    let output_cap = crate::providers::answer_output_cap(llm.model(), budget);
    let future = crate::context_metrics::collect(async {
        let final_events = events.clone();
        let result = Box::pin(run_task_with_evidence_inner(
            llm,
            tools,
            task,
            history,
            approvals,
            policy,
            events,
            stopped,
            skill_catalog,
            evidence_directory,
        ))
        .await;
        if let Some(metrics) = crate::context_metrics::snapshot() {
            final_events
                .send(RunEvent::ContextUsage { metrics })
                .await
                .ok();
        }
        result
    });
    MODEL_REQUEST_BUDGET
        .scope(
            budget,
            crate::providers::OUTPUT_LIMIT.scope(output_cap, future),
        )
        .await
}

#[allow(clippy::too_many_arguments)]
async fn run_task_with_evidence_inner(
    llm: Arc<dyn LLMProvider>,
    tools: Vec<Box<dyn ToolT>>,
    task: String,
    history: Vec<ConversationTurn>,
    approvals: Arc<dyn ApprovalHook>,
    policy: RunPolicy,
    events: tokio::sync::mpsc::Sender<RunEvent>,
    stopped: Arc<AtomicBool>,
    skill_catalog: String,
    evidence_directory: Option<std::path::PathBuf>,
) -> anyhow::Result<String> {
    let requested_budget = policy.context_token_budget;
    let mut window = model_window(llm.model());
    let effective_budget = crate::context_window::effective_budget(requested_budget, window);
    let mut policy = RunPolicy {
        context_token_budget: effective_budget,
        ..policy
    };
    let emit = |event: RunEvent| async {
        if matches!(
            event,
            RunEvent::Finished { .. } | RunEvent::Incomplete { .. } | RunEvent::Failed { .. }
        ) {
            if let Some(metrics) = crate::context_metrics::snapshot() {
                events.send(RunEvent::ContextUsage { metrics }).await.ok();
            }
        }
        events.send(event).await
    };
    let caching = CachingApprovals::wrap(approvals);
    let llm_tools: Vec<Tool> = tools.iter().map(to_llm_tool).collect();
    // Byte estimates are approximate bookkeeping, not a model-specific tokenizer.
    let schema_tokens = serde_json::to_vec(&llm_tools)?.len().div_ceil(4);
    let (mut answer_reserve, mut safety_margin, mut input_budget) =
        request_reserves(llm.model(), policy.context_token_budget, schema_tokens);
    if input_budget == 0 {
        let error = "Tool definitions and answer reserve exceed the context budget".to_owned();
        emit(RunEvent::Failed {
            error: error.clone(),
        })
        .await
        .ok();
        return Err(anyhow!(error));
    }
    let config_fingerprint = {
        use std::hash::{Hash, Hasher};
        let mut hash = std::collections::hash_map::DefaultHasher::new();
        serde_json::to_vec(&compaction_config())?.hash(&mut hash);
        format!("{:016x}", hash.finish())
    };
    let identity = crate::diagnostics::ACTIVE
        .try_with(|context| (context.thread_id.clone(), context.run_id.clone()))
        .ok();
    crate::context_metrics::set_run_metadata(
        serde_json::json!({"thread_id":identity.as_ref().map(|v|&v.0),"run_id":identity.as_ref().map(|v|&v.1),"task_id":identity.as_ref().map(|v|&v.1),"configuration_fingerprint":crate::context_metrics::config_fingerprint(),"compaction_config_fingerprint":config_fingerprint,"requested_context_tokens":requested_budget,"effective_context_tokens":effective_budget,"provider_capacity_tokens":window.tokens,"capacity_source":window.source.as_str(),"schema_estimated_tokens":schema_tokens,"output_reserve_tokens":answer_reserve,"safety_margin_tokens":safety_margin,"usable_message_tokens":input_budget}),
    );
    let mut context_policy = RunPolicy {
        context_token_budget: input_budget,
        ..policy
    };

    emit(RunEvent::Started {
        task: task.clone(),
        max_turns: policy.total_turns,
    })
    .await
    .ok();

    let mut messages = initial_messages(history, task);
    messages[0].content.push_str(&skill_catalog);
    if let Some(directory) = &evidence_directory {
        messages[0].content.push_str(&evidence_pointer(
            directory,
            llm_tools.iter().any(|tool| tool.function.name == "recall"),
        )?);
        messages[0].content.push('\n');
        messages[0]
            .content
            .push_str(crate::prompts::SYSTEM_TASK_LEDGER);
    }

    let mut observations = HashSet::new();
    let mut last_shell_observation = None;
    let mut unchanged_results = 0;
    // Whether this run changed files, and whether its spec check was already sent.
    let mut changed_files = false;
    let mut spec_checked = false;
    // Whether the run has used tools, and the one extra request a stretch of work may get
    // when a reply with no tool call does not read as the end of the turn.
    let mut used_tools = false;
    let mut turn_end_guard = turn_end::TurnEndGuard::default();
    let mut stop_hook_rounds = 0usize;
    // The reply just sent back by that guard, which the next reply must not show again.
    let mut sent_back: Option<String> = None;
    // What the run already told the user before a runtime check or note sent it back
    // without a tool call: the final answer continues it instead of repeating it.
    let mut answered = String::new();
    // Steered messages this run read, for the check before it ends.
    let mut steered: Vec<String> = Vec::new();
    let mut steers_checked = false;
    let config = compaction_config();
    // Prior turns arrive as text or as checkpoint-retained messages that a
    // checkpoint already summarized; only this run's new work is unsummarized.
    let mut summarized_until = messages.len() - 1;
    let mut previous_size = estimated_tokens(&messages);
    // The model request (turn) at which each checkpoint was installed, for thrash detection.
    let mut checkpoint_turns: Vec<usize> = Vec::new();
    let mut growth = 0usize;
    // Budget decisions use the raw byte estimate scaled by what the provider reported.
    let mut calibration = crate::calibration::TokenCalibration::new();
    let usage_sink = Arc::new(std::sync::atomic::AtomicU64::new(0));
    let mut last_reported_input = None;
    let mut overflow_retried = false;
    let mut force_compaction = false;
    let mut turn = 0;
    let mut action_counts = std::collections::HashMap::new();
    let mut reaction_due = false;
    // Screenshots this run's model may see, when it reads images (none otherwise).
    let mut recent_images =
        crate::vision::RecentImages::new(if crate::providers::accepts_images(llm.model()) {
            crate::vision::KEPT_IMAGES
        } else {
            0
        });
    while turn < policy.total_turns {
        turn += 1;
        check_stopped(&stopped, &events).await?;
        // Steered messages join the conversation at this boundary, after any
        // running tool finished and before the next model request.
        for text in take_steers() {
            messages.push(ChatMessage {
                role: ChatRole::User,
                message_type: MessageType::Text,
                content: steered_message(&text),
            });
            if crate::secret_look::looks_like_secret(&text) {
                messages.push(pasted_secret_note());
            }
            steered.push(text.clone());
            events.send(RunEvent::Steered { text }).await.ok();
        }
        // What a finished background hook reported joins here too, labelled as the plugin's.
        for note in crate::plugins::hooks::rewake_notes_for_run() {
            messages.push(ChatMessage {
                role: ChatRole::User,
                message_type: MessageType::Text,
                content: note.model_text(),
            });
            // A row in the chat, so the person sees what the agent was told about.
            events
                .send(RunEvent::ToolCallFinished {
                    tool: format!("hook_{}", note.hook),
                    ok: true,
                    output: note.summary,
                })
                .await
                .ok();
        }
        // Results of subagents (and other notes) join at the same boundary.
        for note in run_notes().map(|notes| notes.take()).unwrap_or_default() {
            // Memory the memory models changed meanwhile: information as it stands, no reaction.
            if let Some(memory) = note.strip_prefix(MEMORY_NOTE_MARKER) {
                messages.push(ChatMessage {
                    role: ChatRole::User,
                    message_type: MessageType::Text,
                    content: memory.trim_start().to_owned(),
                });
                continue;
            }
            reaction_due = true;
            messages.push(ChatMessage {
                role: ChatRole::User,
                message_type: MessageType::Text,
                content: subagent_note(&note),
            });
        }
        // A subagent just reported: this step gives the main agent no tools, so it says what it
        // makes of the reply before doing anything about it, whichever model it is.
        let reacting = std::mem::take(&mut reaction_due) && !is_subagent_run();
        if reacting {
            messages.push(ChatMessage {
                role: ChatRole::User,
                message_type: MessageType::Text,
                content: SUBAGENT_REACTION_NOTE.to_owned(),
            });
        }
        // Compaction and maintenance compare raw estimates, so hand them the equivalent raw budget.
        context_policy.context_token_budget = calibration.raw_budget(input_budget, schema_tokens);
        let raw_size = estimated_tokens(&messages);
        let size = calibration.messages(raw_size, schema_tokens);
        let added = raw_size.saturating_sub(previous_size);
        if added > 0 {
            growth = if growth == 0 {
                added
            } else {
                (growth * 3 + added) / 4
            };
        }
        if size >= (input_budget as f64 * config.maintenance_threshold_ratio) as usize {
            crate::context_metrics::activity("maintenance_attempts");
            if maintenance::maintain(
                &mut messages,
                &context_policy,
                evidence_directory.as_deref(),
                summarized_until,
            )
            .await?
            {
                crate::context_metrics::activity("maintenance_reductions");
            }
        }
        let calibrated_growth = (growth as f64 * calibration.factor()) as usize;
        let forecast = calibrated_growth
            .saturating_mul(config.target_headroom_interactions)
            .min(input_budget / 8);
        let semantic_threshold = ((input_budget as f64 * config.semantic_threshold_ratio) as usize)
            .min(input_budget.saturating_sub(forecast));
        // A short conversational reply (greeting, thanks) does not pay for a checkpoint
        // while the request still fits the hard budget; the next substantive turn compacts.
        let forced = std::mem::take(&mut force_compaction);
        let calibrated_now = calibrated_tokens(&calibration, &messages, schema_tokens);
        let defer_for_short_reply = turn == 1
            && !forced
            && calibrated_now > semantic_threshold
            && calibrated_now.saturating_add(forecast) <= input_budget
            && SHORT_REPLY_TURN.try_with(|short| *short).unwrap_or(false);
        if defer_for_short_reply {
            crate::context_metrics::activity("compaction_deferred_short_reply");
            crate::diagnostics::emit(
                "context",
                "compaction_deferred_short_reply",
                "info",
                serde_json::json!({"calibrated_message_tokens":calibrated_now,"semantic_threshold_tokens":semantic_threshold,"usable_message_tokens":input_budget}),
            );
        }
        // Room for work after the system message, and the budget that would leave enough.
        let system_tokens = |messages: &[ChatMessage]| {
            let parts = context_breakdown(messages);
            calibration.messages(parts.system + parts.memory, 0)
        };
        let work_room =
            |messages: &[ChatMessage]| input_budget.saturating_sub(system_tokens(messages));
        let minimum_budget = |messages: &[ChatMessage]| {
            policy
                .context_token_budget
                .saturating_sub(input_budget)
                .saturating_add(system_tokens(messages))
                .saturating_add(crate::context_window::MIN_WORK_ROOM_TOKENS)
        };
        if turn == 1
            && policy.context_token_budget != usize::MAX
            && work_room(&messages) < crate::context_window::MIN_WORK_ROOM_TOKENS
        {
            crate::diagnostics::emit(
                "context",
                "budget_below_minimum",
                "warn",
                serde_json::json!({"budget_tokens":policy.context_token_budget,"work_room_tokens":work_room(&messages),"minimum_budget_tokens":minimum_budget(&messages)}),
            );
        }
        // The instructions and tools leave no room to work: a checkpoint can only shorten the
        // conversation, so say so now instead of failing checkpoint after checkpoint.
        if turn == 1
            && policy.context_token_budget != usize::MAX
            && work_room(&messages) < crate::context_window::MIN_START_ROOM_TOKENS
        {
            let fixed = policy
                .context_token_budget
                .saturating_sub(input_budget)
                .saturating_add(system_tokens(&messages));
            let error = format!(
                "Stopped: the context budget of {} tokens is too small. The instructions and tools alone need about {} tokens; raise the context budget to at least {} tokens.",
                grouped(policy.context_token_budget),
                grouped(fixed),
                grouped(minimum_budget(&messages))
            );
            crate::diagnostics::emit(
                "context",
                "budget_below_instructions",
                "warn",
                serde_json::json!({"budget_tokens":policy.context_token_budget,"fixed_tokens":fixed,"minimum_budget_tokens":minimum_budget(&messages)}),
            );
            emit(RunEvent::Failed {
                error: error.clone(),
            })
            .await
            .ok();
            return Err(anyhow!(error));
        }
        // The context budget the composer meter and checkpoint divider show.
        let budget_now = |messages: &[ChatMessage]| {
            let raw_used = estimated_tokens(messages);
            let used = calibration.messages(raw_used, schema_tokens);
            ContextBudget {
                budget_tokens: policy.context_token_budget,
                usable_tokens: input_budget,
                used_tokens: used,
                tool_definition_tokens: schema_tokens,
                output_reserve_tokens: answer_reserve,
                safety_margin_tokens: safety_margin,
                maintenance_at_tokens: (input_budget as f64 * config.maintenance_threshold_ratio)
                    as usize,
                compaction_at_tokens: semantic_threshold,
                compaction_soon: used.saturating_add(calibrated_growth.saturating_mul(2))
                    > semantic_threshold,
                breakdown: context_breakdown(messages),
                raw_estimated_tokens: Some(raw_used),
                provider_input_tokens: last_reported_input,
                calibration_ratio_milli: calibration
                    .ratio()
                    .map(|ratio| (ratio * 1000.0).round() as u32),
            }
        };
        let compacting_now =
            !defer_for_short_reply && (forced || calibrated_now > semantic_threshold);
        if compacting_now {
            // Report the size that triggered the checkpoint (including this turn's new
            // input), so its divider reads "before → after" rather than the last turn's size.
            if policy.context_token_budget != usize::MAX {
                emit(RunEvent::ContextBudget(budget_now(&messages)))
                    .await
                    .ok();
            }
            crate::plugins::hooks::emit_current(
                "BeforeCompaction",
                serde_json::json!({"event":"BeforeCompaction"}),
                &events,
            )
            .await?;
            match compact_context(
                &llm,
                &mut messages,
                &context_policy,
                &events,
                &stopped,
                evidence_directory.as_deref(),
            )
            .await
            {
                Ok(installed) => {
                    if installed {
                        summarized_until = messages.len();
                        checkpoint_turns.push(turn);
                        if thrashing(&checkpoint_turns, config.thrash_checkpoints) {
                            let error = format!(
                                "Stopped: the context budget is too small to make progress ({} checkpoints, one every two requests or sooner). It leaves about {} tokens for work after the instructions and tools; raise the context budget to at least {} tokens.",
                                config.thrash_checkpoints,
                                grouped(work_room(&messages)),
                                grouped(minimum_budget(&messages))
                            );
                            crate::diagnostics::emit(
                                "context",
                                "compaction_thrash",
                                "warn",
                                serde_json::json!({"checkpoints":config.thrash_checkpoints,"work_room_tokens":work_room(&messages),"minimum_budget_tokens":minimum_budget(&messages)}),
                            );
                            emit(RunEvent::Failed {
                                error: error.clone(),
                            })
                            .await
                            .ok();
                            return Err(anyhow!(error));
                        }
                    }
                }
                Err(error) if stopped.load(Ordering::SeqCst) => return Err(error),
                Err(error) => {
                    let error = format!("Context checkpoint failed: {error}");
                    emit(RunEvent::Failed {
                        error: error.clone(),
                    })
                    .await
                    .ok();
                    return Err(anyhow!(error));
                }
            }
        }
        if calibrated_tokens(&calibration, &messages, schema_tokens) > input_budget {
            let error = "Context still exceeds the request budget after one compaction; originals preserved".to_owned();
            emit(RunEvent::Failed {
                error: error.clone(),
            })
            .await
            .ok();
            return Err(anyhow!(error));
        }
        if let Some(metrics) = crate::context_metrics::snapshot() {
            emit(RunEvent::ContextUsage { metrics }).await.ok();
        }
        if policy.context_token_budget != usize::MAX {
            emit(RunEvent::ContextBudget(budget_now(&messages)))
                .await
                .ok();
        }
        crate::diagnostics::emit(
            "context",
            "request_budget",
            "info",
            serde_json::json!({"context_token_budget":policy.context_token_budget,"schema_tokens":schema_tokens,"output_reserve_tokens":answer_reserve,"safety_margin_tokens":safety_margin,"usable_message_tokens":input_budget,"estimated_message_tokens":estimated_tokens(&messages),"calibrated_message_tokens":calibrated_tokens(&calibration, &messages, schema_tokens),"calibration_ratio":calibration.ratio(),"semantic_threshold_tokens":semantic_threshold,"forecast_growth_tokens":forecast,"requested_context_tokens":requested_budget,"provider_capacity_tokens":window.tokens,"capacity_source":window.source.as_str()}),
        );
        previous_size = estimated_tokens(&messages);
        usage_sink.store(0, Ordering::SeqCst);
        // The latest screenshots go with this request only; the history keeps their text.
        let (request_messages, attached) = recent_images.request(&messages);
        if attached.images > 0 {
            crate::diagnostics::emit(
                "vision",
                "images_sent",
                "info",
                serde_json::json!({"model": llm.model(), "image_count": attached.images, "bytes": attached.bytes}),
            );
        }
        let image_tokens = attached.images * crate::calibration::IMAGE_TOKENS;
        let (text, calls) = match crate::context_metrics::with_usage_sink(
            usage_sink.clone(),
            SENT_BACK_REPLY.scope(
                sent_back.clone(),
                request_answer(
                    &llm,
                    &request_messages,
                    if reacting { &[] } else { &llm_tools },
                    &events,
                    &stopped,
                ),
            ),
        )
        .await
        {
            Ok(answer) => {
                sent_back = None;
                overflow_retried = false;
                let raw_request = previous_size
                    .saturating_add(schema_tokens)
                    .saturating_add(image_tokens);
                let reported = usage_sink.load(Ordering::SeqCst);
                let sample = calibration_sample(&mut calibration, raw_request, reported);
                last_reported_input = (reported > 0).then_some(reported);
                crate::diagnostics::emit("context", "token_calibration", "info", sample.clone());
                crate::context_metrics::calibration_sample(sample);
                answer
            }
            Err(error) => {
                // check_stopped already emitted the cancellation event.
                if stopped.load(Ordering::SeqCst) {
                    return Err(error);
                }
                // Some providers refuse a request without tools once tools were used: skip the
                // pause rather than lose the run over it.
                if reacting {
                    crate::diagnostics::emit(
                        "subagents",
                        "reaction_skipped",
                        "warn",
                        serde_json::json!({}),
                    );
                    continue;
                }
                // The provider rejected the input size before any answer text: tighten the
                // estimate and compact once, then retry. Completed tools are never replayed.
                if !overflow_retried
                    && error
                        .downcast_ref::<autoagents::llm::error::LLMError>()
                        .is_some_and(crate::calibration::error_is_context_overflow)
                {
                    overflow_retried = true;
                    force_compaction = true;
                    // The request was larger than the model accepts. A run that knows
                    // its model's window learns the real limit for that model; when it
                    // is below this run's budget the budget shrinks so compaction fits.
                    let previous_window = window.tokens;
                    let learned = (window.source
                        != crate::context_window::WindowSource::Unverified)
                        .then(|| {
                            let failed_request =
                                calibrated_tokens(&calibration, &messages, schema_tokens)
                                    .saturating_add(schema_tokens)
                                    .saturating_add(answer_reserve);
                            crate::context_window::learn_limit(
                                llm.model(),
                                crate::context_window::implied_limit(
                                    &overflow_error_text(&error),
                                    failed_request,
                                ),
                            )
                        });
                    window = model_window(llm.model());
                    let shrunk = learned.filter(|limit| *limit < policy.context_token_budget);
                    if let Some(limit) = shrunk {
                        policy.context_token_budget = limit;
                        (answer_reserve, safety_margin, input_budget) =
                            request_reserves(llm.model(), limit, schema_tokens);
                        if input_budget == 0 {
                            let error = format!("Tool definitions and answer reserve exceed this model's context window ({limit} tokens)");
                            emit(RunEvent::Failed {
                                error: error.clone(),
                            })
                            .await
                            .ok();
                            return Err(anyhow!(error));
                        }
                    } else {
                        calibration.tighten(previous_size, schema_tokens, semantic_threshold);
                    }
                    crate::context_metrics::activity("context_overflow_recoveries");
                    crate::diagnostics::emit(
                        "context",
                        "context_overflow",
                        "warn",
                        serde_json::json!({"estimated_message_tokens":previous_size,"schema_tokens":schema_tokens,"calibration_ratio":calibration.ratio(),"action":"compact_and_retry_once","previous_window_tokens":previous_window,"learned_window_tokens":learned,"capacity_source":window.source.as_str(),"budget_shrunk":shrunk.is_some(),"context_token_budget":policy.context_token_budget}),
                    );
                    turn -= 1;
                    continue;
                }
                let error = format!("LLM request failed: {error}");
                emit(RunEvent::Failed {
                    error: error.clone(),
                })
                .await
                .ok();
                return Err(anyhow!(error));
            }
        };
        // The guard reads the reply as it came; everything after it, and the model's next request,
        // has what the person saw.
        let reply_as_sent = text.clone();
        let text = text
            .map(|reply| turn_end::before_written_call(&reply))
            .filter(|reply| !reply.is_empty());
        check_stopped(&stopped, &events).await?;
        // A message steered in while the model wrote its answer keeps the run
        // going: the answer stays, and the next request reads the message.
        // The reaction is said, not an answer: the run carries on with its tools back.
        if reacting && calls.is_empty() {
            let line = text.clone().unwrap_or_default();
            if !line.trim().is_empty() {
                events
                    .send(RunEvent::AssistantText("\n\n".into()))
                    .await
                    .ok();
            }
            messages.push(ChatMessage {
                role: ChatRole::Assistant,
                message_type: MessageType::Text,
                content: line,
            });
            continue;
        }
        // An answer while subagents are still working keeps the run waiting for their results.
        let notes = run_notes().filter(|notes| notes.waiting() || notes.pending() > 0);
        if calls.is_empty() && !steer_waiting() {
            if let Some(notes) = &notes {
                let answer = text.clone().unwrap_or_default();
                answered = joined(&answered, &answer);
                if !answer.trim().is_empty() {
                    events
                        .send(RunEvent::AssistantText("\n\n".into()))
                        .await
                        .ok();
                }
                messages.push(ChatMessage {
                    role: ChatRole::Assistant,
                    message_type: MessageType::Text,
                    content: answer,
                });
                wait_for_note(notes, &stopped).await;
                check_stopped(&stopped, &events).await?;
                continue;
            }
        }
        if calls.is_empty() && steer_waiting() {
            let answer = text.unwrap_or_default();
            if !answer.trim().is_empty() {
                events
                    .send(RunEvent::AssistantText("\n\n".into()))
                    .await
                    .ok();
            }
            messages.push(ChatMessage {
                role: ChatRole::Assistant,
                message_type: MessageType::Text,
                content: answer,
            });
            continue;
        }
        if calls.is_empty() {
            // A run that read two or more steered messages is shown them once before it ends:
            // with several arriving mid-task, agents answered the latest and dropped earlier ones
            // (verify eval, interleaved-requests at 32k: 3 of 3 runs).
            if steered.len() >= 2 && !steers_checked && !nudge_off("steers_check") {
                steers_checked = true;
                crate::context_metrics::activity("steers_checks");
                let answer = text.clone().unwrap_or_default();
                answered = joined(&answered, &answer);
                if !answer.trim().is_empty() {
                    events
                        .send(RunEvent::AssistantText("\n\n".into()))
                        .await
                        .ok();
                }
                messages.push(ChatMessage {
                    role: ChatRole::Assistant,
                    message_type: MessageType::Text,
                    content: answer,
                });
                messages.push(ChatMessage {
                    role: ChatRole::User,
                    message_type: MessageType::Text,
                    content: steers_check(&steered),
                });
                continue;
            }
            // A run that changed files checks its work against the request once before it
            // ends: agents report success when their own tests pass, though those cover only
            // what they remembered to test.
            if changed_files && !spec_checked && !nudge_off("spec_check") {
                spec_checked = true;
                crate::context_metrics::activity("spec_checks");
                let answer = text.unwrap_or_default();
                answered = joined(&answered, &answer);
                // The checked answer starts a new paragraph after the one already shown.
                if !answer.trim().is_empty() {
                    events
                        .send(RunEvent::AssistantText("\n\n".into()))
                        .await
                        .ok();
                }
                messages.push(ChatMessage {
                    role: ChatRole::Assistant,
                    message_type: MessageType::Text,
                    content: answer,
                });
                messages.push(ChatMessage {
                    role: ChatRole::User,
                    message_type: MessageType::Text,
                    content: SPEC_CHECK.to_owned(),
                });
                continue;
            }
            // A reply with no tool call ends the turn only if it reads as an ending. A short
            // progress line ("Continuing through the ember files.") is not one: agents narrated
            // a step and stopped (replays, 13 of 1,452 turns). Nor is a reply that only
            // announces its next step or writes a tool call as text (a free model's goal run,
            // 3 of 6 checked turns). Asked once, it continues or sums up. The last request
            // ends the run with what it has.
            let unfinished = (!llm_tools.is_empty()
                && turn < policy.total_turns
                && !nudge_off("turn_summary"))
            .then(|| turn_end_guard.check(reply_as_sent.as_deref().unwrap_or_default(), used_tools))
            .flatten();
            if let Some(reason) = unfinished {
                let answer = text.unwrap_or_default();
                let note = if reason == turn_end::Unfinished::ProgressLine {
                    crate::context_metrics::activity("turn_summary_requests");
                    TURN_SUMMARY
                } else {
                    crate::context_metrics::activity("turn_end_guard_requests");
                    TURN_END_GUARD
                };
                crate::diagnostics::emit(
                    "answer",
                    "turn_end_guard",
                    "info",
                    serde_json::json!({"reason":reason.as_str(),"request":turn,"reply_chars":answer.chars().count()}),
                );
                // The line is not part of the answer: the chat keeps it as it is, and the next
                // reply starts its own paragraph without showing it again.
                sent_back = Some(answer.clone());
                messages.push(ChatMessage {
                    role: ChatRole::Assistant,
                    message_type: MessageType::Text,
                    content: answer,
                });
                messages.push(ChatMessage {
                    role: ChatRole::User,
                    message_type: MessageType::Text,
                    content: note.to_owned(),
                });
                continue;
            }
            // The turn would end here. A `Stop` hook may ask it to go on, as a loop plugin does;
            // each round is a request like any other, so the turn limit and Stop bound it.
            if turn < policy.total_turns && !stopped.load(Ordering::SeqCst) {
                let reply = text.clone().unwrap_or_default();
                let refused =
                    crate::plugins::hooks::stop_refused(&reply, stop_hook_rounds > 0, &events)
                        .await;
                if let Some(keep_going) = refused {
                    stop_hook_rounds += 1;
                    crate::context_metrics::activity("stop_hook_rounds");
                    crate::diagnostics::emit(
                        "hook",
                        "stop_kept_going",
                        "info",
                        serde_json::json!({"hook":keep_going.hook,"rounds":stop_hook_rounds,"request":turn}),
                    );
                    answered = joined(&answered, &reply);
                    if !reply.trim().is_empty() {
                        events
                            .send(RunEvent::AssistantText("\n\n".into()))
                            .await
                            .ok();
                    }
                    messages.push(ChatMessage {
                        role: ChatRole::Assistant,
                        message_type: MessageType::Text,
                        content: reply,
                    });
                    messages.push(ChatMessage {
                        role: ChatRole::User,
                        message_type: MessageType::Text,
                        content: keep_going.note(),
                    });
                    continue;
                }
            }
            let result = joined(&answered, &text.unwrap_or_default());
            emit(RunEvent::Finished {
                result: result.clone(),
            })
            .await
            .ok();
            return Ok(result);
        }

        used_tools = true;
        turn_end_guard.tool_ran();
        if let Some(said) = text.as_deref() {
            crate::plugins::hooks::note_reply_current(said);
        }
        // The line a subagent tool carries is said before the tool runs, when the reply had none.
        if text.as_deref().is_none_or(|text| text.trim().is_empty()) && !is_subagent_run() {
            if let Some(line) = said_with_calls(&calls) {
                events.send(RunEvent::AssistantText(line)).await.ok();
            }
        }
        // A tool call starts new work: an earlier answer is no longer the turn's answer.
        answered.clear();
        let mut results = execute_tool_calls(
            &tools,
            &caching,
            &calls,
            &events,
            &stopped,
            evidence_directory.as_deref(),
            text.as_deref().unwrap_or_default(),
            policy.context_token_budget.saturating_mul(4) / 8,
        )
        .await?;
        // What `PostToolBatch` hooks hear about, as the tools answered.
        let batch: Vec<(String, serde_json::Value, String)> = calls
            .iter()
            .zip(&results)
            .map(|(call, result)| {
                (
                    call.function.name.clone(),
                    serde_json::from_str(&call.function.arguments).unwrap_or_default(),
                    result.function.arguments.clone(),
                )
            })
            .collect();
        for (call, result) in calls.iter().zip(&mut results) {
            // Actions on a page change it, so its results never look unchanged: a run that kept
            // clicking the same spot went unnoticed. These count by the call itself.
            if is_repeatable_action(&call.function.name, &call.function.arguments) {
                let times = action_counts
                    .entry(content_fingerprint(&format!(
                        "{}{}",
                        call.function.name, call.function.arguments
                    )))
                    .or_insert(0u32);
                *times += 1;
                if *times >= REPEATED_ACTION_NOTE_AT {
                    let note = crate::prompts::fill(
                        crate::prompts::RUNTIME_REPEATED_ACTION,
                        &[("times", &times.to_string())],
                    );
                    annotate_result(result, "loop_warning", &note);
                }
            }
            if call.function.name == "wait_subagents"
                && result.function.arguments.contains("\"result\":")
            {
                reaction_due = true;
            }
            // ponytail: exact observations only; semantic progress needs a separate audit.
            if matches!(
                call.function.name.as_str(),
                "read_file" | "list_dir" | "search_file" | "shell" | "recall"
            ) {
                let value = serde_json::from_str::<serde_json::Value>(&result.function.arguments)
                    .unwrap_or_default();
                let fingerprint = value["original_result_fingerprint"]
                    .as_u64()
                    .unwrap_or_else(|| content_fingerprint(&result.function.arguments));
                // Canonical read output already identifies the path, range and content.
                let args = if call.function.name == "read_file" && value["path"].is_string() {
                    serde_json::Value::Null
                } else {
                    serde_json::from_str::<serde_json::Value>(&call.function.arguments)
                        .unwrap_or_else(|_| call.function.arguments.clone().into())
                };
                let key = (
                    call.function.name.clone(),
                    content_fingerprint(&args.to_string()),
                    fingerprint,
                );
                let unchanged = if call.function.name == "shell" {
                    // Shell can mutate files silently; only retain its existing adjacent-call guard.
                    observations.clear();
                    let unchanged = last_shell_observation.as_ref() == Some(&key);
                    last_shell_observation = Some(key);
                    unchanged
                } else {
                    last_shell_observation = None;
                    !observations.insert(key)
                };
                if value["arguments_rejected"] != true && unchanged {
                    unchanged_results += 1;
                    // Every streak of three gets a warning, never a stop: a re-read after a
                    // checkpoint can be needed, and stopping hides why the agent repeats itself.
                    // The run's turn limit bounds a real loop.
                    let warn = unchanged_results >= 3;
                    // An identical result still in context is referenced, not resent.
                    let live =
                        messages
                            .iter()
                            .rev()
                            .find_map(|message| match &message.message_type {
                                MessageType::ToolResult(earlier) => {
                                    earlier.iter().find(|earlier| {
                                        earlier.function.name == result.function.name
                                            && earlier.function.arguments
                                                == result.function.arguments
                                    })
                                }
                                _ => None,
                            });
                    if nudge_off("repeat_receipt") {
                        // Eval comparison: the repeated result goes back in full.
                    } else if let Some(earlier) = live {
                        *result = freshness_receipt(result, &earlier.id, fingerprint);
                    } else {
                        remind_unchanged_result(
                            result,
                            fingerprint,
                            tool_result_allowance(
                                policy.context_token_budget.saturating_mul(4) / 8,
                                calls.len(),
                            ),
                        )?;
                    }
                    if warn {
                        unchanged_results = 0;
                        if !nudge_off("loop_warning") {
                            warn_repeated_results(result);
                        }
                    }
                } else {
                    unchanged_results = 0;
                }
            } else {
                unchanged_results = 0;
                observations.clear();
                last_shell_observation = None;
            }
            // Counted for diagnostics only. Like repeated results, many searches or archive
            // reads never stop a run: stopping hides why the agent kept looking, and the run's
            // turn limit bounds a real loop.
            if call.function.name == "recall" {
                crate::context_metrics::activity("recall_searches");
            }
            if crate::tools::context_recovery_call(&call.function.name, &call.function.arguments) {
                crate::context_metrics::activity("recovery_reads");
            }
        }
        let mut delivered_calls = calls;
        let argument_allowance =
            tool_argument_allowance(policy.context_token_budget.saturating_mul(4) / 8);
        for (call, result) in delivered_calls.iter_mut().zip(&results) {
            // Rejection is a runtime decision; arbitrary tool output cannot assert it.
            if serde_json::to_vec(call)?.len() > argument_allowance {
                let value: serde_json::Value = serde_json::from_str(&result.function.arguments)?;
                call.function.arguments =
                    serde_json::json!({"arguments_rejected":true,"source":value["original_call"]})
                        .to_string();
            }
        }
        changed_files |= delivered_calls.iter().zip(&results).any(|(call, result)| {
            FILE_CHANGES.contains(&call.function.name.as_str())
                && serde_json::from_str::<serde_json::Value>(&result.function.arguments)
                    .is_ok_and(|value| value["success"] == true)
        });
        // Screenshots the tool kept for this model join the next requests (see `vision`).
        for result in &results {
            if result.function.name != crate::tools::BROWSER_TOOL {
                continue;
            }
            let Some(path) = crate::vision::attached_path(&result.function.arguments) else {
                continue;
            };
            if let Some(image) = crate::vision::copy_of(&path) {
                let note = crate::prompts::fill(
                    crate::prompts::RUNTIME_SCREENSHOT_IMAGE,
                    &[("path", &path)],
                );
                recent_images.push(&result.id, note, image);
            }
        }
        messages.push(ChatMessage {
            role: ChatRole::Assistant,
            message_type: MessageType::ToolUse(delivered_calls),
            content: text.unwrap_or_default(),
        });
        messages.push(ChatMessage {
            role: ChatRole::Tool,
            message_type: MessageType::ToolResult(results),
            content: String::new(),
        });
        match crate::plugins::hooks::post_tool_batch(&batch, &events).await {
            Ok(Some(context)) => messages.push(ChatMessage {
                role: ChatRole::User,
                message_type: MessageType::Text,
                content: hook_context_note(&context),
            }),
            Ok(None) => {}
            Err(error) => {
                let error = error.to_string();
                emit(RunEvent::Failed {
                    error: error.clone(),
                })
                .await
                .ok();
                return Err(anyhow!(error));
            }
        }
        if let Some(result) = take_end_request() {
            emit(RunEvent::Finished {
                result: result.clone(),
            })
            .await
            .ok();
            return Ok(result);
        }
    }

    check_stopped(&stopped, &events).await?;
    // The ratio may have risen on the last turn; compaction compares raw estimates.
    context_policy.context_token_budget = calibration.raw_budget(input_budget, schema_tokens);
    // The last tool batch is complete. Save a handoff before the hard stop.
    if policy.context_token_budget != usize::MAX
        && calibrated_tokens(&calibration, &messages, schema_tokens) > input_budget
    {
        crate::plugins::hooks::emit_current(
            "BeforeCompaction",
            serde_json::json!({"event":"BeforeCompaction"}),
            &events,
        )
        .await?;
        if let Err(error) = compact_context(
            &llm,
            &mut messages,
            &context_policy,
            &events,
            &stopped,
            evidence_directory.as_deref(),
        )
        .await
        {
            if stopped.load(Ordering::SeqCst) {
                return Err(error);
            }
            emit(RunEvent::Failed {
                error: format!("Context checkpoint failed: {error}"),
            })
            .await
            .ok();
            return Err(error);
        }
    }
    if policy.context_token_budget != usize::MAX {
        let prompt = crate::prompts::RUNTIME_HANDOFF;
        let mut handoff = messages.clone();
        handoff.push(ChatMessage {
            role: ChatRole::User,
            message_type: MessageType::Text,
            content: prompt.to_owned(),
        });
        let details = if calibration.messages(estimated_tokens(&handoff), schema_tokens)
            <= policy.context_token_budget.saturating_sub(answer_reserve)
        {
            llm.chat(&handoff, None)
                .await
                .ok()
                .and_then(|answer| answer.text())
        } else {
            None
        };
        let result = details
            .filter(|text| !text.trim().is_empty())
            .unwrap_or_else(|| {
                "I couldn't produce a final summary. Please review the recorded changes and choose the next step."
                    .to_owned()
            });
        emit(RunEvent::Incomplete {
            result: result.clone(),
        })
        .await
        .ok();
        return Ok(result);
    }
    let error = format!(
        "max turns overall ({}) reached without a final answer",
        policy.total_turns
    );
    emit(RunEvent::Failed {
        error: error.clone(),
    })
    .await
    .ok();
    Err(anyhow!(error))
}

// Model requests are safe to cancel: no tool execution is in progress here.
// Atomic stop flags are shared with desktop/CLI; poll while awaiting I/O too.
async fn until_stopped<T>(
    future: impl std::future::Future<Output = T>,
    stopped: &AtomicBool,
    events: &tokio::sync::mpsc::Sender<RunEvent>,
) -> anyhow::Result<T> {
    tokio::select! {
        biased;
        _ = async {
            while !stopped.load(Ordering::SeqCst) {
                tokio::time::sleep(std::time::Duration::from_millis(50)).await;
            }
        } => {
            check_stopped(stopped, events).await?;
            unreachable!("stop flag remains set for this run")
        }
        result = future => Ok(result),
    }
}

/// Sent once when a run that changed files gives its final answer.
const SPEC_CHECK: &str = crate::prompts::RUNTIME_SPEC_CHECK;
/// Tools whose success changes project files (the shell's changes are not counted: most shell
/// calls run or inspect, and edits go through these).
const FILE_CHANGES: [&str; 2] = ["write_file", "apply_patch"];

const CUT_OFF_NOTE: &str = crate::prompts::RUNTIME_CUT_OFF;
const SUBAGENT_REACTION_NOTE: &str = crate::prompts::RUNTIME_SUBAGENT_REACTION;
/// The main agent's tools for working with subagents.
fn is_subagent_tool(name: &str) -> bool {
    matches!(
        name,
        "spawn_subagents" | "wait_subagents" | "message_subagent"
    )
}

/// The `say` line of a reply's subagent calls, for a reply with no text of its own.
fn said_with_calls(calls: &[ToolCall]) -> Option<String> {
    calls
        .iter()
        .filter(|call| is_subagent_tool(&call.function.name))
        .find_map(|call| {
            let args: serde_json::Value = serde_json::from_str(&call.function.arguments).ok()?;
            let line = args["say"].as_str()?.trim();
            (!line.is_empty()).then(|| line.chars().take(800).collect())
        })
}

const SILENT_CUT_OFF_NOTE: &str = crate::prompts::RUNTIME_SILENT_CUT_OFF;

/// The answer text after `shown`, the text of earlier cut-off replies, as the user saw it.
fn with_shown(shown: &str, text: Option<String>) -> Option<String> {
    match (shown.is_empty(), text) {
        (true, text) => text,
        (false, None) => Some(shown.to_owned()),
        (false, Some(text)) => Some(format!("{shown}\n\n{text}")),
    }
}

const EMPTY_ANSWER_NOTE: &str = crate::prompts::RUNTIME_EMPTY_ANSWER;

const TURN_SUMMARY: &str = crate::prompts::RUNTIME_TURN_SUMMARY;
const TURN_END_GUARD: &str = crate::prompts::RUNTIME_TURN_END_GUARD;

/// `earlier` and `later` as one answer, a blank line between; either may be empty. When
/// one already holds the other (a model repeating itself), the fuller one is the answer.
fn joined(earlier: &str, later: &str) -> String {
    let (before, after) = (earlier.trim(), later.trim());
    if before.is_empty() || after.contains(before) {
        later.to_owned()
    } else if after.is_empty() || before.contains(after) {
        earlier.to_owned()
    } else {
        format!("{}\n\n{}", earlier.trim_end(), later.trim_start())
    }
}

/// Longest final answer still read as a progress line rather than a summary, in characters.
const PROGRESS_LINE_CHARS: usize = 120;

/// Words that say the work is still going ("continuing", "moving on", "next"): the
/// progress lines agents stopped on (stuck-goal replays, 2026-10-28) all had one. A short
/// finished answer ("Done.", "Verified: a.txt holds 1.") has none.
const STILL_GOING: [&str; 14] = [
    "continu",
    "moving on",
    "move on",
    "next",
    "nearing",
    "pulling",
    "proceed",
    "let me",
    "i'll",
    "i will",
    "going to",
    "starting",
    "working on",
    "now checking",
];

/// Whether a final answer is only a short progress line: one line, under
/// [`PROGRESS_LINE_CHARS`], that says the work is still going. A longer, multi-line or
/// finished answer is the turn's own summary.
fn progress_line(text: &str) -> bool {
    let text = text.trim();
    let lower = text.to_lowercase().replace('\u{2019}', "'");
    !text.contains('\n')
        && text.chars().count() < PROGRESS_LINE_CHARS
        && STILL_GOING.iter().any(|word| lower.contains(word))
}

fn evidence_relative_path(path: &std::path::Path) -> std::path::PathBuf {
    let components = path.components().collect::<Vec<_>>();
    components
        .iter()
        .rposition(|part| part.as_os_str() == ".themis")
        .map_or_else(
            || path.to_owned(),
            |index| components[index..].iter().collect(),
        )
}

tokio::task_local! { pub static MODEL_CONTEXT_CAPACITY: Option<usize>; }
tokio::task_local! { static SHORT_REPLY_TURN: bool; }
// The reply the turn-end guard sent back, while the model answers it.
tokio::task_local! { static SENT_BACK_REPLY: Option<String>; }
tokio::task_local! { static STEER_INBOX: SteerInbox; }
tokio::task_local! { static SYSTEM_HEAD: String; }
tokio::task_local! { static RUN_NOTES: Arc<RunNotes>; }

tokio::task_local! { static CHECKPOINT_NOTE: CheckpointNote; }
tokio::task_local! { static MEMORY_BLOCK: String; }

tokio::task_local! { static FIRST_REQUEST: String; }

tokio::task_local! { static END_REQUEST: EndRequest; }

#[cfg(feature = "eval")]
tokio::task_local! {
    /// Eval-only ablation: runtime nudges switched off (`loop_warning`,
    /// `repeat_receipt`, `spec_check`, `steers_check`, `turn_summary`), to measure whether each one earns
    /// its place.
    pub static EVAL_NUDGES_OFF: Arc<Vec<String>>;
}

/// How a run stopped by the user's Stop begins its failure text; callers
/// (the goal supervisor among them) recognize a stop by it.
pub const STOPPED_BY_YOU: &str = "Stopped by you";

/// Longest new user message, in characters, that can count as a short conversational reply.
pub const SHORT_REPLY_MAX_CHARS: usize = 200;

/// Whether a new user message is a short greeting, thanks or farewell with no task in it, so a
/// checkpoint can wait for the next substantive turn. Short task messages ("fix the auth bug",
/// "run the tests"), bare acknowledgements and anything with attachments are not.
pub fn is_short_reply(text: &str, has_attachments: bool) -> bool {
    let text = text.trim();
    !has_attachments
        && !text.is_empty()
        && text.chars().count() <= SHORT_REPLY_MAX_CHARS
        && crate::social::is_social_request(text)
}

tokio::task_local! { static MODEL_REQUEST_BUDGET: usize; }

tokio::task_local! { pub(super) static SUMMARY_DEADLINE: tokio::time::Instant; }
tokio::task_local! { pub(super) static SUMMARY_STAGE_DEADLINE: tokio::time::Instant; }

tokio::task_local! { static COMPACTION_CONFIG: crate::configuration::CompactionConfig; }

fn pasted_secret_note() -> ChatMessage {
    ChatMessage {
        role: ChatRole::User,
        message_type: MessageType::Text,
        content: crate::prompts::RUNTIME_PASTED_SECRET.to_owned(),
    }
}

/// Opens with the shared system instructions and verification habits; the evidence
/// pointer and skill catalog follow.
fn initial_messages(history: Vec<ConversationTurn>, task: String) -> Vec<ChatMessage> {
    let head = SYSTEM_HEAD.try_with(Clone::clone).unwrap_or_else(|_| {
        format!(
            "{}\n{}",
            crate::prompts::SYSTEM_INSTRUCTIONS,
            crate::prompts::SYSTEM_VERIFY
        )
    });
    let mut messages = vec![ChatMessage {
        role: ChatRole::System,
        message_type: MessageType::Text,
        content: head,
    }];
    for turn in history {
        if turn.role == ConversationRole::Checkpoint {
            let (summary, recent) = checkpoint_parts(&turn.text);
            // Checkpoints saved while the notes tool existed carry its block; drop it.
            let summary = notes::strip_blocks(summary);
            messages.push(ChatMessage {
                role: ChatRole::Assistant,
                message_type: MessageType::Text,
                content: format!("Earlier context checkpoint (historical; later user requests take precedence; mention or resume this work only when the user's message continues or asks about it):\n{summary}"),
            });
            messages.extend(recent);
        } else {
            messages.push(ChatMessage {
                role: if turn.role == ConversationRole::User {
                    ChatRole::User
                } else {
                    ChatRole::Assistant
                },
                message_type: MessageType::Text,
                content: turn.text,
            });
        }
    }
    // On the device, and only the fixed note goes out: the warning shouldn't rest on the
    // model noticing. A subagent's brief comes from the main agent, not a person pasting.
    let pasted =
        SYSTEM_HEAD.try_with(|_| ()).is_err() && crate::secret_look::looks_like_secret(&task);
    messages.push(ChatMessage {
        role: ChatRole::User,
        message_type: MessageType::Text,
        content: task,
    });
    if pasted {
        messages.push(pasted_secret_note());
    }
    if let Some(context) = crate::plugins::hooks::prompt_context() {
        messages.push(ChatMessage {
            role: ChatRole::User,
            message_type: MessageType::Text,
            content: hook_context_note(&context),
        });
    }
    messages
}

/// The model-facing form of context a `PromptSubmit` hook added: the label, then the
/// context fenced so it cannot pose as the label or close the fence early.
pub fn hook_context_note(context: &str) -> String {
    let fenced = context.replace("<<<", "< < <").replace(">>>", "> > >");
    format!(
        "{}\n<<<hook-context\n{fenced}\n>>>",
        crate::prompts::HOOK_CONTEXT_NOTE
    )
}

async fn check_stopped(
    stopped: &AtomicBool,
    events: &tokio::sync::mpsc::Sender<RunEvent>,
) -> anyhow::Result<()> {
    if stopped.load(Ordering::SeqCst) {
        let error = format!("{STOPPED_BY_YOU}. Completed actions remain available for review.");
        let _ = events
            .send(RunEvent::Failed {
                error: error.clone(),
            })
            .await;
        return Err(anyhow!(error));
    }
    Ok(())
}

/// Runs one agent task with skill bundles applied.
///
/// Filters `tools` to the intersection of every skill's grants (see
/// [`filter_tools`]), appends each skill's instructions to the task (see
/// [`compose_task`]), then delegates to [`run_task`]. With empty `skills` this
/// is exactly [`run_task`]: same tools, same prompt, same behavior.
pub async fn run_task_skilled(
    llm: Arc<dyn LLMProvider>,
    tools: Vec<Box<dyn ToolT>>,
    task: String,
    approvals: Arc<dyn ApprovalHook>,
    max_turns: usize,
    events: tokio::sync::mpsc::Sender<RunEvent>,
    skills: &[Skill],
) -> anyhow::Result<String> {
    let tools = filter_tools(tools, skills);
    let task = compose_task(&task, skills);
    run_task(llm, tools, task, approvals, max_turns, events).await
}

/// Entry points for the `fuzz/` cargo-fuzz targets; not a stable API.
#[cfg(feature = "fuzzing")]
#[doc(hidden)]
pub mod fuzzing {
    use super::*;

    /// Social-message detection and recall terms for arbitrary UI text.
    pub fn social_request(text: &str) -> (bool, Vec<String>) {
        (
            crate::social::is_social_request(text),
            crate::social::content_terms(text),
        )
    }

    /// Restores `history` as an earlier assistant turn (possibly a checkpoint
    /// with ledger blocks), adds one tool call returning `output`, and renders
    /// the ledger within `total` bytes.
    pub fn restore_ledger(
        history: &str,
        tool: &str,
        arguments: &str,
        output: &str,
        total: usize,
    ) -> String {
        let mut messages = initial_messages(
            vec![ConversationTurn {
                role: ConversationRole::Assistant,
                text: history.to_owned(),
            }],
            "Continue".to_owned(),
        );
        let call = |arguments: &str| ToolCall {
            id: "fuzz".into(),
            call_type: "function".into(),
            function: FunctionCall {
                name: tool.into(),
                arguments: arguments.into(),
            },
        };
        messages.push(ChatMessage {
            role: ChatRole::Assistant,
            message_type: MessageType::ToolUse(vec![call(arguments)]),
            content: String::new(),
        });
        messages.push(ChatMessage {
            role: ChatRole::Tool,
            message_type: MessageType::ToolResult(vec![call(output)]),
            content: String::new(),
        });
        ledger::render(
            &messages,
            messages.len(),
            ledger::Budget {
                total,
                actions: total / 2,
            },
            None,
        )
    }

    /// Characters kept by the summary truncation helper.
    pub fn char_prefix(input: &str, limit: usize) -> (String, bool) {
        take_char_prefix(input, limit)
    }

    /// The ledger's framing markers.
    pub const LEDGER_MARKERS: (&str, &str) = (ledger::START, ledger::END);
}

mod answer;
mod budget;
mod checkpoint;
mod compaction;
mod scope;
mod tool_calls;
use answer::*;
pub use budget::with_model_context_capacity;
use budget::*;
pub use checkpoint::checkpoint_parts;
use checkpoint::*;
pub use compaction::subagent_note;
use compaction::*;
pub(crate) use scope::compaction_config;
use scope::*;
pub use scope::{
    steered_message, with_checkpoint_note, with_configuration, with_end_request,
    with_first_request, with_memory_block, with_run_notes, with_short_reply_turn, with_steer_inbox,
    with_system_head, CheckpointNote, EndRequest, RunNotes, SteerInbox, MEMORY_NOTE_MARKER,
};
use tool_calls::*;

#[cfg(test)]
mod tests;

#[cfg(test)]
mod checkpoint_section_tests {
    use super::validate_checkpoint;

    #[test]
    fn accepts_three_and_four_section_checkpoints() {
        let three = "Goal/constraints: g. Unresolved work: u. Next action: n.";
        let four = "Goal/constraints: g.\nUnresolved work: u.\nParked work: trip bug, fix in parser.rs.\nNext action: if the user continues this task, run tests.";
        assert!(validate_checkpoint(three).is_ok());
        assert!(validate_checkpoint(four).is_ok());
        // Older five-section checkpoints still contain the three headings in order.
        let five = "Goal/constraints: g. Confirmed findings: f. Completed changes/tests: c. Unresolved work: u. Next action: n.";
        assert!(validate_checkpoint(five).is_ok());
    }

    #[test]
    fn four_section_form_requires_order_and_content() {
        assert!(validate_checkpoint(
            "Goal/constraints: g.\nUnresolved work: u.\nParked work: .\nNext action: n."
        )
        .is_err());
        assert!(validate_checkpoint(
            "Goal/constraints: g.\nParked work: p.\nUnresolved work: u.\nNext action: n."
        )
        .is_err());
        assert!(validate_checkpoint("Goal/constraints: g.\nParked work: p.").is_err());
        // The phrase inside prose is not a heading: the three-section form applies.
        assert!(validate_checkpoint(
            "Goal/constraints: keep the parked work: trip. Unresolved work: u. Next action: n."
        )
        .is_ok());
    }
}

#[cfg(test)]
mod split_tests {
    use super::*;
    use proptest::prelude::*;

    fn naive_split(messages: &[ChatMessage], policy: &RunPolicy) -> usize {
        let recent_budget = policy
            .context_token_budget
            .saturating_sub(estimated_tokens(&messages[..1]))
            / 3;
        let mut split = messages.len().saturating_sub(policy.recent_messages.max(4));
        split = split.max(2);
        while split < messages.len() && estimated_tokens(&messages[split..]) > recent_budget {
            split += 1;
        }
        if split < messages.len() && matches!(messages[split].role, ChatRole::Tool) {
            split += 1;
        }
        split
    }

    proptest! {
        #[test]
        fn linear_split_matches_the_quadratic_definition(
            sizes in proptest::collection::vec(0usize..600, 1..40),
            budget in 0usize..6000,
            recent in 0usize..12,
        ) {
            let messages: Vec<ChatMessage> = sizes
                .iter()
                .enumerate()
                .map(|(index, size)| ChatMessage {
                    role: if index == 0 { ChatRole::System } else if index % 3 == 0 { ChatRole::Tool } else { ChatRole::User },
                    message_type: MessageType::Text,
                    content: "x".repeat(*size),
                })
                .collect();
            let policy = RunPolicy { total_turns: 1, context_token_budget: budget, recent_messages: recent };
            prop_assert_eq!(context_split(&messages, &policy), naive_split(&messages, &policy));
        }
    }
}
