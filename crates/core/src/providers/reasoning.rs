//! Replaying a thinking model's reasoning with its own tool calls.
//!
//! A thinking model reasons before each set of tool calls. Without that reasoning in the
//! next request it re-derives every step from the tool results alone and tends to repeat
//! checks it already made. So the reasoning that came with a set of tool calls is kept in
//! memory, keyed by call id, and sent back on that assistant message, in the shape the
//! wire protocol uses:
//!
//! - Chat Completions: `reasoning_content` (or `reasoning`) text on the assistant message.
//! - Messages: the `thinking` / `redacted_thinking` blocks, unchanged (their signatures
//!   must round-trip), before the message's text and tool uses.
//! - Responses: the `reasoning` output items (with `encrypted_content`, requested because
//!   requests are not stored), before the function calls.
//!
//! It is never shown, logged or saved: history, transcripts and evidence keep only the
//! public text and the calls, so reasoning lasts as long as the process and stops being
//! sent once its calls leave the request. Summaries never get it.

use autoagents::llm::ToolCall;
use std::collections::VecDeque;
use std::sync::Mutex;

tokio::task_local! {
    /// `false` turns replay off for a run (an eval comparison); on when unset.
    pub static REPLAY_REASONING: bool;
}

/// Call sets remembered at once; the oldest are dropped first.
const KEPT: usize = 512;

/// The reasoning that came with one set of tool calls.
#[derive(Clone, Debug, Default, PartialEq)]
pub(super) struct Reasoning {
    /// Chat Completions `reasoning_content`.
    pub(super) text: String,
    /// Messages `thinking` / `redacted_thinking` blocks.
    pub(super) blocks: Vec<serde_json::Value>,
    /// Responses `reasoning` items.
    pub(super) items: Vec<serde_json::Value>,
}

impl Reasoning {
    fn is_empty(&self) -> bool {
        self.text.trim().is_empty() && self.blocks.is_empty() && self.items.is_empty()
    }
}

static KEPT_REASONING: Mutex<VecDeque<(String, Reasoning)>> = Mutex::new(VecDeque::new());

fn enabled() -> bool {
    REPLAY_REASONING.try_with(|on| *on).unwrap_or(true)
        && super::COMPACTION_REQUEST.try_with(|_| ()).is_err()
}

/// Remembers the reasoning that came with `calls`.
pub(super) fn remember(calls: &[ToolCall], reasoning: Reasoning) {
    if !enabled() || reasoning.is_empty() {
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
    kept.push_back((id.clone(), reasoning));
}

/// The reasoning remembered for `calls`, when replay is on.
pub(super) fn replay(calls: &[ToolCall]) -> Option<Reasoning> {
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

/// Collects the reasoning of one streamed reply, whatever the wire protocol.
#[derive(Default)]
pub(super) struct StreamedReasoning {
    reasoning: Reasoning,
}

impl StreamedReasoning {
    /// Takes the reasoning parts of a normalized stream delta.
    pub(super) fn take(&mut self, delta: &serde_json::Value) {
        if let Some(text) = delta["reasoning_content"]
            .as_str()
            .or_else(|| delta["reasoning"].as_str())
        {
            self.reasoning.text.push_str(text);
        }
        if delta["thinking_block"].is_object() {
            self.reasoning.blocks.push(delta["thinking_block"].clone());
        }
        for (field, key) in [
            ("thinking_delta", "thinking"),
            ("signature_delta", "signature"),
        ] {
            if let (Some(part), Some(block)) =
                (delta[field].as_str(), self.reasoning.blocks.last_mut())
            {
                let joined = format!("{}{part}", block[key].as_str().unwrap_or_default());
                block[key] = joined.into();
            }
        }
        if delta["reasoning_item"].is_object() {
            self.reasoning.items.push(delta["reasoning_item"].clone());
        }
    }

    #[cfg(test)]
    pub(super) fn peek(&self) -> &Reasoning {
        &self.reasoning
    }

    /// Remembers what was collected for the reply's `calls`, and starts over.
    pub(super) fn finish(&mut self, calls: &[ToolCall]) {
        remember(calls, std::mem::take(&mut self.reasoning));
    }
}

/// The reasoning parts of a complete (non-streamed) reply's normalized message.
pub(super) fn from_message(message: &super::WireChoiceMessage) -> Reasoning {
    Reasoning {
        text: message.reasoning_content.clone().unwrap_or_default(),
        blocks: message.thinking_blocks.clone(),
        items: message.reasoning_items.clone(),
    }
}
