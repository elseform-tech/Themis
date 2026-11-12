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
use crate::types::{Question, ThreadEvent, VisionsChange};

/// Open questions that wait on the person: passed on by the Vision, or asking for a permission
/// (an Auto block, a destructive step's box), each counted once.
fn waiting_on_person(questions: &[Question]) -> usize {
    questions
        .iter()
        .filter(|q| q.answer.is_none() && (q.passed_on || q.permission))
        .count()
}

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
    /// approvals waiting there, and open questions it passed on or that ask for a permission
    /// (an Auto block, a destructive step's box: the person's alone), each counted once.
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
    /// Each driven chat's open questions that wait on the person, kept by the question writers
    /// (read from the chat once when unknown, as after a restart).
    waiting: HashMap<String, usize>,
    /// Chats whose questions were read to count what waits on the person (for the scale test).
    pub(super) chats_read: usize,
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

    /// What waits on the person, by Vision: one pass over the waiting approvals and each driven
    /// chat's kept count (a chat not counted yet is read once).
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
        let known: Vec<Option<usize>> = {
            let cache = self.visions_cache();
            let waiting = &cache.shown.waiting;
            driven
                .iter()
                .map(|owner| waiting.get(&owner.thread_id).copied())
                .collect()
        };
        for (owner, known) in driven.iter().zip(known) {
            let count = known.unwrap_or_else(|| {
                let read = waiting_on_person(&self.questions_for(&owner.thread_id));
                let mut cache = self.visions_cache();
                cache.shown.chats_read += 1;
                // A writer's count, kept meanwhile, is newer than this read.
                *cache
                    .shown
                    .waiting
                    .entry(owner.thread_id.clone())
                    .or_insert(read)
            });
            if count > 0 {
                *needs.entry(owner.vision_id.clone()).or_default() += count;
            }
        }
        needs
    }

    /// A chat's questions were written: when a Vision drives it, its count of what waits on the
    /// person is kept, and a change is announced with the Vision so "Needs you" follows at once.
    pub(in crate::state) fn visions_questions_saved(
        &self,
        thread_id: &str,
        questions: &[Question],
    ) {
        if !self.visions_owns(thread_id) {
            return;
        }
        let waiting = waiting_on_person(questions);
        let before = self
            .visions_cache()
            .shown
            .waiting
            .insert(thread_id.to_owned(), waiting);
        if before.unwrap_or(0) == waiting {
            return;
        }
        if let Some(owner) = self
            .owner_of(thread_id)
            .filter(|o| o.hold != Hold::TakenOver)
        {
            self.visions_announce(&owner.vision_id);
        }
    }

    /// Announces `visions-changed` with the Vision only: the app reads the list again.
    fn visions_announce(&self, vision_id: &str) {
        if let Some(sink) = self.scheduler_sink() {
            sink.emit_visions_changed(&VisionsChange {
                channel_id: None,
                vision_id: Some(vision_id.to_owned()),
            });
        }
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
        let changed = {
            let sorry = &mut self.visions_cache().shown.sorry;
            match terminal {
                ThreadEvent::Failed { error }
                    if !error.starts_with(themis_core::runtime::STOPPED_BY_YOU) =>
                {
                    sorry.insert(owner.vision_id.clone())
                }
                ThreadEvent::Finished { .. } | ThreadEvent::Incomplete { .. } => {
                    sorry.remove(&owner.vision_id)
                }
                _ => false,
            }
        };
        // Announced once it is recorded, so the app's read sees it.
        if changed {
            self.visions_announce(&owner.vision_id);
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
        self.visions_announce(vision_id);
    }

    /// True the first time only: the minute re-check runs once however often the scheduler's
    /// sink is set.
    pub(crate) fn visions_recheck_loop_once(&self) -> bool {
        !self
            .inner
            .visions
            .recheck_loop
            .swap(true, std::sync::atomic::Ordering::SeqCst)
    }

    /// Judges every Vision's budget again, announcing those that flipped: run when prices
    /// become known or fail to load, and every minute (a raise in a file, the day's start).
    /// Nothing to do with no Visions or with Visions turned off in your runtime.jsonc.
    pub(crate) fn visions_budget_recheck(&self) {
        let visions = self.visions_list();
        let enabled = self
            .runtime_configuration(None, None)
            .map_or(true, |resolved| resolved.config.visions.enabled);
        if visions.is_empty() || !enabled {
            return;
        }
        for vision in visions {
            let over = self.visions_over_budget(&vision.id);
            self.visions_budget_shown(&vision.id, over);
        }
    }
}
