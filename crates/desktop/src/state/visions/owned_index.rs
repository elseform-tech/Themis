//! The ids of the chats Visions own, kept apart from the Visions cache's lock so the check after
//! every model request costs a chat no Vision owns one read lock and a set lookup. It is built
//! from `owners.json` on first use and rebuilt whenever an owner record is saved or released
//! here; while `owners.json` cannot be read it is tried again at most once a minute (the failure
//! logged once each time), and no chat counts as owned meanwhile.

use std::collections::HashSet;
use std::sync::{Mutex, RwLock};
use std::time::{Duration, Instant};

use serde_json::json;

use crate::state::AppState;

/// How long after a failed build the index is tried again.
const RETRY_AFTER: Duration = Duration::from_secs(60);

#[derive(Debug, Default)]
pub(crate) struct OwnedIndex {
    ids: RwLock<Option<HashSet<String>>>,
    tried: Mutex<Option<Instant>>,
}

impl OwnedIndex {
    /// Whether `thread_id` is owned; None while the index is not built.
    fn get(&self, thread_id: &str) -> Option<bool> {
        let ids = self.ids.read().unwrap_or_else(|e| e.into_inner());
        ids.as_ref().map(|ids| ids.contains(thread_id))
    }

    fn set(&self, ids: Option<HashSet<String>>) {
        *self.ids.write().unwrap_or_else(|e| e.into_inner()) = ids;
    }

    /// Whether a build may be tried now (it is then counted as tried).
    fn may_try(&self) -> bool {
        let mut tried = self.tried.lock().unwrap_or_else(|e| e.into_inner());
        if tried.is_some_and(|at| at.elapsed() < RETRY_AFTER) {
            return false;
        }
        *tried = Some(Instant::now());
        true
    }
}

impl AppState {
    /// Whether a Vision owns `thread_id`, by the index (built when needed and allowed).
    pub(in crate::state) fn visions_owns(&self, thread_id: &str) -> bool {
        let index = &self.inner.visions.owned;
        if let Some(owned) = index.get(thread_id) {
            return owned;
        }
        if index.may_try() {
            self.visions_rebuild_owned();
        }
        index.get(thread_id).unwrap_or(false)
    }

    /// Builds the index again from the owner records (left unbuilt when they cannot be read).
    pub(super) fn visions_rebuild_owned(&self) {
        // Set while the cache's lock is held, so two owner writes cannot leave the older
        // snapshot last (the lock order is cache, then index; nothing takes them the other way).
        let mut cache = self.visions_cache();
        let ids = cache
            .owners(&self.inner.visions_dir)
            .map(|owners| owners.keys().cloned().collect::<HashSet<String>>());
        let index = &self.inner.visions.owned;
        match ids {
            Ok(ids) => {
                index.set(Some(ids));
                drop(cache);
            }
            Err(error) => {
                index.set(None);
                drop(cache);
                self.visions_note(
                    "owned_index_unreadable",
                    json!({"file": error.file(), "reason": error.reason()}),
                );
            }
        }
    }
}

#[cfg(test)]
impl OwnedIndex {
    /// Forgets the index and the last try, as at start.
    pub(crate) fn reset_for_test(&self) {
        self.set(None);
        *self.tried.lock().unwrap() = None;
    }
}
