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

/// What marks a turn a hook started: it isn't the person's message, and the chat shows it as what
/// it is. The chat's text after it, up to the line's end, is the plugin's own description.
pub const HOOK_WAKE_MARKER: &str = "[Plugin hook \u{2014} runtime, not from the user]";

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
        let notes = hooks::take_rewake_notes(&thread_id);
        if notes.is_empty() {
            return;
        }
        let summaries: Vec<String> = notes
            .iter()
            .map(|note| themis_core::goal::one_line(&note.summary, 200))
            .collect();
        let bodies: Vec<String> = notes.iter().map(hooks::RewakeNote::model_text).collect();
        let text = format!(
            "{HOOK_WAKE_MARKER} {}\n{}",
            summaries.join("; "),
            bodies.join("\n\n")
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
        // Refused before it was saved (busy, the concurrency limit): nothing was lost, and the
        // chat's next turn reads them.
        if !woke && matches!(progress, ServerSend::Pending) {
            hooks::restore_rewake_notes(&thread_id, notes);
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
