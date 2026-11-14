//! Visions' daily budgets, enforced by the server. A budget is a channel's: its team's.
//!
//! A channel's spend today is what usage recorded since local midnight for the chats its agents
//! own there: their Vision chats and work chats (a chat the person took over included: usage rows
//! do not say who started a run). Prices are the catalog's list prices; a model the loaded
//! catalog does not price counts as $0 and is counted apart (a free model, priced at $0, is
//! priced). A channel is over budget when its spend reaches its cap (its `daily_budget_usd`,
//! never above the overall cap), when its project's channels together reach a lower cap the
//! project sets, or when all Visions together reach your `visions.daily_budget_usd`.
//! When spend cannot be known (the usage store, `owners.json` or the configuration unreadable,
//! the price catalog not loaded yet) the budget counts as reached, and the notice says why.
//!
//! The cap is checked after every request recorded for a chat a Vision owns (on its own task,
//! once the usage writer stored the row), at the end of each such run, and before anything a
//! Vision would start. Over budget, a Vision wakes for nothing (what woke it waits for a later
//! wake), its spending calls are refused, its queued messages wait, a goal in its chats pauses,
//! and its running chats are stopped (every Vision's, at the overall cap) the ordinary way,
//! holding nothing back; each stopped work chat waits as a `budget_stopped` wake reason. Its
//! channels get each kind of notice once a local day (`notices`). Raising a cap lets the next
//! wake run. Logged as metadata only.

mod ledger;
mod notices;

use std::time::{Duration, Instant};

use serde_json::json;

use super::super::usage::{catalog_prices, Prices};
use super::{Hold, OwnedKind, Owner, WakeReason};
use crate::state::AppState;
pub(crate) use ledger::{Ledger, Tally, Unknown};

/// Longest wait, after a request, for the usage writer to store its row before the check reads.
const SETTLE: Duration = Duration::from_millis(200);

/// Why a Vision may not spend now in a channel.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) enum Over {
    /// Its channel's cap; with the requests behind it that had no price.
    Channel { unpriced: u64 },
    /// A shared cap: your own `runtime.jsonc`'s, reached by all Visions together, or a lower
    /// one a project sets, reached by the Visions with a channel there.
    Overall { unpriced: u64 },
    /// Spend cannot be known, so it counts as reached.
    Unknown(Unknown),
}

impl Over {
    /// The wake outcome it gives.
    pub(crate) fn outcome(self) -> &'static str {
        match self {
            Self::Unknown(Unknown::PricesPending) => "prices_pending",
            Self::Unknown(_) => "budget_unknown",
            _ => "over_budget",
        }
    }

    /// What a refused Vision call is told.
    pub(crate) fn refusal_text(self) -> &'static str {
        match self {
            Self::Unknown(_) => themis_core::prompts::AGENT_CLI_VISION_BUDGET_UNKNOWN,
            _ => themis_core::prompts::AGENT_CLI_VISION_OVER_BUDGET,
        }
    }

    /// Why a goal in its chat paused, as the goal bar shows it after "Paused".
    pub(crate) fn goal_reason(self) -> &'static str {
        match self {
            Self::Unknown(_) => "its Vision's spending cannot be checked right now",
            _ => "its Vision reached today's budget",
        }
    }
}

/// A Vision's budget now: whether it may not spend, and its requests today without a price.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct Judged {
    pub over: Option<Over>,
    pub unpriced: u64,
}

/// The caps that apply to one channel.
struct Caps {
    channel: f64,
    /// Your own `runtime.jsonc`'s, for all Visions together.
    user: f64,
    /// A lower cap the channel's project sets, and that project's channels.
    project: Option<(f64, Vec<String>)>,
}

