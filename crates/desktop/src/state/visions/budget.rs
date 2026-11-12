//! Visions' daily budgets, enforced by the server.
//!
//! A Vision's spend today is what usage recorded since local midnight for the chats it owns:
//! its own Vision chats and its work chats (a chat the person took over included: usage rows
//! do not say who started a run). Prices are the catalog's list prices; a request without one
//! counts as $0 and is counted apart. A Vision is over budget when its own spend reaches its
//! cap (the profile's `daily_budget_usd`, which may only lower `visions.daily_budget_usd`) or
//! when all Visions together reach `visions.daily_budget_usd`.
//!
//! Over budget, a Vision wakes for nothing (what woke it waits for a later wake), its driving
//! calls are refused, its queued messages wait, a goal in its chats pauses, and at the end of
//! any of its runs its running chats are stopped (the stop holds nothing back). Its channels get
//! one `budget_reached` notice a day, remembered in `budget_notices.json` across restarts.
//! Raising either cap lets the next wake run. Logged as metadata only.

use std::collections::HashMap;
use std::path::Path;

use serde::{Deserialize, Serialize};
use serde_json::json;
use themis_core::configuration::VisionsConfig;

use super::super::usage::{catalog_prices, Prices};
use super::store::{read_store, write_store};
use super::{Hold, NoticeKind, Owner, Vision};
use crate::state::AppState;
use crate::usage::UsageRow;

/// The notices posted today, under the Visions directory.
const NOTICES_FILE: &str = "budget_notices.json";

/// Dollars and how many requests had no price.
#[derive(Debug, Default, Clone, Copy, PartialEq)]
pub(crate) struct Spend {
    pub usd: f64,
    pub unpriced: u64,
}

impl Spend {
    fn add(&mut self, row: &UsageRow, prices: &Prices) {
        match prices.get(&(row.provider.clone(), row.model.clone())) {
            Some(price) => {
                self.usd += price.request_cost(
                    row.input_tokens.unwrap_or(0),
                    row.cached_input_tokens.unwrap_or(0),
                    row.output_tokens.unwrap_or(0),
                );
            }
            None => self.unpriced += 1,
        }
    }
}

/// Today's spend of one Vision and of all Visions, and how many rows were looked at.
#[derive(Debug, Default, Clone, Copy, PartialEq)]
pub(crate) struct Tally {
    pub vision: Spend,
    pub all: Spend,
    pub examined: usize,
}

/// Sums `rows` by the owner of each row's chat: one look-up per row, whatever the number of
/// owned chats.
fn tally(
    rows: &[UsageRow],
    owners: &HashMap<String, Owner>,
    prices: &Prices,
    vision_id: Option<&str>,
) -> Tally {
    let mut tally = Tally {
        examined: rows.len(),
        ..Tally::default()
    };
    for row in rows {
        let Some(owner) = row.thread_id.as_deref().and_then(|id| owners.get(id)) else {
            continue;
        };
        tally.all.add(row, prices);
        if vision_id == Some(owner.vision_id.as_str()) {
            tally.vision.add(row, prices);
        }
    }
    tally
}

/// Why a Vision is over budget: which cap, and the requests without a price behind it.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct Over {
    pub overall: bool,
    pub unpriced: u64,
}

/// Which Visions were told today that they reached the budget.
#[derive(Debug, Default, Serialize, Deserialize)]
struct Notices {
    day: String,
    visions: Vec<String>,
}

/// The local day, as notices remember it.
fn today() -> String {
    chrono::Local::now().date_naive().to_string()
}

/// Local midnight, in milliseconds.
fn midnight_ms() -> i64 {
    let today = chrono::Local::now().date_naive();
    crate::usage::local_start(&chrono::Local, today).timestamp_millis()
}

/// `vision`'s cap: its profile's, never above the overall one.
fn own_cap(vision: &Vision, config: &VisionsConfig) -> f64 {
    vision
        .daily_budget_usd
        .map_or(config.daily_budget_usd, |cap| {
            cap.min(config.daily_budget_usd)
        })
}

impl AppState {
    /// Estimated dollars `vision_id` (or all Visions, with `None`) spent today on the chats
    /// they own, at list prices (a request without one counts as $0).
    pub fn visions_spent_today(&self, vision_id: Option<&str>) -> f64 {
        let tally = self.visions_tally(vision_id);
        match vision_id {
            Some(_) => tally.vision.usd,
            None => tally.all.usd,
        }
    }

    /// Whether `vision_id` reached its own cap or all Visions reached the overall one, by your
    /// own `runtime.jsonc` (a channel's project may lower it further).
    pub fn visions_over_budget(&self, vision_id: &str) -> bool {
        self.visions_over(vision_id, None).is_some()
    }

    /// Today's spend, from usage rows since local midnight only.
    pub(super) fn visions_tally(&self, vision_id: Option<&str>) -> Tally {
        let rows = match self.inner.usage.rows_since(midnight_ms()) {
            Ok(rows) => rows,
            Err(_) => {
                self.visions_note("budget_unreadable", json!({"source": "usage"}));
                Vec::new()
            }
        };
        let prices = self.prices_now();
        let mut cache = self.visions_cache();
        match cache.owners(&self.inner.visions_dir) {
            Ok(owners) => tally(&rows, owners, &prices, vision_id),
            Err(error) => {
                drop(cache);
                self.visions_failed(error);
                Tally::default()
            }
        }
    }

