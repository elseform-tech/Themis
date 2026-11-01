//! Model-facing prompt text.
//!
//! Every prompt lives in `crates/core/builtins/prompts/<name>.md` and is
//! compiled in with `include_str!`, so editing a file takes effect on the next
//! build. Files are stored without a trailing newline; the bytes in the file
//! are the bytes sent to the model. Templates use `{{snake_case}}`
//! placeholders filled with [`fill`]. Code that assembles a prompt from
//! several pieces (separators, optional parts) keeps that assembly and takes
//! only the text from here. See `docs/prompts.md`.

// System message of every ordinary run.
/// Shared instructions that open the system message of every ordinary run.
pub const SYSTEM_INSTRUCTIONS: &str = include_str!("../builtins/prompts/system-instructions.md");
/// Verification habits, after [`SYSTEM_INSTRUCTIONS`].
pub const SYSTEM_VERIFY: &str = include_str!("../builtins/prompts/system-verify.md");
/// Task-ledger sentence appended to the system message when originals are saved.
pub const SYSTEM_TASK_LEDGER: &str = include_str!("../builtins/prompts/system-task-ledger.md");
/// Evidence pointer when the `recall` tool is offered; `{{directory}}`, `{{caution}}`.
pub const SYSTEM_EVIDENCE_RECALL: &str =
    include_str!("../builtins/prompts/system-evidence-recall.md");
/// Evidence pointer when only file reads reach the originals; `{{directory}}`, `{{caution}}`.
pub const SYSTEM_EVIDENCE_FILES: &str =
    include_str!("../builtins/prompts/system-evidence-files.md");
/// Staleness and tool-restriction caution closing both evidence pointers.
pub const SYSTEM_EVIDENCE_CAUTION: &str =
    include_str!("../builtins/prompts/system-evidence-caution.md");
/// Opens the instructions of a skill imported from a Claude plugin command.
pub const SKILL_COMMAND_NOTE: &str = include_str!("../builtins/prompts/skill-command-note.md");
pub const SKILL_OUTPUT_STYLE_NOTE: &str =
    include_str!("../builtins/prompts/skill-output-style-note.md");
/// Skill catalog appended to the system message; `{{skills}}` is the metadata JSON.
pub const SYSTEM_SKILL_CATALOG: &str = include_str!("../builtins/prompts/system-skill-catalog.md");

// Subagents.
/// System message head of a subagent run, instead of the ordinary system instructions.
pub const SUBAGENT_SYSTEM: &str = include_str!("../builtins/prompts/subagent-system.md");
/// Added to a read-only subagent's system message.
pub const SUBAGENT_WRITE_PATHS_NOTE: &str =
    include_str!("../builtins/prompts/subagent-write-paths-note.md");
pub const SUBAGENT_MCP_NOTE: &str = include_str!("../builtins/prompts/subagent-mcp-note.md");
pub const SUBAGENT_MCP_LEFT_OUT: &str =
    include_str!("../builtins/prompts/subagent-mcp-left-out.md");
pub const SPAWN_SUBAGENTS_MCP: &str = include_str!("../builtins/prompts/spawn-subagents-mcp.md");
pub const SUBAGENT_WEB_NOTE: &str = include_str!("../builtins/prompts/subagent-web-note.md");
/// Tells a subagent whose profile names `Agent` that it can start helpers.
pub const SUBAGENT_NESTED_NOTE: &str = include_str!("../builtins/prompts/subagent-nested-note.md");
pub const SUBAGENT_SHELL_SCOPE_NOTE: &str =
    include_str!("../builtins/prompts/subagent-shell-scope-note.md");
pub const SUBAGENT_ASK_NOTE: &str = include_str!("../builtins/prompts/subagent-ask-note.md");
pub const SUBAGENT_BACKGROUND_NOTE: &str =
    include_str!("../builtins/prompts/subagent-background-note.md");
pub const SUBAGENT_CLAUDE_NAMES: &str =
    include_str!("../builtins/prompts/subagent-claude-names.md");
pub const SUBAGENT_TODO_TOOL: &str = include_str!("../builtins/prompts/subagent-todo-tool.md");
pub const SPAWN_SUBAGENTS_WEB: &str = include_str!("../builtins/prompts/spawn-subagents-web.md");
pub const SUBAGENT_READ_ONLY_NOTE: &str =
    include_str!("../builtins/prompts/subagent-read-only-note.md");
