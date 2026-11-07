//! How big the context is and may get: token estimates, model windows, reserves and the per-kind breakdown.

use super::*;

/// 19155 -> "19,155", for numbers people read in an error.
pub(super) fn grouped(number: usize) -> String {
    let digits = number.to_string();
    let mut out = String::new();
    for (index, digit) in digits.chars().enumerate() {
        if index > 0 && (digits.len() - index).is_multiple_of(3) {
            out.push(',');
        }
        out.push(digit);
    }
    out
}

/// Whether the last `limit` checkpoints each came within two model requests of the one before:
/// the budget is too small for the run to make progress. `limit` 0 never thrashes.
pub(super) fn thrashing(checkpoint_turns: &[usize], limit: usize) -> bool {
    limit > 0
        && checkpoint_turns.len() >= limit
        && checkpoint_turns[checkpoint_turns.len() - limit..]
            .windows(2)
            .all(|pair| pair[1] - pair[0] <= 2)
}

pub(super) fn context_breakdown(messages: &[ChatMessage]) -> ContextBreakdown {
    let mut breakdown = ContextBreakdown::default();
    // Only the latest goal note counts as the goal's: older goal turns are conversation now.
    let latest_goal_note = messages.iter().rposition(|message| {
        message.role == ChatRole::User
            && matches!(message.message_type, MessageType::Text)
            && crate::goal::goal_note_start(&message.content).is_some()
    });
    for (index, message) in messages.iter().enumerate() {
        // Visible content goes to its own part; replayed reasoning has a part of its own.
        let tokens = crate::calibration::message_tokens(message);
        breakdown.reasoning += crate::providers::reasoning::replayed_tokens(message);
        let slot = match (&message.role, &message.message_type) {
            (ChatRole::System, _) => &mut breakdown.system,
            (_, MessageType::ToolUse(_) | MessageType::ToolResult(_)) => {
                &mut breakdown.tool_activity
            }
            (ChatRole::Assistant, _)
                if message.content.starts_with("Earlier context checkpoint")
                    || message.content.starts_with("Suspended run progress (") =>
            {
                &mut breakdown.checkpoint
            }
            (ChatRole::User, _)
                if message.content.starts_with("Attached UTF-8 file")
                    || message.content.starts_with("Attached original") =>
            {
                &mut breakdown.attachments
            }
            _ if Some(index) == latest_goal_note => {
                // The note's share of the message's tokens, by bytes, so the parts still add up.
                let at = crate::goal::goal_note_start(&message.content).unwrap_or(0);
                let total = message.content.len().max(1);
                let goal = tokens * (total - at) / total;
                breakdown.goal += goal;
                breakdown.conversation += tokens - goal;
                continue;
            }
            _ => &mut breakdown.conversation,
        };
        *slot += tokens;
    }
    let memory = MEMORY_BLOCK
        .try_with(|block| memory_tokens(block, messages))
        .unwrap_or(0);
    breakdown.memory = memory.min(breakdown.system);
    breakdown.system -= breakdown.memory;
    breakdown
}

/// Tokens of `block` when the request's system message carries it (bytes / 4, as every part
/// is estimated); 0 when it is empty or not sent.
pub(super) fn memory_tokens(block: &str, messages: &[ChatMessage]) -> usize {
    if block.is_empty() {
        return 0;
    }
    let sent = messages
        .iter()
        .any(|message| message.role == ChatRole::System && message.content.contains(block));
    if sent {
        block.len().div_ceil(4)
    } else {
        0
    }
}

pub(super) fn estimated_tokens(messages: &[ChatMessage]) -> usize {
    messages
        .iter()
        .map(|message| {
            crate::calibration::message_tokens(message)
                + crate::providers::reasoning::replayed_tokens(message)
        })
        .sum()
}

/// Message tokens corrected by provider-reported usage; unchanged before the first report.
pub(super) fn calibrated_tokens(
    calibration: &crate::calibration::TokenCalibration,
    messages: &[ChatMessage],
    schema_tokens: usize,
) -> usize {
    calibration.messages(estimated_tokens(messages), schema_tokens)
}

pub async fn with_model_context_capacity<F: std::future::Future>(
    capacity: Option<Option<usize>>,
    future: F,
) -> F::Output {
    if let Some(capacity) = capacity {
        MODEL_CONTEXT_CAPACITY.scope(capacity, future).await
    } else {
        future.await
    }
}

/// The provider's error text (message and body) where a stated context
/// limit may appear; never logged.
pub(super) fn overflow_error_text(error: &anyhow::Error) -> String {
    use autoagents::llm::error::LLMError;
    match error.downcast_ref::<LLMError>() {
        Some(LLMError::HttpStatusError {
            message,
            response_body,
            ..
        }) => format!("{message}\n{response_body}"),
        Some(LLMError::InvalidRequest {
            message,
            response_body,
            ..
        }) => format!(
            "{message}\n{}",
            response_body.as_deref().unwrap_or_default()
        ),
        Some(other) => other.to_string(),
        None => error.to_string(),
    }
}

/// Output reserve, safety margin and the message budget left for a request
/// within `budget` tokens.
pub(super) fn request_reserves(
    model: &str,
    budget: usize,
    schema_tokens: usize,
) -> (usize, usize, usize) {
    let answer_reserve =
        crate::providers::bounded_output_limit(model, (budget / 8).min(8192) as u32) as usize;
    let safety_margin = (budget / 50).max(256);
    let input_budget = budget.saturating_sub(
        schema_tokens
            .saturating_add(answer_reserve)
            .saturating_add(safety_margin),
    );
    (answer_reserve, safety_margin, input_budget)
}

/// The model's window as this run knows it (see [`crate::context_window`]).
pub(super) fn model_window(model: &str) -> crate::context_window::Window {
    crate::context_window::window(model, MODEL_CONTEXT_CAPACITY.try_with(|c| *c).ok())
}

/// The configured cap clamped to the model's window.
pub(super) fn effective_context_budget(requested: usize, model: &str) -> usize {
    crate::context_window::effective_budget(requested, model_window(model))
}
