//! `list_visions`: each Vision with what the app's cloud shows of it, read on the server so the
//! app never guesses: whether its budget lets it spend now, what waits on the person from it,
//! whether its latest run failed, and its own (hidden) chats. A Vision's budget state flipping
//! is announced (`visions-changed` with the Vision only), so the app reads the list again.
//!
//! Known cost (deferred): each Vision's budget judgement reads the Visions and channels again,
//! so a listing or a re-check is quadratic in the number of Visions, which the person sets and
//! keeps small.

use std::collections::{HashMap, HashSet};

use serde::{Deserialize, Serialize};

use super::{Hold, OwnedKind, Vision};
use crate::state::AppState;
use crate::types::{ThreadEvent, VisionsChange};

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
    /// What waits on the person from the chats it drives (not those the person took over):
    /// approvals waiting there and the questions it passed on that are still open.
    pub needs: usize,
    /// Its latest run ended in a failure (its own chat's or a work chat's; the person's Stop is
    /// none), and no run of its has succeeded since. Kept in memory: a restart clears it.
    pub sorry: bool,
}

/// What the app was last told of each Vision, in the Visions cache.
#[derive(Debug, Default)]
pub(crate) struct Shown {
    /// Each Vision's budget state as last judged.
    over: HashMap<String, bool>,
    /// The Visions whose latest run failed.
    sorry: HashSet<String>,
}

impl AppState {
    /// Every Vision, by name, with its budget state, what waits on the person, whether it is
    /// sorry, and its own chats.
    pub async fn visions_listed(&self) -> Vec<VisionListed> {
        let visions = self.visions_list();
        let driven = self.owners_where(|owner| owner.hold != Hold::TakenOver);
        // Judged before the chats' lock is taken: the judgement reads usage and configuration.
        let over: Vec<bool> = visions
            .iter()
            .map(|vision| self.visions_over_budget(&vision.id))
            .collect();
        for (vision, over) in visions.iter().zip(&over) {
            self.visions_budget_shown(&vision.id, *over);
        }
        let needs = self.visions_needs(&driven);
        let sorry = self.visions_cache().shown.sorry.clone();
        let threads = self.inner.threads.read().await;
        visions
            .into_iter()
            .zip(over)
            .map(|(vision, over_budget)| {
                let vision_chats = driven
                    .iter()
                    .filter(|o| o.vision_id == vision.id && o.kind == OwnedKind::VisionChat)
                    .map(|owner| VisionChat {
                        thread_id: owner.thread_id.clone(),
                        channel_id: owner.channel_id.clone(),
                        running: threads
                            .get(&owner.thread_id)
                            .is_some_and(|record| record.running),
                    })
                    .collect();
                VisionListed {
                    needs: needs.get(&vision.id).copied().unwrap_or(0),
                    sorry: sorry.contains(&vision.id),
                    vision,
                    over_budget,
                    vision_chats,
                }
            })
            .collect()
    }

    /// What waits on the person, by Vision: one pass over the waiting approvals and one read of
    /// each driven chat's questions.
    fn visions_needs(&self, driven: &[super::Owner]) -> HashMap<String, usize> {
        let vision_of: HashMap<&str, &str> = driven
            .iter()
            .map(|owner| (owner.thread_id.as_str(), owner.vision_id.as_str()))
            .collect();
        let mut needs: HashMap<String, usize> = HashMap::new();
        for request in self.pending_approvals() {
            if let Some(vision) = vision_of.get(request.thread_id.as_str()) {
                *needs.entry((*vision).to_owned()).or_default() += 1;
            }
        }
        for owner in driven {
            let passed = self
                .open_questions(&owner.thread_id)
                .iter()
                .filter(|question| question.passed_on)
                .count();
            if passed > 0 {
                *needs.entry(owner.vision_id.clone()).or_default() += passed;
            }
        }
        needs
    }

    /// A run of `thread_id` ended: when a Vision drives the chat, a failure (not the person's
    /// Stop) makes it sorry and a success clears that.
    pub(super) fn visions_mark_outcome(&self, thread_id: &str, terminal: &ThreadEvent) {
        let Some(owner) = self
            .owner_of(thread_id)
            .filter(|owner| owner.hold != Hold::TakenOver)
        else {
            return;
        };
        let sorry = &mut self.visions_cache().shown.sorry;
        match terminal {
            ThreadEvent::Failed { error }
                if !error.starts_with(themis_core::runtime::STOPPED_BY_YOU) =>
            {
                sorry.insert(owner.vision_id);
            }
            ThreadEvent::Finished { .. } | ThreadEvent::Incomplete { .. } => {
                sorry.remove(&owner.vision_id);
            }
            _ => {}
        }
    }

    /// Records `vision_id`'s budget state as judged now; a flip from what was last recorded is
    /// announced with the Vision only (a change the app reads the list again for). The first
    /// record announces nothing: the app reads the list when it starts.
    pub(super) fn visions_budget_shown(&self, vision_id: &str, over: bool) {
        let before = self
            .visions_cache()
            .shown
            .over
            .insert(vision_id.to_owned(), over);
        if before.is_none_or(|before| before == over) {
            return;
        }
        if let Some(sink) = self.scheduler_sink() {
            sink.emit_visions_changed(&VisionsChange {
                channel_id: None,
                vision_id: Some(vision_id.to_owned()),
            });
        }
    }

    /// Judges every Vision's budget again, announcing those that flipped: run when prices
    /// become known or fail to load, and every minute (a raise in a file, the day's start).
    pub(crate) fn visions_budget_recheck(&self) {
        for vision in self.visions_list() {
            let over = self.visions_over_budget(&vision.id);
            self.visions_budget_shown(&vision.id, over);
        }
    }
}
