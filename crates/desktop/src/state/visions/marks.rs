//! Which Vision runs each chat, for plain chat: a Vision's work chats are listed there marked
//! with its cloud, and its own chats stay hidden. A listing reads the owner records and the
//! Visions once (one lock of the Visions cache), then looks each chat up in a map.

use std::collections::HashMap;

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
    /// The marks of every owned chat, read in one lock of the Visions cache. While the owner
    /// records cannot be read nothing is marked or hidden; while the Visions cannot be read
    /// their own chats stay hidden and work chats show unmarked.
    pub(crate) fn visions_marks(&self) -> VisionMarks {
        let dir = &self.inner.visions_dir;
        let mut cache = self.visions_cache();
        cache.marks_built += 1;
        let (visions, failed) = match cache.visions(dir) {
            Ok(visions) => (
                visions.values().map(|v| (v.id.clone(), mark(v))).collect(),
                None,
            ),
            Err(error) => (HashMap::new(), Some(error)),
        };
        let (owned, failed) = match cache.owners(dir) {
            Ok(owners) => {
                let owned = owners
                    .values()
                    .map(|owner| {
                        let vision = visions.get(&owner.vision_id).cloned();
                        (owner.thread_id.clone(), (owner.kind, vision))
                    })
                    .collect();
                (owned, failed)
            }
            Err(error) => (HashMap::new(), Some(error)),
        };
        drop(cache);
        if let Some(error) = failed {
            self.visions_failed(error);
        }
        VisionMarks { owned }
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