/// Explains the optional `status` field every subagent tool call accepts.
pub const SUBAGENT_STATUS_FIELD: &str =
    include_str!("../builtins/prompts/subagent-status-field.md");
/// Description of the `spawn_subagents` tool.
pub const SPAWN_SUBAGENTS_TOOL: &str = include_str!("../builtins/prompts/spawn-subagents-tool.md");
/// Appended to the spawn tool when `subagents.models` lists models the agent may pick.
pub const SPAWN_SUBAGENTS_MODELS: &str =
    include_str!("../builtins/prompts/spawn-subagents-models.md");
/// Appended to the spawn tool when enabled plugins ship subagent profiles; `{{profiles}}`.
pub const SPAWN_SUBAGENTS_PROFILES: &str =
    include_str!("../builtins/prompts/spawn-subagents-profiles.md");
/// Follows the profile list when it was cut to its budget; `{{count}}`.
pub const SPAWN_SUBAGENTS_PROFILES_MORE: &str =
    include_str!("../builtins/prompts/spawn-subagents-profiles-more.md");
/// A plugin profile's body in a subagent's system message; `{{name}}`, `{{plugin}}`,
/// `{{instructions}}`.
pub const SUBAGENT_PROFILE: &str = include_str!("../builtins/prompts/subagent-profile.md");
/// Description of the `wait_subagents` tool.
pub const WAIT_SUBAGENTS_TOOL: &str = include_str!("../builtins/prompts/wait-subagents-tool.md");
/// Description of the `message_subagent` tool.
pub const MESSAGE_SUBAGENT_TOOL: &str =
    include_str!("../builtins/prompts/message-subagent-tool.md");
/// Description of the `stop_subagent` tool.
pub const STOP_SUBAGENT_TOOL: &str = include_str!("../builtins/prompts/stop-subagent-tool.md");
/// A subagent's habit with secret files, in its system message.
pub const SUBAGENT_SECRETS_NOTE: &str =
    include_str!("../builtins/prompts/subagent-secrets-note.md");
/// A subagent's result, delivered to the main agent; `{{name}}`, `{{id}}`, `{{status}}`, `{{result}}`.
pub const SUBAGENT_RESULT_NOTE: &str = include_str!("../builtins/prompts/subagent-result-note.md");

// Desktop run setup.
/// Current run workspace preamble before the skill catalog; `{{workspace}}`.
pub const DESKTOP_RUN_WORKSPACE: &str =
    include_str!("../builtins/prompts/desktop-run-workspace.md");
/// System prompt note: secret-looking files Git does not ignore; `{{files}}`.
pub const DESKTOP_SECRETS_UNIGNORED: &str =
    include_str!("../builtins/prompts/desktop-secrets-unignored.md");
/// System prompt note: secret-looking files Git already tracks; `{{files}}`.
pub const DESKTOP_SECRETS_TRACKED: &str =
    include_str!("../builtins/prompts/desktop-secrets-tracked.md");
/// Added to the system prompt of YOLO runs only: no approvals or sandbox, so caution.
pub const DESKTOP_RUN_YOLO: &str = include_str!("../builtins/prompts/desktop-run-yolo.md");
/// Browser verification habit, for runs with the built-in browser.
pub const DESKTOP_VERIFY_BROWSER: &str =
    include_str!("../builtins/prompts/desktop-verify-browser.md");
/// Label before an MCP server's own instructions in the system message.
pub const DESKTOP_MCP_GUIDANCE: &str = include_str!("../builtins/prompts/desktop-mcp-guidance.md");
/// Integration status for an MCP connection that failed; `{{plugin}}`, `{{name}}`, `{{reason}}`.
pub const DESKTOP_MCP_UNAVAILABLE: &str =
    include_str!("../builtins/prompts/desktop-mcp-unavailable.md");
/// Heading before the configured repository guidance in the task message.
pub const DESKTOP_REPOSITORY_GUIDANCE: &str =
    include_str!("../builtins/prompts/desktop-repository-guidance.md");
/// Heading of an attached text file sent in full; `{{path}}`.
pub const DESKTOP_ATTACHED_FILE: &str =
    include_str!("../builtins/prompts/desktop-attached-file.md");
