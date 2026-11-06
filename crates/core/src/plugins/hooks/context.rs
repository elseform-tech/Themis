use super::*;

/// Most context `PromptSubmit` hooks may add to a turn, in bytes.
pub const PROMPT_CONTEXT_LIMIT: usize = 4096;

/// What this run's `PromptSubmit` hooks added for the model, if anything.
pub fn prompt_context() -> Option<String> {
    PROMPT_CONTEXT
        .try_with(Clone::clone)
        .ok()
        .filter(|context| !context.is_empty())
}

/// The context a hook's output adds: `context`, `additionalContext`, or Claude's
/// `hookSpecificOutput.additionalContext`.
fn added_context(output: &Value) -> Option<&str> {
    output["context"]
        .as_str()
        .or_else(|| output["additionalContext"].as_str())
        .or_else(|| output["hookSpecificOutput"]["additionalContext"].as_str())
        .filter(|text| !text.trim().is_empty())
}

/// Adds what each hook said to `context`, a line apart.
pub(super) fn add_context(context: &mut String, outputs: &[Value]) {
    for output in outputs {
        if let Some(text) = added_context(output) {
            if !context.is_empty() {
                context.push('\n');
            }
            context.push_str(text.trim());
        }
    }
}

/// A turn's hook context, within [`PROMPT_CONTEXT_LIMIT`]: the session's, the expansion's, then the
/// prompt's. This turn's own is kept whole when there's no room for all of it.
pub(super) fn prompt_context_of(session: &str, expansion: &str, prompt: &str) -> String {
    let own = [expansion, prompt]
        .into_iter()
        .filter(|text| !text.is_empty())
        .collect::<Vec<_>>()
        .join("\n");
    let own = capped(&own, PROMPT_CONTEXT_LIMIT);
    let room = PROMPT_CONTEXT_LIMIT.saturating_sub(own.len() + 1);
    let session = capped(session, room);
    match (session.is_empty(), own.is_empty()) {
        (true, _) => own.to_owned(),
        (false, true) => session.to_owned(),
        (false, false) => format!("{session}\n{own}"),
    }
}
