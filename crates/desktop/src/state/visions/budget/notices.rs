//! The budget's notices, each kind once a local day per Vision in each of its channels, and
//! loading the price catalog.
//!
//! A notice counts as given in a channel once that channel recorded it; the day's given notices
//! are then saved in `budget_notices.json`, so a restart does not post them again. A channel
//! that could not record one is tried again after [`RETRY_AFTER`] (only that channel); a notice
//! that was posted but could not be saved is kept in memory, so it is not posted again until a
//! restart. Without an OpenCode Go key, the catalog is not loaded and that is not a failure:
//! the configured providers' prices are the ones known.

use std::collections::HashSet;
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};
use serde_json::json;

use super::super::store::{read_store, write_store};
use super::super::{ChannelEntry, NoticeKind};
use super::{Over, Unknown};
use crate::state::AppState;

/// The notices given today, under the Visions directory.
const NOTICES_FILE: &str = "budget_notices.json";
/// How long after a failed post the same notice is tried again.
const RETRY_AFTER: Duration = Duration::from_secs(60);
/// How long after a try loading the price catalog is tried again.
const CATALOG_RETRY: Duration = Duration::from_secs(60);
/// Longest wait for the catalog to load.
const CATALOG_WAIT: Duration = Duration::from_secs(30);

/// The notices given on `day`, as `{kind}:{vision id}:{channel id}` keys.
#[derive(Debug, Default, Serialize, Deserialize)]
struct Given {
    day: String,
    keys: Vec<String>,
}

/// One notice to give.
struct Notice<'a> {
    kind: NoticeKind,
    key: String,
    unpriced: Option<u64>,
    reason: Option<&'a str>,
}

fn today() -> String {
    chrono::Local::now().date_naive().to_string()
}

impl AppState {
    /// The reached cap, or why spend cannot be known, once a day.
    pub(super) fn visions_notice_over(&self, vision_id: &str, over: Over) {
        let notice = match over {
            Over::Unknown(why) => Notice {
                kind: NoticeKind::BudgetUnknown,
                key: format!("budget_unknown_{}", why.label()),
                unpriced: None,
                reason: Some(why.label()),
            },
            Over::Vision { unpriced } | Over::Overall { unpriced } => Notice {
                kind: NoticeKind::BudgetReached,
                key: "budget_reached".to_owned(),
                unpriced: Some(unpriced),
                reason: None,
            },
        };
        if self.visions_notice_once(vision_id, &notice) {
            let event = match over {
                Over::Unknown(_) => "budget_unknown",
                _ => "budget_reached",
            };
            self.visions_note(
                event,
                json!({"vision_id": vision_id, "overall": matches!(over, Over::Overall { .. }),
                    "reason": notice.reason, "unpriced_requests": notice.unpriced}),
            );
        }
    }

    /// Today's first requests without a price, once a day.
    pub(super) fn visions_notice_unpriced(&self, vision_id: &str, unpriced: u64) {
        let notice = Notice {
            kind: NoticeKind::Unpriced,
            key: "unpriced".to_owned(),
            unpriced: Some(unpriced),
            reason: None,
        };
        if self.visions_notice_once(vision_id, &notice) {
            self.visions_note(
                "budget_unpriced",
                json!({"vision_id": vision_id, "unpriced_requests": unpriced}),
            );
        }
    }

    /// Posts `notice` in each of the Vision's channels that has not recorded it today; true
    /// when one recorded it now. A channel that could not record it is tried again after
    /// [`RETRY_AFTER`], and only that channel.
    fn visions_notice_once(&self, vision_id: &str, notice: &Notice<'_>) -> bool {
        let mut posted = false;
        let mut failed = false;
        for channel in self.channels_all() {
            if !channel.members.iter().any(|member| member == vision_id) {
                continue;
            }
            let key = format!("{}:{vision_id}:{}", notice.key, channel.id);
            if !self.visions_notice_claim(&key) {
                continue;
            }
            let entry = ChannelEntry::Notice {
                kind: notice.kind,
                vision_id: Some(vision_id.to_owned()),
                thread_id: None,
                unpriced_requests: notice.unpriced,
                reason: notice.reason.map(str::to_owned),
            };
            let recorded = self.channel_append(&channel.id, entry).is_ok();
            let mut cache = self.visions_cache();
            let budget = &mut cache.budget;
            if recorded {
                budget.retry_after.remove(&key);
                posted = true;
            } else {
                if let Some((_, keys)) = budget.notified.as_mut() {
                    keys.remove(&key);
                }
                budget.retry_after.insert(key, Instant::now() + RETRY_AFTER);
                failed = true;
            }
        }
        if failed {
            self.visions_note(
                "budget_notice_failed",
                json!({"vision_id": vision_id, "kind": notice.kind}),
            );
        }
        if posted {
            self.visions_notices_save(vision_id, notice.kind);
        }
        posted
    }