/// Heading of an attached file sent as a map only; `{{path}}`.
pub const DESKTOP_ATTACHED_MAP: &str = include_str!("../builtins/prompts/desktop-attached-map.md");
/// Map of a large attachment; `{{bytes}}`, `{{lines}}`, `{{headings}}`, `{{previews}}`.
pub const DESKTOP_ATTACHMENT_MAP: &str =
    include_str!("../builtins/prompts/desktop-attachment-map.md");
/// How an agent watch ends, added to each watch run's message.
pub const DESKTOP_WATCH_COMPLETE: &str =
    include_str!("../builtins/prompts/desktop-watch-complete.md");
/// A watch's changed check output; `{{command}}`, `{{exit}}`, `{{output}}`.
pub const DESKTOP_WATCH_CHECK: &str = include_str!("../builtins/prompts/desktop-watch-check.md");

// Goal mode.
/// System message of the goal check that judges each goal turn.
pub const GOAL_CHECK_SYSTEM: &str = include_str!("../builtins/prompts/goal-check-system.md");
/// User message of the goal check; `{{goal}}`, `{{answer}}`, `{{evidence}}` are fenced data.
pub const GOAL_CHECK_USER: &str = include_str!("../builtins/prompts/goal-check-user.md");
/// Appended to the goal check's input when the user added requests while the goal ran.
pub const GOAL_CHECK_ADDED: &str = include_str!("../builtins/prompts/goal-check-added.md");
/// Active-goal context appended to every run in a chat with a running goal; `{{goal}}`.
pub const GOAL_CONTEXT: &str = include_str!("../builtins/prompts/goal-context.md");
/// Message that starts a goal turn; `{{goal_turn_marker}}`, `{{turn}}`, `{{goal}}`, `{{note}}`.
pub const GOAL_TURN: &str = include_str!("../builtins/prompts/goal-turn.md");
pub const GOAL_TURN_PROOF: &str = include_str!("../builtins/prompts/goal-turn-proof.md");
pub const GOAL_TURN_FILES: &str = include_str!("../builtins/prompts/goal-turn-files.md");
pub const GOAL_EVIDENCE_SCRIPT: &str = include_str!("../builtins/prompts/goal-evidence-script.md");
pub const GOAL_NOTE_SCRIPT_PROOF: &str =
    include_str!("../builtins/prompts/goal-note-script-proof.md");
pub const GOAL_CHECKLIST_SYSTEM: &str =
    include_str!("../builtins/prompts/goal-checklist-system.md");
pub const GOAL_CHECKLIST_USER: &str = include_str!("../builtins/prompts/goal-checklist-user.md");
pub const GOAL_CHECK_MIDTURN: &str = include_str!("../builtins/prompts/goal-check-midturn.md");
pub const GOAL_CONTEXT_OVER: &str = include_str!("../builtins/prompts/goal-context-over.md");
pub const GOAL_CONTEXT_PAUSED: &str = include_str!("../builtins/prompts/goal-context-paused.md");
pub const GOAL_CHECK_PROOF: &str = include_str!("../builtins/prompts/goal-check-proof.md");
/// Message that tells the user a checked goal is reached; `{{goal_turn_marker}}`,
/// `{{turn}}`, `{{goal}}`, `{{reason}}`.
pub const GOAL_REACHED_TURN: &str = include_str!("../builtins/prompts/goal-reached-turn.md");
pub const GOAL_REACHED_OPEN_QUESTIONS: &str =
    include_str!("../builtins/prompts/goal-reached-open-questions.md");
/// Goal-turn note after turns without progress; `{{turns}}`.
pub const GOAL_NOTE_STALLED: &str = include_str!("../builtins/prompts/goal-note-stalled.md");
/// Goal-turn note after the user resumes a goal saved as stuck (older versions).
pub const GOAL_NOTE_RESUMED_STUCK: &str =
    include_str!("../builtins/prompts/goal-note-resumed-stuck.md");
/// Goal-turn note after the user stopped the previous turn.
pub const GOAL_NOTE_STOPPED: &str = include_str!("../builtins/prompts/goal-note-stopped.md");
/// Goal-turn note after the previous turn failed; `{{error}}`.
pub const GOAL_NOTE_FAILED: &str = include_str!("../builtins/prompts/goal-note-failed.md");
/// Goal-turn note after Themis closed during a turn.
pub const GOAL_NOTE_INTERRUPTED: &str =
    include_str!("../builtins/prompts/goal-note-interrupted.md");
