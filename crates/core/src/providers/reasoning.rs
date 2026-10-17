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
//! A final answer's reasoning (a reply with no tool calls) is kept too, keyed by the answer
//! text, so it goes back with that answer in later messages of the chat: the model keeps
//! its own line of thought from one message to the next, not only within one.
//!
//! It is never shown, logged or saved: history, transcripts and evidence keep only the
//! public text and the calls, so reasoning lasts as long as the process and stops being
//! sent once its calls or answer leave the request. Summaries never get it.

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
    /// Chat Completions reasoning text, sent back under the field it came in.
    pub(super) text: String,
    /// True when the text came as `reasoning` (Kimi, Hunyuan), not `reasoning_content`.
    pub(super) text_as_reasoning: bool,
    /// Chat Completions `reasoning_details` entries, fragments joined by index.
    pub(super) details: Vec<serde_json::Value>,
    /// Messages `thinking` / `redacted_thinking` blocks.
    pub(super) blocks: Vec<serde_json::Value>,
    /// Responses `reasoning` items.
    pub(super) items: Vec<serde_json::Value>,
}

impl Reasoning {
    fn is_empty(&self) -> bool {
        self.text.trim().is_empty()
            && self.details.is_empty()
            && self.blocks.is_empty()
            && self.items.is_empty()
    }
}

static KEPT_REASONING: Mutex<VecDeque<(String, Reasoning)>> = Mutex::new(VecDeque::new());

fn enabled() -> bool {
    REPLAY_REASONING.try_with(|on| *on).unwrap_or(true)
        && super::COMPACTION_REQUEST.try_with(|_| ()).is_err()
}

fn answer_key(text: &str) -> Option<String> {
    use std::hash::{Hash, Hasher};
    let text = text.trim();
    if text.is_empty() {
        return None;
    }
    let mut hash = std::collections::hash_map::DefaultHasher::new();
    text.hash(&mut hash);
    Some(format!("answer:{:016x}", hash.finish()))
}

/// Remembers the reasoning of one reply: by its tool calls, or by its answer text when it
/// has none.
pub(super) fn remember(calls: &[ToolCall], text: &str, reasoning: Reasoning) {
    if !enabled() || reasoning.is_empty() {
        return;
    }
    let id = calls
        .iter()
        .map(|call| call.id.clone())
        .find(|id| !id.is_empty())
        .or_else(|| calls.is_empty().then(|| answer_key(text)).flatten());
    let Some(id) = id else {
        return;
    };
    let mut kept = KEPT_REASONING.lock().unwrap_or_else(|e| e.into_inner());
    kept.retain(|(known, _)| *known != id);
    if kept.len() >= KEPT {
        kept.pop_front();
    }
    kept.push_back((id, reasoning));
}

/// The reasoning remembered for a final answer with this text, when replay is on.
pub(super) fn replay_answer(text: &str) -> Option<Reasoning> {
    let key = answer_key(text).filter(|_| enabled())?;
    let kept = KEPT_REASONING.lock().unwrap_or_else(|e| e.into_inner());
    kept.iter()
        .find(|(id, _)| *id == key)
        .map(|(_, reasoning)| reasoning.clone())
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
    /// The reply's public text, the key for an answer's reasoning.
    text: String,
}

impl StreamedReasoning {
    /// Takes the reasoning parts of a normalized stream delta.
    pub(super) fn take(&mut self, delta: &serde_json::Value) {
        if let Some(text) = delta["content"].as_str() {
            self.text.push_str(text);
        }
        if let Some(text) = delta["reasoning_content"].as_str() {
            self.reasoning.text.push_str(text);
        } else if let Some(text) = delta["reasoning"].as_str() {
            self.reasoning.text.push_str(text);
            self.reasoning.text_as_reasoning = true;
        }
        for part in delta["reasoning_details"].as_array().into_iter().flatten() {
            join_detail(&mut self.reasoning.details, part);
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
        let text = std::mem::take(&mut self.text);
        remember(calls, &text, std::mem::take(&mut self.reasoning));
    }
}

/// The reasoning parts of a complete (non-streamed) reply's normalized message.
pub(super) fn from_message(message: &super::WireChoiceMessage) -> Reasoning {
    Reasoning {
        text: message
            .reasoning_content
            .clone()
            .or_else(|| message.reasoning.clone())
            .unwrap_or_default(),
        text_as_reasoning: message.reasoning_content.is_none() && message.reasoning.is_some(),
        details: message.reasoning_details.clone(),
        blocks: message.thinking_blocks.clone(),
        items: message.reasoning_items.clone(),
    }
}

/// Joins a streamed `reasoning_details` fragment into the entry with its index (text,
/// summary, data and signature strings append), or adds it as a new entry.
fn join_detail(details: &mut Vec<serde_json::Value>, part: &serde_json::Value) {
    if !part.is_object() {
        return;
    }
    let same = details
        .iter_mut()
        .find(|known| known["index"] == part["index"] && known["type"] == part["type"]);
    let Some(known) = same else {
        details.push(part.clone());
        return;
    };
    for key in ["text", "summary", "data", "signature"] {
        if let Some(more) = part[key].as_str() {
            let joined = format!("{}{more}", known[key].as_str().unwrap_or_default());
            known[key] = joined.into();
        }
    }
}

/// The Chat Completions reasoning fields for an assistant message, in the shape they came.
#[derive(serde::Serialize, Default)]
pub(super) struct WireReasoning {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(super) reasoning_content: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(super) reasoning: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(super) reasoning_details: Option<Vec<serde_json::Value>>,
}

impl WireReasoning {
    pub(super) fn from_kept(kept: Option<Reasoning>) -> Self {
        let Some(kept) = kept else {
            return Self::default();
        };
        let text = Some(kept.text).filter(|text| !text.trim().is_empty());
        Self {
            reasoning_content: text.clone().filter(|_| !kept.text_as_reasoning),
            reasoning: text.filter(|_| kept.text_as_reasoning),
            reasoning_details: Some(kept.details).filter(|details| !details.is_empty()),
        }
    }
}
