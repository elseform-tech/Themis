//! `list_visions`: each Vision with what the app's cloud shows of it, read on the server so the
//! app never guesses: whether its budget lets it spend now and its own (hidden) chats.

use serde::{Deserialize, Serialize};

use super::{OwnedKind, Vision};
use crate::state::AppState;

/// A Vision's own chat in one of its channels, and whether a run of it is going.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct VisionChat {
    pub thread_id: String,
    pub channel_id: String,
    pub running: bool,
}

/// A Vision as the person's list gives it (`VisionInfo` in the app).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct VisionListed {
    #[serde(flatten)]
    pub vision: Vision,
    /// It may not spend now: a cap reached, or its spend cannot be checked (the server's own
    /// judgement, the same one that refuses its calls).
    pub over_budget: bool,
    pub vision_chats: Vec<VisionChat>,
}

impl AppState {
    /// Every Vision, by name, with its budget state and its own chats.
    pub async fn visions_listed(&self) -> Vec<VisionListed> {
        let visions = self.visions_list();
        let own = self.owners_where(|owner| owner.kind == OwnedKind::VisionChat);
        // Judged before the chats' lock is taken: the judgement reads usage and configuration.
        let over: Vec<bool> = visions
            .iter()
            .map(|vision| self.visions_over_budget(&vision.id))
            .collect();
        let threads = self.inner.threads.read().await;
        visions
            .into_iter()
            .zip(over)
            .map(|(vision, over_budget)| {
                let vision_chats = own
                    .iter()
                    .filter(|owner| owner.vision_id == vision.id)
                    .map(|owner| VisionChat {
                        thread_id: owner.thread_id.clone(),
                        channel_id: owner.channel_id.clone(),
                        running: threads
                            .get(&owner.thread_id)
                            .is_some_and(|record| record.running),
                    })
                    .collect();
                VisionListed {
                    vision,
                    over_budget,
                    vision_chats,
                }
            })
            .collect()
    }
}
