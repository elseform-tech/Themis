//! Ending a run, and the hook-session folders chats leave behind.

use super::*;

/// App-data folder of what hooks read about a chat (`hook-sessions/<chat id>/`).
const HOOK_SESSIONS_DIR: &str = "hook-sessions";

impl AppState {
    pub(in crate::state) fn hook_session_dir(&self, thread_id: &str) -> std::path::PathBuf {
        self.app_data_dir().join(HOOK_SESSIONS_DIR).join(thread_id)
    }

    /// Forgets what hooks read about deleted chats.
    pub(in crate::state) fn forget_hook_sessions(&self, thread_ids: &[String]) {
        for id in thread_ids {
            if super::chat_grants::safe_chat_id(id) {
                let _ = std::fs::remove_dir_all(self.hook_session_dir(id));
            }
        }
    }

    /// Marks a run idle, releases its concurrency permit, and persists.
    pub(in crate::state) async fn finish_run(&self, thread_id: &str) {
        self.touch_hook_session(thread_id);
        self.close_steer_inbox(thread_id);
        self.stop_subagents(thread_id);
        // A recording the agent left running ends with its run, and so do its breakpoints,
        // request blocks and throttling.
        self.stop_agent_recording(thread_id).await;
        if let Some(chat) = self.inner.browsers.existing(thread_id) {
            chat.end_debugging().await;
        }
        self.inner
            .stop_flags
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .remove(thread_id);
        // Both the terminal-event pump and run tail can finish a run; release
        // the permit only for the path that still owns the running state.
        let was_running = self
            .inner
            .threads
            .write()
            .await
            .get_mut(thread_id)
            .map(|record| std::mem::replace(&mut record.running, false))
            .unwrap_or(false);
        if was_running {
            // Keep Rust 1.98 support; the try_update replacement requires Rust 1.99.
            #[allow(deprecated)]
            let _ = self.inner.running_count.fetch_update(
                Ordering::SeqCst,
                Ordering::SeqCst,
                |count| count.checked_sub(1),
            );
        }
        self.persist_registry().await;
    }
}
