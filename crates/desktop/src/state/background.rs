//! Background shell tasks in the app state: the per-chat feed, user Stop, the
//! bounded log view, and teardown with the chat or the server.
//!
//! The process work lives in `themis_core::tools::background`; this module only
//! adapts it to the shared server (events, JSON shapes, lifetimes).

use std::sync::Arc;

use chrono::{DateTime, SecondsFormat, Utc};
use themis_core::tools::background::{
    TaskView, DEFAULT_LOG_PAGE_BYTES, MAX_LOG_PAGE_BYTES, STOP_WAIT,
};

use super::AppState;
use crate::sink::EventSink;
use crate::types::{BackgroundTaskLog, ThreadEvent, ThreadEventEnvelope, ThreadTaskBackground};

/// Lines of recent output shown on a feed row.
const FEED_TAIL_LINES: usize = 3;

fn timestamp(milliseconds: u64) -> String {
    i64::try_from(milliseconds)
        .ok()
        .and_then(DateTime::<Utc>::from_timestamp_millis)
        .unwrap_or_default()
        .to_rfc3339_opts(SecondsFormat::Secs, true)
}

fn feed_row(view: TaskView) -> ThreadTaskBackground {
    ThreadTaskBackground {
        id: view.record.id,
        run_id: view.record.run_id,
        command: view.summary,
        cwd: view.record.cwd,
        state: view.record.state,
        exit_code: view.record.exit_code,
        pid: view.record.pid,
        started_at: timestamp(view.record.started_at_ms),
        ended_at: view.record.ended_at_ms.map(timestamp),
        timeout_secs: view.record.timeout_secs,
        last_lines: view.tail,
        last_lines_complete: view.tail_complete,
        log_bytes: view.log_bytes,
    }
}

impl AppState {
    /// This chat's background tasks, oldest first, with their latest output.
    pub(super) fn thread_background_tasks(&self, thread_id: &str) -> Vec<ThreadTaskBackground> {
        self.inner
            .background
            .list(thread_id, FEED_TAIL_LINES)
            .into_iter()
            .map(feed_row)
            .collect()
    }

    /// Stops one background task (the whole process group). The only task
    /// action a person can take; it never undoes work the task did.
    pub async fn stop_background_task(
        &self,
        thread_id: String,
        task_id: String,
    ) -> Result<ThreadTaskBackground, String> {
        let registry = Arc::clone(&self.inner.background);
        let view =
            tokio::task::spawn_blocking(move || registry.stop(&thread_id, &task_id, STOP_WAIT))
                .await
                .map_err(|error| error.to_string())??;
        Ok(feed_row(view))
    }

    /// A bounded page of the task's log; without `offset` the newest output.
    pub async fn get_background_task_log(
        &self,
        thread_id: String,
        task_id: String,
        offset: Option<u64>,
        max_bytes: Option<usize>,
    ) -> Result<BackgroundTaskLog, String> {
        let (record, chunk) = self.inner.background.log(
            &thread_id,
            &task_id,
            offset,
            max_bytes
                .unwrap_or(DEFAULT_LOG_PAGE_BYTES)
                .min(MAX_LOG_PAGE_BYTES),
        )?;
        Ok(BackgroundTaskLog {
            id: record.id,
            state: record.state,
            text: chunk.text,
            offset: chunk.offset,
            next_offset: chunk.next_offset,
            total_bytes: chunk.total,
            oldest_retained_offset: chunk.retained_from,
        })
    }

    /// Stops every chat's background tasks (server shutdown).
    pub async fn shutdown_background_tasks(&self) {
        let registry = Arc::clone(&self.inner.background);
        let _ = tokio::task::spawn_blocking(move || registry.shutdown(STOP_WAIT)).await;
    }

    /// Kills and forgets the background tasks of removed chats.
    pub(super) async fn forget_background_tasks(&self, thread_ids: &[String]) {
        let registry = Arc::clone(&self.inner.background);
        let ids = thread_ids.to_vec();
        let _ = tokio::task::spawn_blocking(move || {
            for id in &ids {
                registry.remove_thread(id, STOP_WAIT);
            }
        })
        .await;
    }

    /// Announces task changes on `sink` so the Tasks view refreshes without polling.
    pub(super) fn install_background_notifier(&self, sink: Arc<dyn EventSink>) {
        self.inner
            .background
            .set_notifier(Arc::new(move |thread_id| {
                sink.emit_thread_event(&ThreadEventEnvelope {
                    thread_id: thread_id.to_owned(),
                    run_id: String::new(),
                    event: ThreadEvent::BackgroundTasksChanged,
                });
            }));
    }

    /// Background-task registry for in-crate callers (tool wiring).
    pub(super) fn background_registry(
        &self,
    ) -> Arc<themis_core::tools::background::BackgroundRegistry> {
        Arc::clone(&self.inner.background)
    }
}
