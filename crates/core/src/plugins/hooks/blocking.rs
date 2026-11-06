use super::*;

/// Most of a hook's block reason that is kept, in bytes.
pub(super) const BLOCK_REASON_LIMIT: usize = 1024;

/// Most of the reason a `Stop` hook gives, in bytes: it is what the agent does next, often a whole
/// prompt, where another hook's only explains a refusal.
const STOP_REASON_LIMIT: usize = 32 * 1024;

pub(super) fn reason_limit(hook: &Hook) -> usize {
    if hook.event == "Stop" {
        STOP_REASON_LIMIT
    } else {
        BLOCK_REASON_LIMIT
    }
}

/// A hook asked to block the action, with the reason it gave, if any.
#[derive(Debug)]
pub(super) struct Blocked(pub(super) Option<String>);

impl std::fmt::Display for Blocked {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str("Hook blocked the action")
    }
}

impl std::error::Error for Blocked {}

/// The reason in a hook's output: `reason`, or Claude's
/// `hookSpecificOutput.permissionDecisionReason`.
pub(super) fn block_reason(output: &Value, limit: usize) -> Option<String> {
    output["reason"]
        .as_str()
        .or_else(|| output["hookSpecificOutput"]["permissionDecisionReason"].as_str())
        .map(str::trim)
        .filter(|reason| !reason.is_empty())
        .map(|reason| capped(reason, limit).to_owned())
}

/// A `Stop` hook wants the turn to go on: what the model is told. Labelled and fenced, since it
/// comes from a plugin and can't pose as the person.
#[derive(Debug)]
pub struct KeepGoing {
    pub hook: String,
    pub reason: Option<String>,
}

impl std::fmt::Display for KeepGoing {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(formatter, "Hook {} asks to keep going", self.hook)
    }
}

impl std::error::Error for KeepGoing {}

impl KeepGoing {
    /// The note the model gets.
    #[must_use]
    pub fn note(&self) -> String {
        let reason = self.reason.as_deref().unwrap_or_default();
        let fenced = reason.replace("<<<", "< < <").replace(">>>", "> > >");
        crate::prompts::fill(
            crate::prompts::HOOK_STOP_CONTINUE,
            &[("hook", &self.hook), ("reason", &fenced)],
        )
    }
}

/// What the person sees when a hook blocks a turn or a stage of the run.
pub(super) fn blocked_message(hook: &str, reason: Option<&str>) -> String {
    match reason {
        Some(reason) => format!("Blocked by hook {hook}: {reason}"),
        None => format!("Blocked by hook {hook}"),
    }
}

/// What the model gets when a hook blocks a tool call: the reason is labelled and fenced so
/// it cannot pose as the note or close the fence early.
pub(super) fn blocked_tool_note(hook: &str, reason: Option<&str>) -> String {
    let blocked = crate::prompts::fill(crate::prompts::HOOK_BLOCKED, &[("hook", hook)]);
    match reason {
        Some(reason) => {
            let fenced = reason.replace("<<<", "< < <").replace(">>>", "> > >");
            format!(
                "{blocked} {}\n<<<hook-reason\n{fenced}\n>>>",
                crate::prompts::HOOK_BLOCK_REASON_NOTE
            )
        }
        None => blocked,
    }
}