/// Goal-turn note listing the questions still open; `{{questions}}`.
pub const GOAL_OPEN_QUESTIONS: &str = include_str!("../builtins/prompts/goal-open-questions.md");

// Questions to the user.
/// Description of the `ask_user` tool.
pub const ASK_USER_DESCRIPTION: &str = include_str!("../builtins/prompts/ask-user-description.md");
/// `note` of an `ask_user` result that keeps the run going; `{{assumption}}`.
pub const ASK_USER_ASKED: &str = include_str!("../builtins/prompts/ask-user-asked.md");
/// `note` of a blocking `ask_user` result: the run ends.
pub const ASK_USER_BLOCKING: &str = include_str!("../builtins/prompts/ask-user-blocking.md");
/// The answer of a run that ended on a blocking question.
pub const ASK_USER_WAITING: &str = include_str!("../builtins/prompts/ask-user-waiting.md");
/// `ask_user` refusal when too many questions are open; `{{max}}`.
pub const ASK_USER_LIMIT: &str = include_str!("../builtins/prompts/ask-user-limit.md");
/// The message that delivers the user's answer; `{{question}}`, `{{assumption}}`, `{{answer}}`.
pub const QUESTION_ANSWER: &str = include_str!("../builtins/prompts/question-answer.md");
/// `note` field of the set_goal tool result.
pub const GOAL_SET_NOTE: &str = include_str!("../builtins/prompts/goal-set-note.md");

// Summaries and compaction.
/// System prompt of every context checkpoint request.
pub const COMPACTION_CHECKPOINT: &str =
    include_str!("../builtins/prompts/compaction-checkpoint.md");
/// User message of a checkpoint request; `{{previous}}`, `{{label}}`, `{{notes}}`, `{{current_request}}`.
pub const SUMMARY_CHECKPOINT_REQUEST: &str =
    include_str!("../builtins/prompts/summary-checkpoint-request.md");
/// Section appended to a checkpoint request with the chat's first user message; `{{first_request}}`.
pub const SUMMARY_FIRST_REQUEST: &str =
    include_str!("../builtins/prompts/summary-first-request.md");
/// Heading before the user-configured `compaction.prompt` guidance.
pub const SUMMARY_CONFIGURED_GUIDANCE: &str =
    include_str!("../builtins/prompts/summary-configured-guidance.md");
/// Output budget appended to every summary request; `{{words}}`, `{{guidance}}`.
pub const SUMMARY_OUTPUT_BUDGET: &str =
    include_str!("../builtins/prompts/summary-output-budget.md");
/// Output format guidance for context checkpoints.
pub const SUMMARY_CONTEXT_OUTPUT: &str =
    include_str!("../builtins/prompts/summary-context-output.md");
/// Output format guidance for document summaries.
pub const SUMMARY_DOCUMENT_OUTPUT: &str =
    include_str!("../builtins/prompts/summary-document-output.md");
/// System prompt of an attached-document summary request.
pub const SUMMARY_DOCUMENT_SYSTEM: &str =
    include_str!("../builtins/prompts/summary-document-system.md");
/// User message of a document summary request; `{{current}}`, `{{label}}`, `{{notes}}`.
pub const SUMMARY_DOCUMENT_REQUEST: &str =
    include_str!("../builtins/prompts/summary-document-request.md");
/// Header of one staged summary's notes; `{{stage}}`, `{{stages}}`, `{{notes}}`.
pub const SUMMARY_SOURCE_STAGE: &str = include_str!("../builtins/prompts/summary-source-stage.md");
/// Coverage footer of a summary made in one request.
pub const SUMMARY_COVERAGE_SINGLE: &str =
    include_str!("../builtins/prompts/summary-coverage-single.md");
/// Coverage footer of a staged summary; `{{fragments}}`.
pub const SUMMARY_COVERAGE_STAGED: &str =
    include_str!("../builtins/prompts/summary-coverage-staged.md");