    /// The catalog's prices as they are now. The catalog is only ever written whole and
    /// briefly; if it stays locked, nothing has a price this time.
    fn prices_now(&self) -> Prices {
        for _ in 0..1000 {
            if let Ok(models) = self.inner.go_catalog.try_read() {
                return catalog_prices(&models);
            }
            std::thread::yield_now();
        }
        Prices::default()
    }

    /// Whether `vision_id` is over budget, by the configuration of `project` (or your own
    /// `runtime.jsonc`). A Vision that is gone, or a configuration or usage that cannot be read,
    /// counts as over: spending is the person's unless it is known to be within the cap.
    pub(super) fn visions_over(&self, vision_id: &str, project: Option<&Path>) -> Option<Over> {
        let config = self.runtime_configuration(project, None).map(|c| c.config);
        let (Some(vision), Ok(config)) = (self.vision_get(vision_id), config) else {
            return Some(Over {
                overall: false,
                unpriced: 0,
            });
        };
        if !self.inner.usage.available() {
            return Some(Over {
                overall: true,
                unpriced: 0,
            });
        }
        let tally = self.visions_tally(Some(vision_id));
        if tally.vision.usd >= own_cap(&vision, &config.visions) {
            return Some(Over {
                overall: false,
                unpriced: tally.vision.unpriced,
            });
        }
        (tally.all.usd >= config.visions.daily_budget_usd).then_some(Over {
            overall: true,
            unpriced: tally.all.unpriced,
        })
    }

    /// Whether `vision_id` is over budget (see [`Self::visions_over`]); the first time on a day
    /// its channels are told.
    pub(crate) fn visions_budget_reached(&self, vision_id: &str, project: Option<&Path>) -> bool {
        let Some(over) = self.visions_over(vision_id, project) else {
            return false;
        };
        if self.visions_first_notice_today(vision_id) {
            self.visions_notice_in_its_channels(
                vision_id,
                NoticeKind::BudgetReached,
                Some(over.unpriced),
            );
            self.visions_note(
                "budget_reached",
                json!({"vision_id": vision_id, "overall": over.overall, "unpriced_requests": over.unpriced}),
            );
        }
        true
    }

    /// Whether `thread_id` is a chat its Vision drives (not held by the person) and that Vision
    /// is over budget.
    pub(crate) fn visions_budget_holds(&self, thread_id: &str) -> bool {
        let Some(owner) = self.owner_of(thread_id).filter(|o| o.hold == Hold::None) else {
            return false;
        };
        let project = self.channel_get(&owner.channel_id).map(|c| c.project_root);
        self.visions_budget_reached(&owner.vision_id, project.as_deref())
    }

    /// A run of a chat a Vision owns ended: over budget, every chat of that Vision that runs
    /// for it (not one the person took over) is stopped the ordinary way, holding nothing back.
    pub(super) fn visions_budget_after_run(&self, thread_id: &str) {
        let Some(owner) = self.owner_of(thread_id) else {
            return;
        };
        // The run's last requests are counted.
        self.inner
            .usage
            .settle(std::time::Duration::from_millis(200));
        let project = self.channel_get(&owner.channel_id).map(|c| c.project_root);
        if !self.visions_budget_reached(&owner.vision_id, project.as_deref()) {
            return;
        }
        let vision_id = owner.vision_id;
        let stopped = self
            .visions_stop_owned(|owner| owner.vision_id == vision_id && owner.hold == Hold::None);
        if stopped > 0 {
            self.visions_note(
                "budget_stopped",
                json!({"vision_id": vision_id, "stopped": stopped}),
            );
        }
    }

    /// Whether `vision_id` has not been told yet today (it is then remembered, in the store).
    fn visions_first_notice_today(&self, vision_id: &str) -> bool {
        let day = today();
        let dir = self.inner.visions_dir.clone();
        // The cache's lock keeps two checks from both posting.
        let cache = self.visions_cache();
        let mut notices: Notices = read_store(&dir, NOTICES_FILE).unwrap_or_default();
        if notices.day != day {
            notices = Notices {
                day,
                visions: Vec::new(),
            };
        }
        if notices.visions.iter().any(|id| id == vision_id) {
            return false;
        }
        notices.visions.push(vision_id.to_owned());
        let saved = write_store(&dir, NOTICES_FILE, &notices);
        drop(cache);
        if let Err(error) = saved {
            self.visions_failed(error);
        }
        true
    }

    /// Prices to count spend by, for tests (the catalog otherwise comes from the provider).
    #[doc(hidden)]
    pub async fn set_model_catalog_for_test(&self, models: Vec<themis_core::providers::GoModel>) {
        *self.inner.go_catalog.write().await = models;
    }
}

#[cfg(test)]
mod tests;
