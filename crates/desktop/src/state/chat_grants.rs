//! Per-chat "Allow in this chat" approval grants in the app data directory.
//!
//! Each chat owns `chat_grants/<chat id>.json` beside `threads.json`. The
//! format and its validation live in `themis_core::chat_grants`; this module
//! only locates, loads and removes the file. To undo a grant, edit or delete
//! the file (or start a new chat); there is intentionally no UI for it.

use std::path::PathBuf;
use std::sync::Arc;

use serde_json::json;
use themis_core::chat_grants::ChatGrants;

use super::AppState;

/// Directory name under the app data directory.
pub(super) const CHAT_GRANTS_DIR: &str = "chat_grants";

/// Chat ids are generated UUIDs; anything else never maps to a file.
pub(super) fn safe_chat_id(id: &str) -> bool {
    !id.is_empty()
        && id.len() <= 64
        && id
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || byte == b'-' || byte == b'_')
}

impl AppState {
    /// File holding the grants of `thread_id`, or `None` for an unsafe id.
    pub fn chat_grants_path(&self, thread_id: &str) -> Option<PathBuf> {
        safe_chat_id(thread_id)
            .then(|| self.inner.chat_grants_dir.join(format!("{thread_id}.json")))
    }

    /// Loads the chat's grants. A damaged file is ignored (no grants, never
    /// more permissions) and noted in diagnostics with a fixed reason.
    pub(super) fn chat_grants_for(&self, thread_id: &str) -> Arc<ChatGrants> {
        let Some(path) = self.chat_grants_path(thread_id) else {
            return ChatGrants::in_memory();
        };
        let grants = ChatGrants::load(&path);
        if let Some(reason) = grants.warning() {
            let _ = self.inner.diagnostics.append(
                "approval",
                "chat_grants_ignored",
                "warn",
                json!({"thread_id": thread_id, "reason": reason}),
            );
        }
        grants
    }

    /// Removes the grants of deleted chats.
    pub(super) fn forget_chat_grants(&self, thread_ids: &[String]) {
        for id in thread_ids {
            if let Some(path) = self.chat_grants_path(id) {
                let _ = std::fs::remove_file(path);
            }
        }
    }
}
