//! Which Vision runs each chat, for plain chat: a Vision's work chats are listed there marked
//! with its cloud, and its own chats stay hidden. A listing reads the owner records and the
//! Visions once (one lock of the Visions cache), then looks each chat up in a map.

use std::collections::HashMap;

use serde_json::json;

use super::{OwnedKind, Vision};
use crate::state::AppState;
use crate::types::ThreadVision;

/// What plain chat shows of the chats Visions own, as one snapshot.
#[derive(Debug, Default)]
pub(crate) struct VisionMarks {
    /// Each owned chat's kind and Vision (None while the Visions cannot be read).
    owned: HashMap<String, (OwnedKind, Option<ThreadVision>)>,
}

impl VisionMarks {
    /// Whether plain chat leaves `thread_id` out: a Vision's own chat.
    pub(crate) fn hidden(&self, thread_id: &str) -> bool {
        matches!(self.owned.get(thread_id), Some((OwnedKind::VisionChat, _)))
    }

    /// The Vision running the work chat `thread_id`, if any.
    pub(crate) fn of(&self, thread_id: &str) -> Option<ThreadVision> {
        match self.owned.get(thread_id) {
            Some((OwnedKind::Work, vision)) => vision.clone(),
            _ => None,
        }
    }
}

fn mark(vision: &Vision) -> ThreadVision {
    ThreadVision {
        id: vision.id.clone(),
        name: vision.name.clone(),
        shape: vision.shape.clone(),
        color: vision.color.clone(),
    }
}

impl AppState {
    /// The marks of every owned chat, read in one lock of the Visions cache.
    ///
    /// While `owners.json` cannot be read, the owners last read are used (a Vision's own chats
    /// stay hidden, its work chats keep their mark) and diagnostics note how many were kept.
    /// With none read since start, nothing is marked or hidden: hiding needs the owner records,
    /// and listing every chat is better than listing none. While the Visions cannot be read,
    /// their own chats stay hidden and work chats show unmarked.
    pub(crate) fn visions_marks(&self) -> VisionMarks {
        let dir = &self.inner.visions_dir;
        let mut cache = self.visions_cache();
        cache.marks_built += 1;
        let (visions, mut failed) = match cache.visions(dir) {
            Ok(visions) => (
                visions.values().map(|v| (v.id.clone(), mark(v))).collect(),
                None,
            ),
            Err(error) => (HashMap::<String, ThreadVision>::new(), Some(error)),
        };
        let mut kept = None;
        match cache.owners(dir) {
            Ok(owners) => {
                let read = owners
                    .values()
                    .map(|o| (o.thread_id.clone(), (o.kind, o.vision_id.clone())))
                    .collect();
                cache.last_owned = Some(read);
            }
            Err(error) => {
                kept = Some(cache.last_owned.as_ref().map_or(0, HashMap::len));
                failed = Some(error);
            }
        }
        let owned = cache
            .last_owned
            .iter()
            .flatten()
            .map(|(thread, (kind, vision))| (thread.clone(), (*kind, visions.get(vision).cloned())))
            .collect();
        drop(cache);
        if let Some(error) = failed {
            self.visions_failed(error);
        }
        if let Some(kept) = kept {
            self.visions_note("marks_kept", json!({"kept": kept}));
        }
        VisionMarks { owned }
    }

    /// Whether plain chat leaves `thread_id` out (a Vision's own chat): one set lookup for a
    /// chat no Vision owns.
    pub(crate) fn visions_hides(&self, thread_id: &str) -> bool {
        self.visions_owns(thread_id)
            && self
                .owner_of(thread_id)
                .is_some_and(|owner| owner.kind == OwnedKind::VisionChat)
    }

    /// The Vision running the work chat `thread_id`, if any: one set lookup for a chat no
    /// Vision owns.
    pub(crate) fn visions_mark_of(&self, thread_id: &str) -> Option<ThreadVision> {
        if !self.visions_owns(thread_id) {
            return None;
        }
        let owner = self.owner_of(thread_id)?;
        if owner.kind != OwnedKind::Work {
            return None;
        }
        self.vision_get(&owner.vision_id).as_ref().map(mark)
    }
}
