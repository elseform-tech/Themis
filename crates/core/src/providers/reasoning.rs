//! Replaying a thinking model's reasoning with its own tool calls.
//!
//! Chat-completions thinking models return their reasoning in `reasoning_content` (or
//! `reasoning`), apart from the reply text. Without it in the next request the model
//! re-derives every step from the tool results alone. With replay on, the reasoning that
//! came with a set of tool calls is kept in memory, keyed by call id, and sent back on that
//! assistant message while it stays in the request. It is never shown, logged or saved:
//! history and transcripts keep only the reply text and the calls.

use autoagents::llm::ToolCall;
use std::collections::VecDeque;
use std::sync::Mutex;

tokio::task_local! {
    /// Set for a run to replay reasoning; off when unset.
    pub static REPLAY_REASONING: bool;
}

/// Call ids remembered at once; older reasoning is dropped first.
const KEPT: usize = 512;

static KEPT_REASONING: Mutex<VecDeque<(String, String)>> = Mutex::new(VecDeque::new());

fn enabled() -> bool {
    REPLAY_REASONING.try_with(|on| *on).unwrap_or(false)
}

/// Remembers the reasoning that came with `calls`.
pub(super) fn remember(calls: &[ToolCall], reasoning: &str) {
    if !enabled() || reasoning.trim().is_empty() {
        return;
    }
    let Some(id) = calls.iter().map(|call| &call.id).find(|id| !id.is_empty()) else {
        return;
    };
    let mut kept = KEPT_REASONING.lock().unwrap_or_else(|e| e.into_inner());
    kept.retain(|(known, _)| known != id);
    if kept.len() >= KEPT {
        kept.pop_front();
    }
    kept.push_back((id.clone(), reasoning.to_owned()));
}

/// The reasoning remembered for `calls`, when replay is on.
pub(super) fn replay(calls: &[ToolCall]) -> Option<String> {
    if !enabled() {
        return None;
    }
    let kept = KEPT_REASONING.lock().unwrap_or_else(|e| e.into_inner());
    calls.iter().find_map(|call| {
        kept.iter()
            .find(|(id, _)| !call.id.is_empty() && *id == call.id)
            .map(|(_, reasoning)| reasoning.clone())
    })
}
