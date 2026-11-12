//! Waking a Vision: a run in its Vision chat for a channel.

use std::sync::Arc;

use crate::sink::EventSink;
use crate::state::AppState;

/// Why a Vision wakes. Chat-related reasons carry the owned chat's id.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum WakeReason {
    /// The person mentioned it in its channel (or wrote where it is the only member).
    Mentioned,
    ChatFinished(String),
    ChatFailed(String),
    ChatStopped(String),
    /// The chat and the question's id.
    QuestionAsked(String, String),
    HandedBack(String),
}

impl AppState {
    /// Wakes `vision_id` in `channel_id` for `reason`. Vision runs are not built yet, so this
    /// does nothing for now.
    pub fn visions_wake(
        &self,
        _sink: &Arc<dyn EventSink>,
        _channel_id: &str,
        _vision_id: &str,
        _reason: WakeReason,
    ) {
    }
}
