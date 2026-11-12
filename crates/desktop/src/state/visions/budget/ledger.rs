//! Today's spend of every chat, kept as a running sum per chat: each check reads only the usage
//! rows recorded since the last one (by row id), so its work grows with the new requests, never
//! with the day's or history's. The sums start again at local midnight and are rebuilt from the
//! usage store after a restart.

use std::collections::{HashMap, HashSet};
use std::time::Instant;

use chrono::NaiveDate;

use super::super::super::usage::Prices;
use super::super::Owner;
use crate::usage::UsageRow;

/// Dollars and how many requests had no price.
#[derive(Debug, Default, Clone, Copy, PartialEq)]
pub(crate) struct Spend {
    pub usd: f64,
    pub unpriced: u64,
}

impl Spend {
    /// Adds one request at the catalog's list price; a model the catalog does not price is
    /// counted apart (a free model, priced at $0, is priced).
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

    fn join(&mut self, other: Spend) {
        self.usd += other.usd;
        self.unpriced += other.unpriced;
    }
}

/// Today's spend of one Vision and of all Visions.
#[derive(Debug, Default, Clone, Copy, PartialEq)]
pub(crate) struct Tally {
    pub vision: Spend,
    pub all: Spend,
}

/// Why today's spend is not known; the budget then counts as reached.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Unknown {
    UsageUnavailable,
    UsageUnreadable,
    OwnersUnreadable,
    CatalogNotLoaded,
    ConfigUnreadable,
    VisionGone,
}

impl Unknown {
    /// The reason's name in notices and logs.
    pub(crate) fn label(self) -> &'static str {
        match self {
            Self::UsageUnavailable => "usage_unavailable",
            Self::UsageUnreadable => "usage_unreadable",
            Self::OwnersUnreadable => "owners_unreadable",
            Self::CatalogNotLoaded => "catalog_not_loaded",
            Self::ConfigUnreadable => "config_unreadable",
            Self::VisionGone => "vision_gone",
        }
    }
}

/// The running sums, the notices posted today and the checks waiting, in the Visions cache.
#[derive(Debug, Default)]
pub(crate) struct Ledger {
    day: Option<NaiveDate>,
    last_id: i64,
    by_thread: HashMap<String, Spend>,
    /// Rows added to the sums so far (for the scale test).
    pub(super) rows_read: usize,
    /// The day and keys of the notices posted on it; read from the store on first use.
    pub(super) notified: Option<(String, HashSet<String>)>,
    /// Notices whose post failed, and when they may be tried again.
    pub(super) retry_after: HashMap<String, Instant>,
    /// Visions with a check waiting to run after a request.
    pub(super) scheduled: HashSet<String>,
    /// When loading the price catalog was last started.
    pub(super) catalog_tried: Option<Instant>,
}

impl Ledger {
    /// The row id to read after: on a new day the sums start again from nothing.
    pub(super) fn begin(&mut self, day: NaiveDate) -> i64 {
        if self.day != Some(day) {
            self.day = Some(day);
            self.last_id = 0;
            self.by_thread.clear();
        }
        self.last_id
    }

    /// Adds rows read after [`Self::begin`]'s id.
    pub(super) fn add(&mut self, rows: &[(i64, UsageRow)], prices: &Prices) {
        for (id, row) in rows {
            self.last_id = self.last_id.max(*id);
            self.rows_read += 1;
            if let Some(thread) = &row.thread_id {
                self.by_thread
                    .entry(thread.clone())
                    .or_default()
                    .add(row, prices);
            }
        }
    }

    /// Today's spend by owner: one lookup per chat that spent today.
    pub(super) fn tally(&self, owners: &HashMap<String, Owner>, vision_id: Option<&str>) -> Tally {
        let mut tally = Tally::default();
        for (thread, spend) in &self.by_thread {
            let Some(owner) = owners.get(thread) else {
                continue;
            };
            tally.all.join(*spend);
            if vision_id == Some(owner.vision_id.as_str()) {
                tally.vision.join(*spend);
            }
        }
        tally
    }
}
