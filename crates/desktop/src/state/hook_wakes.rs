//! When a finished background hook has left a note for a chat (`plugins::hooks::RewakeNote`) and
//! nothing is running there, it may start a turn so the agent reads it. That is bounded:
//! `hooks.max_wakes` turns in a row until the person writes again, none after a turn that failed or
//! that the person stopped, none in YOLO (nobody asked anything, and everything is open), none while
//! a goal is running (its next turn reads the note). Otherwise the note waits for the next turn.

use std::sync::Arc;

use crate::types::GoalStatus;
use serde_json::json;
use themis_core::configuration::ApprovalMode;
use themis_core::plugins::hooks;

use super::queue::ServerSend;
use super::*;
use crate::sink::EventSink;

/// A count above any `hooks.max_wakes`: no more wakes until the person writes.
const HELD: u32 = u32::MAX;

impl AppState {
    /// What a run's hooks call when one leaves a note.
    pub(super) fn hook_report_notifier(&self, sink: Arc<dyn EventSink>) -> hooks::RewakeNotify {
        let state = self.clone();
        Arc::new(move |thread_id: &str| {
            let (state, sink, thread_id) = (state.clone(), Arc::clone(&sink), thread_id.to_owned());
            let task: std::pin::Pin<Box<dyn std::future::Future<Output = ()> + Send>> =
                Box::pin(async move { state.wake_for_hook_report(sink, thread_id).await });
            tokio::spawn(task);
        })
    }

    fn hook_wake_count(&self, thread_id: &str) -> u32 {
        self.inner
            .hook_wakes
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .get(thread_id)
            .copied()
            .unwrap_or(0)
    }

    fn set_hook_wake_count(&self, thread_id: &str, count: u32) {
        self.inner
            .hook_wakes
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .insert(thread_id.to_owned(), count);
    }

    /// The person wrote: hooks may start turns again.
    pub(super) fn reset_hook_wakes(&self, thread_id: &str) {
        self.inner
            .hook_wakes
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .remove(thread_id);
    }

    /// No more wakes in this chat until the person writes.
    pub(super) fn hold_hook_wakes(&self, thread_id: &str) {
        self.set_hook_wake_count(thread_id, HELD);
    }

    /// A deleted chat leaves nothing behind.
    pub(super) fn forget_hook_reports(&self, thread_id: &str) {
        self.reset_hook_wakes(thread_id);
        hooks::take_rewake_notes(thread_id);
    }

    /// Why a note may not start a turn in this chat now, for the diagnostics.
    async fn hook_wake_refused(&self, thread_id: &str) -> Option<&'static str> {
        let threads = self.inner.threads.read().await;
        let record = threads.get(thread_id)?;
        if record.running {
            return Some("running");
        }
        if record.approval_mode == ApprovalMode::Yolo {
            return Some("yolo");
        }
        let root = std::path::PathBuf::from(&record.project_root);
        drop(threads);
        if self
            .goal_for(thread_id)
            .is_some_and(|goal| goal.status == GoalStatus::Running)
        {
            return Some("goal");
        }
        let limit = self
            .runtime_configuration(Some(&root), None)
            .map_or(0, |resolved| resolved.config.hooks.max_wakes);
        (self.hook_wake_count(thread_id) >= limit).then_some("limit")
    }

    /// Starts a turn for what a hook reported, if a turn may start. Every other case leaves the
    /// note for the chat's next turn.
    pub(super) async fn wake_for_hook_report(&self, sink: Arc<dyn EventSink>, thread_id: String) {
        if !hooks::rewake_waiting(&thread_id) {
            return;
        }
        if self.require_thread(&thread_id).await.is_err() {
            self.forget_hook_reports(&thread_id);
            return;
        }
        if let Some(reason) = self.hook_wake_refused(&thread_id).await {
            let _ = self.inner.diagnostics.append(
                "hook",
                "rewake_waits",
                "info",
                json!({"thread_id": thread_id, "reason": reason}),
            );
            return;
        }
        // Only this fixed text goes through the path of the person's messages, which reads skills,
        // plugins and commands in what it is given. What the hook wrote stays where it is, and the
        // run takes it at its first step as a labelled note.
        let text = themis_core::prompts::fill(
            themis_core::prompts::HOOK_WAKE_TURN,
            &[("marker", hooks::HOOK_WAKE_MARKER)],
        );
        self.set_hook_wake_count(
            &thread_id,
            self.hook_wake_count(&thread_id).saturating_add(1),
        );
        let run_id = uuid::Uuid::new_v4().to_string();
        let (sent, progress) = self
            .send_from_server(sink, &thread_id, text, &run_id, None, Vec::new())
            .await;
        let woke = sent.is_ok();
        // Refused before it was saved (busy, the concurrency limit): the chat's next turn reads
        // the notes, and this one didn't count.
        if !woke && matches!(progress, ServerSend::Pending) {
            let count = self.hook_wake_count(&thread_id);
            if count != HELD {
                self.set_hook_wake_count(&thread_id, count.saturating_sub(1));
            }
        }
        let _ = self.inner.diagnostics.append(
            "hook",
            if woke {
                "rewake_started"
            } else {
                "rewake_refused"
            },
            if woke { "info" } else { "warn" },
            json!({"thread_id": thread_id}),
        );
    }
}