    /// Saves today's given notices; one that cannot be saved stays given in memory.
    fn visions_notices_save(&self, vision_id: &str, kind: NoticeKind) {
        let cache = self.visions_cache();
        let Some((day, keys)) = cache.budget.notified.as_ref() else {
            return;
        };
        let mut given: Vec<String> = keys.iter().cloned().collect();
        given.sort();
        let given = Given {
            day: day.clone(),
            keys: given,
        };
        let saved = write_store(&self.inner.visions_dir, NOTICES_FILE, &given);
        drop(cache);
        if saved.is_err() {
            self.visions_note(
                "budget_notices_unsaved",
                json!({"vision_id": vision_id, "kind": kind}),
            );
        }
    }

    /// Takes `key` for today when no notice of it was given or is being given, and no failed
    /// post waits to be tried again.
    fn visions_notice_claim(&self, key: &str) -> bool {
        let day = today();
        let dir = self.inner.visions_dir.clone();
        let mut cache = self.visions_cache();
        let budget = &mut cache.budget;
        if budget.notified.as_ref().is_none_or(|(on, _)| *on != day) {
            let given: Given = read_store(&dir, NOTICES_FILE).unwrap_or_default();
            let keys: HashSet<String> = if given.day == day {
                given.keys.into_iter().collect()
            } else {
                HashSet::new()
            };
            budget.notified = Some((day, keys));
            budget.retry_after.clear();
        }
        if budget
            .retry_after
            .get(key)
            .is_some_and(|at| Instant::now() < *at)
        {
            return false;
        }
        budget
            .notified
            .as_mut()
            .is_some_and(|(_, keys)| keys.insert(key.to_owned()))
    }

    /// Loads the price catalog on a task of its own, at most once a minute. When it cannot load
    /// (offline, no key), every Vision's channels are told why it starts nothing.
    pub(crate) fn visions_load_prices_soon(&self) {
        let Ok(runtime) = tokio::runtime::Handle::try_current() else {
            return;
        };
        {
            let mut cache = self.visions_cache();
            let tried = &mut cache.budget.catalog_tried;
            if tried.is_some_and(|at| at.elapsed() < CATALOG_RETRY) {
                return;
            }
            *tried = Some(Instant::now());
        }
        let state = self.clone();
        runtime.spawn(async move {
            // No key: the configured providers' prices are all there is (not a failure).
            let no_key = matches!(state.go_key_present().await, Ok(false));
            state.visions_cache().budget.no_go_key = no_key;
            if no_key {
                state.visions_note("budget_catalog", json!({"failure": "no_key"}));
                return;
            }
            let loaded = tokio::time::timeout(CATALOG_WAIT, state.ensure_go_catalog()).await;
            let failure = match loaded {
                Ok(Ok(())) if state.prices_loaded() => None,
                Ok(Ok(())) => Some("empty"),
                Ok(Err(_)) => Some("error"),
                Err(_) => Some("timeout"),
            };
            state.visions_note("budget_catalog", json!({"failure": failure}));
            if failure.is_some() {
                for vision in state.visions_list() {
                    state.visions_notice_over(&vision.id, Over::Unknown(Unknown::CatalogNotLoaded));
                }
            }
        });
    }

    fn prices_loaded(&self) -> bool {
        self.inner
            .go_catalog
            .try_read()
            .is_ok_and(|models| !models.is_empty())
    }

    /// At start: when there are Visions and no price catalog yet, load it.
    pub(crate) fn visions_prices_at_start(&self) {
        if !self.visions_list().is_empty() && !self.prices_loaded() {
            self.visions_load_prices_soon();
        }
    }
}