impl Caps {
    /// The cap `tally` reached for `channel_id`, the shared ones first (reaching one stops
    /// every channel under it).
    fn reached(&self, tally: &Tally, channel_id: &str) -> Option<Over> {
        if tally.all.usd >= self.user {
            return Some(Over::Overall {
                unpriced: tally.all.unpriced,
            });
        }
        if let Some((cap, channels)) = &self.project {
            let mut group = ledger::Spend::default();
            for channel in channels {
                if let Some(spend) = tally.by_channel.get(channel) {
                    group.join(*spend);
                }
            }
            if group.usd >= *cap {
                return Some(Over::Overall {
                    unpriced: group.unpriced,
                });
            }
        }
        let spend = tally
            .by_channel
            .get(channel_id)
            .copied()
            .unwrap_or_default();
        (spend.usd >= self.channel).then_some(Over::Channel {
            unpriced: spend.unpriced,
        })
    }
}

/// The prices of the configured providers' models, under the provider's id and both the
/// model's own id and its qualified `<provider>/<model>` id.
fn configured_prices(providers: &[themis_core::configuration::ProviderDefinition]) -> Prices {
    let mut prices = Prices::default();
    for provider in providers {
        for model in provider.models.iter().flatten() {
            let Some(price) = &model.price else {
                continue;
            };
            let price = themis_core::providers::ModelPrice {
                input: price.input_per_mtok,
                output: price.output_per_mtok,
                cache_read: price.cached_input_per_mtok,
                tiers: Vec::new(),
            };
            for id in [model.id.clone(), provider.qualified(&model.id)] {
                prices.insert((provider.id.clone(), id), price.clone());
            }
        }
    }
    prices
}

/// Local midnight, in milliseconds.
fn midnight_ms(today: chrono::NaiveDate) -> i64 {
    crate::usage::local_start(&chrono::Local, today).timestamp_millis()
}

impl AppState {
    /// Estimated dollars `vision_id` (or all Visions, with `None`) spent today on the chats
    /// they own, at list prices; infinite when it cannot be known.
    pub fn visions_spent_today(&self, vision_id: Option<&str>) -> f64 {
        match (self.visions_tally(vision_id), vision_id) {
            (Ok(tally), Some(_)) => tally.vision.usd,
            (Ok(tally), None) => tally.all.usd,
            (Err(_), _) => f64::INFINITY,
        }
    }

    /// Whether `vision_id` may not spend now in any of its channels (its profile shows so).
    pub fn visions_over_budget(&self, vision_id: &str) -> bool {
        self.channels_all()
            .iter()
            .filter(|channel| channel.members.iter().any(|member| member == vision_id))
            .any(|channel| self.visions_judge(vision_id, &channel.id).over.is_some())
    }

    /// Today's spend, reading only the usage rows recorded since the last check. The usage
    /// store is read without the Visions cache's lock held.
    pub(super) fn visions_tally(&self, vision_id: Option<&str>) -> Result<Tally, Unknown> {
        if !self.inner.usage.available() {
            return Err(Unknown::UsageUnavailable);
        }
        let (mut prices, catalog) = self.prices_now()?;
        let today = chrono::Local::now().date_naive();
        let after = self.visions_cache().budget.begin(today);
        let rows = self
            .inner
            .usage
            .rows_after(after, midnight_ms(today))
            .map_err(|_| Unknown::UsageUnreadable)?;
        // The catalog's prices are looked up under its lock, and only when there is a new row.
        if catalog && !rows.is_empty() {
            prices.extend(self.with_catalog(catalog_prices).unwrap_or_default());
        }
        let dir = self.inner.visions_dir.clone();
        let mut guard = self.visions_cache();
        guard.budget.begin(today);
        guard.budget.add(&rows, &prices);
        if let Err(error) = guard.owners(&dir) {
            drop(guard);
            self.visions_failed(error);
            return Err(Unknown::OwnersUnreadable);
        }
        let cache = &*guard;
        let owners = cache.owners.as_ref().ok_or(Unknown::OwnersUnreadable)?;
        Ok(cache.budget.tally(owners, vision_id))
    }

