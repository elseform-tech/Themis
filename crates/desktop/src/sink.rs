//! Event delivery seam: production emits Tauri events, tests collect them.
//!
//! All handler logic in [`crate::state`] talks to an [`EventSink`], never to
//! Tauri directly. [`TauriSink`] forwards to the running app; [`ChannelSink`]
//! records events on an in-memory channel for headless tests.

use tauri::{AppHandle, Emitter};

use crate::types::{
    ApprovalRequest, ReviewItem, ThreadEventEnvelope, APPROVAL_REQUEST_NAME, REVIEW_ITEM_NAME,
    THREAD_EVENT_NAME,
};

/// Delivers thread, approval, and review events to whoever is listening.
pub trait EventSink: Send + Sync {
    /// Emits a `thread-event` envelope.
    fn emit_thread_event(&self, envelope: &ThreadEventEnvelope);
    /// Emits an `approval-request` dialog request.
    fn emit_approval_request(&self, request: &ApprovalRequest);
    /// Emits a `review-item-added` payload.
    fn emit_review_item(&self, item: &ReviewItem);
}

/// [`EventSink`] backed by the running Tauri app.
///
/// Emission is best-effort: a closed window must never fail a run.
pub struct TauriSink {
    app: AppHandle,
}

impl TauriSink {
    /// Wraps `app` for event emission.
    #[must_use]
    pub const fn new(app: AppHandle) -> Self {
        Self { app }
    }
}

impl EventSink for TauriSink {
    fn emit_thread_event(&self, envelope: &ThreadEventEnvelope) {
        let _ = self.app.emit(THREAD_EVENT_NAME, envelope);
    }

    fn emit_approval_request(&self, request: &ApprovalRequest) {
        let _ = self.app.emit(APPROVAL_REQUEST_NAME, request);
    }

    fn emit_review_item(&self, item: &ReviewItem) {
        let _ = self.app.emit(REVIEW_ITEM_NAME, item);
    }
}

/// One event captured by [`ChannelSink`].
#[derive(Debug, Clone)]
pub enum TestEvent {
    /// A `thread-event` envelope.
    Thread(ThreadEventEnvelope),
    /// An `approval-request` payload.
    Approval(ApprovalRequest),
    /// A `review-item-added` payload.
    Review(ReviewItem),
}

/// [`EventSink`] that records events on an unbounded channel for tests.
///
/// Unbounded on purpose: the synchronous approval hook must never block on a
/// full channel while emitting the dialog request.
pub struct ChannelSink {
    tx: tokio::sync::mpsc::UnboundedSender<TestEvent>,
}

impl ChannelSink {
    /// Creates a sink plus the receiver its events land on.
    #[must_use]
    pub fn channel() -> (Self, tokio::sync::mpsc::UnboundedReceiver<TestEvent>) {
        let (tx, rx) = tokio::sync::mpsc::unbounded_channel();
        (Self { tx }, rx)
    }
}

impl EventSink for ChannelSink {
    fn emit_thread_event(&self, envelope: &ThreadEventEnvelope) {
        let _ = self.tx.send(TestEvent::Thread(envelope.clone()));
    }

    fn emit_approval_request(&self, request: &ApprovalRequest) {
        let _ = self.tx.send(TestEvent::Approval(request.clone()));
    }

    fn emit_review_item(&self, item: &ReviewItem) {
        let _ = self.tx.send(TestEvent::Review(item.clone()));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::{ReviewStatus, RiskLevel, ThreadEvent};

    #[test]
    fn channel_sink_collects_all_event_kinds_in_order() {
        let (sink, mut rx) = ChannelSink::channel();
        sink.emit_thread_event(&ThreadEventEnvelope {
            thread_id: "t".to_owned(),
            run_id: "r".to_owned(),
            event: ThreadEvent::Finished {
                model: None,
                result: "ok".to_owned(),
            },
        });
        sink.emit_approval_request(&ApprovalRequest {
            thread_id: "t".to_owned(),
            approval_id: "a".to_owned(),
            tool: "shell".to_owned(),
            summary: "run".to_owned(),
            risk: RiskLevel::Execute,
        });
        sink.emit_review_item(&ReviewItem {
            id: "r".to_owned(),
            automation_id: "a".to_owned(),
            thread_id: "t".to_owned(),
            created_at: "2026-01-01T00:00:00Z".to_owned(),
            title: "title".to_owned(),
            summary: "done".to_owned(),
            status: ReviewStatus::Pending,
        });
        drop(sink);
        let first = rx.blocking_recv().expect("thread event");
        assert!(matches!(first, TestEvent::Thread(_)), "{first:?}");
        let second = rx.blocking_recv().expect("approval event");
        assert!(matches!(second, TestEvent::Approval(_)), "{second:?}");
        let third = rx.blocking_recv().expect("review event");
        assert!(matches!(third, TestEvent::Review(_)), "{third:?}");
        assert!(rx.blocking_recv().is_none());
    }
}
