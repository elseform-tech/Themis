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
/// Skill catalog appended to the system message; `{{skills}}` is the metadata JSON.
pub const SYSTEM_SKILL_CATALOG: &str = include_str!("../builtins/prompts/system-skill-catalog.md");

// Desktop run setup.
/// Current run workspace preamble before the skill catalog; `{{workspace}}`.
pub const DESKTOP_RUN_WORKSPACE: &str =
    include_str!("../builtins/prompts/desktop-run-workspace.md");
/// Label before an MCP server's own instructions in the system message.
pub const DESKTOP_MCP_GUIDANCE: &str = include_str!("../builtins/prompts/desktop-mcp-guidance.md");
/// Integration status for an MCP connection that failed; `{{plugin}}`, `{{name}}`, `{{reason}}`.
pub const DESKTOP_MCP_UNAVAILABLE: &str =
    include_str!("../builtins/prompts/desktop-mcp-unavailable.md");

// Goal mode.
/// System message of the goal check that judges each goal turn.
pub const GOAL_CHECK_SYSTEM: &str = include_str!("../builtins/prompts/goal-check-system.md");
/// User message of the goal check; `{{goal}}`, `{{answer}}`, `{{evidence}}` are fenced data.
pub const GOAL_CHECK_USER: &str = include_str!("../builtins/prompts/goal-check-user.md");
/// Active-goal context appended to every run in a chat with a running goal; `{{goal}}`.
pub const GOAL_CONTEXT: &str = include_str!("../builtins/prompts/goal-context.md");
/// Message that starts a goal turn; `{{goal_turn_marker}}`, `{{turn}}`, `{{goal}}`, `{{note}}`.
pub const GOAL_TURN: &str = include_str!("../builtins/prompts/goal-turn.md");
/// Goal-turn note after the user resumes a stuck goal.
pub const GOAL_NOTE_RESUMED_STUCK: &str =
    include_str!("../builtins/prompts/goal-note-resumed-stuck.md");
/// Goal-turn note after the user stopped the previous turn.
pub const GOAL_NOTE_STOPPED: &str = include_str!("../builtins/prompts/goal-note-stopped.md");
/// Goal-turn note after the previous turn failed; `{{error}}`.
pub const GOAL_NOTE_FAILED: &str = include_str!("../builtins/prompts/goal-note-failed.md");
/// Goal-turn note after Themis closed during a turn.
pub const GOAL_NOTE_INTERRUPTED: &str =
    include_str!("../builtins/prompts/goal-note-interrupted.md");
/// `note` field of the set_goal tool result.
pub const GOAL_SET_NOTE: &str = include_str!("../builtins/prompts/goal-set-note.md");

// Summaries and compaction.
/// System prompt of every context checkpoint request.
pub const COMPACTION_CHECKPOINT: &str =
    include_str!("../builtins/prompts/compaction-checkpoint.md");
/// User message of a checkpoint request; `{{previous}}`, `{{label}}`, `{{notes}}`, `{{current_request}}`.
pub const SUMMARY_CHECKPOINT_REQUEST: &str =
    include_str!("../builtins/prompts/summary-checkpoint-request.md");
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