    /// The configured providers' prices (`providers[].models[].price` in your `runtime.jsonc`),
    /// and whether the catalog's are there too. Without the catalog, spend is unknown, unless
    /// the last try to load it found no OpenCode Go key: the configured prices are then all there
    /// is; before any try finished, prices are pending.
    fn prices_now(&self) -> Result<(Prices, bool), Unknown> {
        let config = self
            .runtime_configuration(None, None)
            .map_err(|_| Unknown::ConfigUnreadable)?;
        let prices = configured_prices(&config.config.providers);
        match self.with_catalog(|models| !models.is_empty()) {
            Some(true) => Ok((prices, true)),
            _ => {
                self.visions_load_prices_soon();
                match self.visions_cache().budget.go_key {
                    Some(false) => Ok((prices, false)),
                    Some(true) => Err(Unknown::CatalogNotLoaded),
                    None => Err(Unknown::PricesPending),
                }
            }
        }
    }

    /// `read` of the catalog under its lock, or None while it is being written.
    fn with_catalog<T>(
        &self,
        read: impl FnOnce(&[themis_core::providers::GoModel]) -> T,
    ) -> Option<T> {
        for _ in 0..1000 {
            if let Ok(models) = self.inner.go_catalog.try_read() {
                return Some(read(&models));
            }
            std::thread::yield_now();
        }
        None
    }

    /// `channel`'s caps: its own (never above your own `runtime.jsonc`'s
    /// `visions.daily_budget_usd`), that one for all Visions together, and a lower cap its
    /// project sets, for that project's channels.
    fn visions_caps(&self, channel: &super::Channel) -> Result<Caps, Unknown> {
        let resolve = |project: Option<&std::path::Path>| {
            self.runtime_configuration(project, None)
                .map(|config| config.config.visions.daily_budget_usd)
                .map_err(|_| Unknown::ConfigUnreadable)
        };
        let user = resolve(None)?;
        let cap = resolve(Some(&channel.project_root))?;
        let project = (cap < user).then(|| {
            let channels = self
                .channels_all()
                .into_iter()
                .filter(|other| other.project_root == channel.project_root)
                .map(|other| other.id)
                .collect();
            (cap, channels)
        });
        Ok(Caps {
            channel: channel.daily_budget_usd.map_or(user, |own| own.min(user)),
            user,
            project,
        })
    }

    /// Whether `vision_id` may not spend now in `channel_id`, posting nothing.
    pub(super) fn visions_judge(&self, vision_id: &str, channel_id: &str) -> Judged {
        let unknown = |why| Judged {
            over: Some(Over::Unknown(why)),
            unpriced: 0,
        };
        if self.vision_get(vision_id).is_none() {
            return unknown(Unknown::VisionGone);
        }
        let Some(channel) = self.channel_get(channel_id) else {
            return unknown(Unknown::VisionGone);
        };
        let caps = match self.visions_caps(&channel) {
            Ok(caps) => caps,
            Err(why) => return unknown(why),
        };
        let tally = match self.visions_tally(Some(vision_id)) {
            Ok(tally) => tally,
            Err(why) => return unknown(why),
        };
        Judged {
            over: caps.reached(&tally, channel_id),
            unpriced: tally
                .by_channel
                .get(channel_id)
                .map_or(0, |spend| spend.unpriced),
        }
    }

    /// Whether `vision_id` may not spend now in `channel_id`; the channel is told once a day,
    /// under its chief, of the reached cap, of spend that cannot be known, and of requests
    /// without a price.
    pub(crate) fn visions_budget_gate(&self, vision_id: &str, channel_id: &str) -> Option<Over> {
        let judged = self.visions_judge(vision_id, channel_id);
        if judged.unpriced > 0 {
            self.visions_notice_unpriced(channel_id, judged.unpriced);
        }
        match judged.over {
            // Loading the catalog says so itself when it fails; until then it may just be early,
            // and what waits is woken again once the prices are known.
            Some(Over::Unknown(Unknown::CatalogNotLoaded | Unknown::PricesPending)) | None => {}
            Some(over) => self.visions_notice_over(channel_id, over),
        }
        self.visions_budget_shown(vision_id, self.visions_over_budget(vision_id));
        judged.over
    }