/// Prepared-input coverage line of a checkpoint request; `{{selected}}`, `{{omitted}}`, `{{skipped}}`, `{{completeness}}`.
pub const SUMMARY_COVERAGE: &str = include_str!("../builtins/prompts/summary-coverage.md");
/// Pointer to saved originals kept with a checkpoint; `{{directory}}`, `{{count}}`, `{{latest_snapshot}}`.
pub const SUMMARY_ARCHIVE_CONTEXT: &str =
    include_str!("../builtins/prompts/summary-archive-context.md");
/// Marker for the middle cut from a long relevance hint; `{{omitted_bytes}}`.
pub const SUMMARY_RELEVANCE_HINT_OMITTED: &str =
    include_str!("../builtins/prompts/summary-relevance-hint-omitted.md");
/// Record of a run suspended when its checkpoint failed; `{{source}}`, `{{task_excerpt}}`.
pub const SUMMARY_SUSPENDED_PROGRESS: &str =
    include_str!("../builtins/prompts/summary-suspended-progress.md");
/// Semantic capsule of an attached document; `{{identity}}`, `{{summary}}`.
pub const SUMMARY_DOCUMENT_CAPSULE: &str =
    include_str!("../builtins/prompts/summary-document-capsule.md");
/// Header of the original exchange anchors restored from the transcript.
pub const DESKTOP_EXCHANGE_ANCHORS: &str =
    include_str!("../builtins/prompts/desktop-exchange-anchors.md");

// Runtime notes and nudges.
/// Requirement check sent once when a run that changed files gives its final answer.
pub const RUNTIME_SPEC_CHECK: &str = include_str!("../builtins/prompts/runtime-spec-check.md");
/// Note after a reply was cut off at the output limit once some text was shown.
pub const RUNTIME_CUT_OFF: &str = include_str!("../builtins/prompts/runtime-cut-off.md");
/// Note after a reply was cut off at the output limit before anything was shown.
pub const RUNTIME_SILENT_CUT_OFF: &str =
    include_str!("../builtins/prompts/runtime-silent-cut-off.md");
/// Note after a reply with no text and no tool call.
pub const RUNTIME_EMPTY_ANSWER: &str = include_str!("../builtins/prompts/runtime-empty-answer.md");
/// Asked once when a run that used tools ends on a short progress line instead of a summary.
pub const RUNTIME_TURN_SUMMARY: &str = include_str!("../builtins/prompts/runtime-turn-summary.md");
pub const RUNTIME_TURN_END_GUARD: &str =
    include_str!("../builtins/prompts/runtime-turn-end-guard.md");
/// Warning added after three unchanged tool results in a row.
pub const RUNTIME_REPEATED_ACTION: &str =
    include_str!("../builtins/prompts/runtime-repeated-action.md");
pub const RUNTIME_LOOP_WARNING: &str = include_str!("../builtins/prompts/runtime-loop-warning.md");
/// Note on a result already returned earlier in the run.
pub const RUNTIME_REPEAT_NOTE: &str = include_str!("../builtins/prompts/runtime-repeat-note.md");
/// Note on a result identical to one still in context; `{{earlier_id}}`.
pub const RUNTIME_REPEAT_IN_CONTEXT: &str =
    include_str!("../builtins/prompts/runtime-repeat-in-context.md");
/// Request for a closing answer when a run reaches its turn limit.
pub const RUNTIME_HANDOFF: &str = include_str!("../builtins/prompts/runtime-handoff.md");
/// Header before a message the user sent while the agent was working.
pub const RUNTIME_SUBAGENT_REACTION: &str =
    include_str!("../builtins/prompts/runtime-subagent-reaction.md");
pub const RUNTIME_STEERED: &str = include_str!("../builtins/prompts/runtime-steered.md");
/// Header before a subagent's result delivered between steps: data, not the person's words.
pub const RUNTIME_SUBAGENT_NOTE: &str =
    include_str!("../builtins/prompts/runtime-subagent-note.md");
/// How the agent treats subagent results: claims it checks before relying on them, and says so.
pub const SUBAGENT_REVIEW: &str = include_str!("../builtins/prompts/subagent-review.md");
/// After a message from the person that looks like it holds a secret; never holds the value.
pub const RUNTIME_PASTED_SECRET: &str =
    include_str!("../builtins/prompts/runtime-pasted-secret.md");
