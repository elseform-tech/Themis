//! Token usage: the recorder runs hand their requests to, and the summaries
//! Settings › Usage, the context meter and `themis usage` read.

use std::collections::HashMap;
use std::sync::Arc;

use themis_core::context_metrics::{UsageRecord, UsageRecorder};

use themis_core::providers::{GoModel, ModelPrice};

use super::AppState;
use crate::usage::{summarize, window_starts, Totals, UsageRow, UsageSummary};

/// List prices by (provider, model), as usage rows name them.
pub(super) type Prices = HashMap<(String, String), ModelPrice>;

/// The catalog's prices, keyed as usage rows name their models (a model without a provider is
/// OpenCode Go's).
pub(super) fn catalog_prices(models: &[GoModel]) -> Prices {
    models
        .iter()
        .filter_map(|model| {
            let provider = model
                .provider
                .clone()
                .unwrap_or_else(|| themis_core::providers::GO_PROVIDER_ID.to_owned());
            Some(((provider, model.id.clone()), model.price.clone()?))
        })
        .collect()
}

impl AppState {
    /// Records every completed request of one run (or goal check) for `thread_id`.
    pub(super) fn usage_recorder(&self, thread_id: &str, run_id: &str) -> UsageRecorder {
        let state = self.clone();
        let thread_id = thread_id.to_owned();
        let run_id = run_id.to_owned();
        Arc::new(move |record: UsageRecord| {
            state.inner.usage.record(UsageRow::from_record(
                record,
                chrono::Utc::now().timestamp_millis(),
                Some(&thread_id),
                Some(&run_id),
            ));
            // A Vision's chat: its daily budget is checked after every request.
            state.visions_budget_after_request(&thread_id);
        })
    }

    /// Totals for 5 hours, today, this week and this month, the 30-day chart and the
    /// breakdowns, in this computer's time zone.
    pub async fn get_usage_summary(&self) -> Result<UsageSummary, String> {
        // Prices come from the model catalog; load it once if no run has yet.
        let _ =
            tokio::time::timeout(std::time::Duration::from_secs(3), self.ensure_go_catalog()).await;
        let prices = catalog_prices(&self.inner.go_catalog.read().await);
        let titles: HashMap<String, String> = self
            .inner
            .threads
            .read()
            .await
            .values()
            .map(|record| (record.id.clone(), record.title.clone()))
            .collect();
        let now = chrono::Utc::now();
        let zone = chrono::Local;
        let since = window_starts(now, &zone).earliest();
        let usage = Arc::clone(&self.inner.usage);
        let rows = tokio::task::spawn_blocking(move || usage.rows_since(since))
            .await
            .map_err(|error| error.to_string())??;
        let mut summary = summarize(&rows, now, &zone, &prices, &titles);
        summary.available = self.inner.usage.available();
        Ok(summary)
    }

    /// Everything one chat used that Themis still keeps.
    pub async fn get_chat_usage(&self, thread_id: String) -> Result<Totals, String> {
        let usage = Arc::clone(&self.inner.usage);
        tokio::task::spawn_blocking(move || usage.chat_totals(&thread_id))
            .await
            .map_err(|error| error.to_string())?
    }
}