    /// Why `thread_id`'s Vision may not spend in it now, when its Vision drives it (the person
    /// holds nothing there).
    pub(crate) fn visions_budget_holds(&self, thread_id: &str) -> Option<Over> {
        let owner = self.owner_of(thread_id).filter(|o| o.hold == Hold::None)?;
        self.visions_budget_gate(&owner.vision_id, &owner.channel_id)
    }

    /// Stops the running chats the budget no longer allows: `channel_id`'s, and at a shared cap
    /// those of every channel that is over too. Only chats that run for a Vision (not those the
    /// person took over), the ordinary way, holding nothing back; each stopped work chat waits
    /// as `budget_stopped` with its cause.
    pub(super) fn visions_budget_enforce(&self, vision_id: &str, channel_id: &str) {
        let Some(over) = self.visions_budget_gate(vision_id, channel_id) else {
            return;
        };
        let mut over_ids = vec![channel_id.to_owned()];
        if matches!(over, Over::Overall { .. }) {
            over_ids.extend(
                self.channels_all()
                    .into_iter()
                    .filter(|channel| channel.id != channel_id)
                    .filter(|channel| {
                        channel
                            .members
                            .iter()
                            .any(|member| self.visions_judge(member, &channel.id).over.is_some())
                    })
                    .map(|channel| channel.id),
            );
        }
        let stopped: Vec<Owner> = self
            .owners_where(|o| o.hold == Hold::None && over_ids.contains(&o.channel_id))
            .into_iter()
            .filter(|owner| self.stop_run(&owner.thread_id).is_ok())
            .collect();
        if stopped.is_empty() {
            return;
        }
        let cause = match over {
            Over::Unknown(_) => "unknown",
            _ => "reached",
        };
        let mut cache = self.visions_cache();
        for owner in stopped.iter().filter(|o| o.kind == OwnedKind::Work) {
            let reason = WakeReason::BudgetStopped(owner.thread_id.clone(), cause);
            cache
                .wakes
                .wait(&owner.vision_id, &owner.channel_id, Some(reason));
        }
        drop(cache);
        self.visions_note(
            "budget_stopped",
            json!({"channel_id": channel_id, "stopped": stopped.len(), "channels": over_ids.len(), "cause": cause}),
        );
    }

    /// A run of a chat a Vision owns ended: the cap is checked again (the requests' own checks
    /// usually caught it already).
    pub(super) fn visions_budget_after_run(&self, thread_id: &str) {
        if let Some(owner) = self.owner_of(thread_id) {
            self.visions_budget_enforce(&owner.vision_id, &owner.channel_id);
        }
    }

    /// A request of `thread_id` was recorded: when a Vision owns the chat, its channel's cap is
    /// checked on a task of its own, at most one waiting per channel.
    pub(in crate::state) fn visions_budget_after_request(&self, thread_id: &str) {
        // Most chats: one read lock and a set lookup, no Visions lock and no SQLite.
        if !self.visions_owns(thread_id) {
            return;
        }
        let Some(owner) = self.owner_of(thread_id) else {
            return;
        };
        let Ok(runtime) = tokio::runtime::Handle::try_current() else {
            return;
        };
        let (vision_id, channel_id) = (owner.vision_id, owner.channel_id);
        if !self
            .visions_cache()
            .budget
            .scheduled
            .insert(channel_id.clone())
        {
            return;
        }
        let state = self.clone();
        runtime.spawn(async move {
            // The row reaches SQLite on the usage writer's own thread; this waits for it
            // (bounded) without holding a worker, so the check below counts it.
            let started = Instant::now();
            while state.inner.usage.pending() > 0 && started.elapsed() < SETTLE {
                tokio::time::sleep(Duration::from_millis(2)).await;
            }
            state.visions_cache().budget.scheduled.remove(&channel_id);
            let _ = tokio::task::spawn_blocking(move || {
                state.visions_budget_enforce(&vision_id, &channel_id)
            })
            .await;
        });
    }
}

#[cfg(test)]
mod tests;