/// Label before context a `PromptSubmit` hook added: external information, not instructions.
pub const HOOK_CONTEXT_NOTE: &str = include_str!("../builtins/prompts/hook-context-note.md");
pub const HOOK_BLOCKED: &str = include_str!("../builtins/prompts/hook-blocked.md");
pub const HOOK_BLOCK_REASON_NOTE: &str =
    include_str!("../builtins/prompts/hook-block-reason-note.md");
pub const HOOK_STOP_CONTINUE: &str = include_str!("../builtins/prompts/hook-stop-continue.md");
/// The message that starts a turn for a hook's report: nobody wrote it. `{{marker}}`.
pub const HOOK_WAKE_TURN: &str = include_str!("../builtins/prompts/hook-wake-turn.md");
/// What a background hook reported when it finished (Claude's `asyncRewake`): the plugin's words,
/// fenced; not the person's, not instructions. `{{hook}}`, `{{output}}`.
pub const HOOK_REWAKE_NOTE: &str = include_str!("../builtins/prompts/hook-rewake-note.md");
/// Sent once before a run that read two or more steered messages ends, listing them.
pub const RUNTIME_STEERS_CHECK: &str = include_str!("../builtins/prompts/runtime-steers-check.md");
/// Error for a call left out of a batch too large for the context; `{{calls}}`, `{{runnable}}`.
pub const RUNTIME_BATCH_NOT_RUN: &str =
    include_str!("../builtins/prompts/runtime-batch-not-run.md");
/// Error for tool arguments over the size limit; `{{argument_allowance}}`.
pub const RUNTIME_ARGUMENTS_TOO_LARGE: &str =
    include_str!("../builtins/prompts/runtime-arguments-too-large.md");
/// Heading before one repository instruction file in the task message; `{{reference}}`.
pub const RUNTIME_REPOSITORY_GUIDANCE_FILE: &str =
    include_str!("../builtins/prompts/runtime-repository-guidance-file.md");

// Tool-result notes.
/// Note on a receipt for a result offloaded to saved originals.
pub const TOOL_OFFLOADED_RECEIPT: &str =
    include_str!("../builtins/prompts/tool-offloaded-receipt.md");
/// Note on a receipt for a call whose arguments were rejected as too large.
pub const TOOL_ARGUMENTS_REJECTED_RECEIPT: &str =
    include_str!("../builtins/prompts/tool-arguments-rejected-receipt.md");
/// Note on a receipt for a result compacted away at a checkpoint.
pub const TOOL_COMPACTED_RECEIPT: &str =
    include_str!("../builtins/prompts/tool-compacted-receipt.md");
/// Warning on a bounded excerpt re-read from saved originals.
pub const TOOL_EXCERPT_WARNING: &str = include_str!("../builtins/prompts/tool-excerpt-warning.md");
/// Refusal guidance added to every denied tool call.
pub const TOOL_REFUSAL: &str = include_str!("../builtins/prompts/tool-refusal.md");
/// Note on a call denied because the person steered a new message in while it waited.
pub const TOOL_DENIED_NEW_MESSAGE: &str =
    include_str!("../builtins/prompts/tool-denied-new-message.md");
/// Note when a command's child process kept its output open.
pub const TOOL_SHELL_OUTPUT_HELD: &str =
    include_str!("../builtins/prompts/tool-shell-output-held.md");
/// Note when the sandbox blocked a command and running it outside was not approved; `{{reason}}`.
pub const TOOL_SANDBOX_BLOCKED_UNAPPROVED: &str =
    include_str!("../builtins/prompts/tool-sandbox-blocked-unapproved.md");
/// Note when the sandbox blocked a command without an approval prompt; `{{reason}}`.
pub const TOOL_SANDBOX_BLOCKED: &str = include_str!("../builtins/prompts/tool-sandbox-blocked.md");
/// Note when a failed command may have been blocked by the sandbox.
pub const TOOL_SANDBOX_NOTE: &str = include_str!("../builtins/prompts/tool-sandbox-note.md");
/// read_file warning on generated archive-recovery output.
pub const TOOL_READ_RECOVERY_WARNING: &str =
    include_str!("../builtins/prompts/tool-read-recovery-warning.md");
/// read_file warning on a verbatim historical exchange record.
pub const TOOL_READ_EXCHANGE_WARNING: &str =
    include_str!("../builtins/prompts/tool-read-exchange-warning.md");
/// read_file warning on an assistant navigation record.
pub const TOOL_READ_NAVIGATION_WARNING: &str =
    include_str!("../builtins/prompts/tool-read-navigation-warning.md");
/// Note on fetched web content and search results.
pub const TOOL_WEB_UNTRUSTED: &str = include_str!("../builtins/prompts/tool-web-untrusted.md");
/// Note when a fetched page redirects to another site.
pub const TOOL_WEB_REDIRECT: &str = include_str!("../builtins/prompts/tool-web-redirect.md");
/// Note when a web search found nothing and some engines failed.
pub const TOOL_WEB_NO_RESULTS: &str = include_str!("../builtins/prompts/tool-web-no-results.md");
/// Note on recall results.
pub const TOOL_RECALL_UNTRUSTED: &str =
    include_str!("../builtins/prompts/tool-recall-untrusted.md");
/// Note when recall's kinds/source filters matched nothing.
pub const TOOL_RECALL_FILTERS_IGNORED: &str =
    include_str!("../builtins/prompts/tool-recall-filters-ignored.md");
/// On the result of a command that staged or committed secret-looking files; `{{files}}`.
pub const TOOL_SENSITIVE_COMMIT: &str =
    include_str!("../builtins/prompts/tool-sensitive-commit.md");
/// On a refusal that involved secret-looking files; `{{files}}`.
pub const TOOL_SENSITIVE_DENIED: &str =
    include_str!("../builtins/prompts/tool-sensitive-denied.md");
/// Note on built-in browser page snapshots.
pub const TOOL_SCREENSHOT_NOTE: &str = include_str!("../builtins/prompts/tool-screenshot-note.md");
pub const TOOL_SHELL_ARGS_INVALID: &str =
    include_str!("../builtins/prompts/tool-shell-args-invalid.md");
pub const TOOL_BROWSER_UNTRUSTED: &str =
    include_str!("../builtins/prompts/tool-browser-untrusted.md");
/// Note when DevTools paused the page in the debugger.
pub const TOOL_DEVTOOLS_PAUSED: &str = include_str!("../builtins/prompts/tool-devtools-paused.md");
/// `next` hint after starting a background task.
pub const TOOL_BACKGROUND_NEXT: &str = include_str!("../builtins/prompts/tool-background-next.md");

// Evaluation harness.
/// Memory probe answered from context alone; `{{question}}`.
pub const EVAL_ANSWER_PROBE: &str = include_str!("../builtins/prompts/eval-answer-probe.md");
/// Memory check answered with tools over saved originals; `{{evidence}}`, `{{max_calls}}`, `{{question}}`.
pub const EVAL_RECALL_TASK: &str = include_str!("../builtins/prompts/eval-recall-task.md");

/// Replace each `{{name}}` placeholder in `template` with its value.
///
/// Substitution is a single pass over the template, so values may contain
/// braces of their own. In debug builds it panics when a value names a
/// placeholder the template lacks or when a placeholder is left unfilled.
pub fn fill(template: &str, values: &[(&str, &str)]) -> String {
    let extra: usize = values.iter().map(|(_, value)| value.len()).sum();
    let mut text = String::with_capacity(template.len() + extra);
    let mut used = vec![false; values.len()];
    let mut rest = template;
    while let Some(start) = rest.find("{{") {
        text.push_str(&rest[..start]);
        let after = &rest[start + 2..];
        let found = after.find("}}").and_then(|end| {
            let name = &after[..end];
            values
                .iter()
                .position(|(key, _)| *key == name)
                .map(|index| (index, end))
        });
        if let Some((index, end)) = found {
            text.push_str(values[index].1);
            used[index] = true;
            rest = &after[end + 2..];
        } else {
            if cfg!(debug_assertions) {
                panic!(
                    "prompt template has an unfilled placeholder near {:?}",
                    &rest[start..]
                );
            }
            text.push_str("{{");
            rest = after;
        }
    }
    text.push_str(rest);
    if cfg!(debug_assertions) {
        for ((name, _), used) in values.iter().zip(used) {
            assert!(used, "prompt template has no {{{{{name}}}}} placeholder");
        }
    }
    text
}

#[cfg(test)]
mod tests;
