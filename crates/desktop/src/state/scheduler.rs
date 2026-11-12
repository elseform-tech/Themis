//! The automation scheduler: its background tick, its event sink and due-time arithmetic.

use super::*;

impl AppState {
    /// Spawns the background automation scheduler (production only).
    pub(super) fn spawn_automation_scheduler(state: AppState) {
        tauri::async_runtime::spawn(async move {
            loop {
                tokio::time::sleep(std::time::Duration::from_secs(SCHEDULER_TICK_SECS)).await;
                if let Some(sink) = state.scheduler_sink() {
                    // Not awaited: a slow check must not delay the next tick.
                    // `watch_inflight` keeps one pass per automation.
                    let state = state.clone();
                    tauri::async_runtime::spawn(async move {
                        state.tick_automations_once(&sink).await;
                    });
                }
            }
        });
    }

    /// Registers the sink the background scheduler emits on (production
    /// wires the Tauri sink right after construction).
    pub fn set_scheduler_sink(&self, sink: Arc<dyn EventSink>) {
        self.install_background_notifier(Arc::clone(&sink));
        // Goals and queued messages carry on after a restart.
        if let Ok(runtime) = tokio::runtime::Handle::try_current() {
            let state = self.clone();
            let boot_sink = Arc::clone(&sink);
            runtime.spawn(async move { state.resume_after_boot(boot_sink).await });
            let state = self.clone();
            let bank_sink = Arc::clone(&sink);
            runtime.spawn(async move {
                loop {
                    tokio::time::sleep(goal::BANK_INTERVAL).await;
                    state.bank_running_goals(bank_sink.as_ref());
                }
            });
            // A Vision's budget can flip with nothing else happening (the day's start, a cap
            // raised in a file): judged again every minute, a flip is announced.
            let state = self.clone();
            runtime.spawn(async move {
                loop {
                    tokio::time::sleep(std::time::Duration::from_secs(60)).await;
                    let state = state.clone();
                    let _ =
                        tokio::task::spawn_blocking(move || state.visions_budget_recheck()).await;
                }
            });
        }
        *self
            .inner
            .scheduler_sink
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner()) = Some(sink);
    }

    pub(super) fn scheduler_sink(&self) -> Option<Arc<dyn EventSink>> {
        self.inner
            .scheduler_sink
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .clone()
    }
}

/// Formats `now + minutes` as an RFC3339 UTC timestamp.
pub(super) fn rfc3339_plus_minutes(now: chrono::DateTime<Utc>, minutes: i64) -> String {
    (now + chrono::Duration::minutes(minutes)).to_rfc3339_opts(SecondsFormat::Secs, true)
}

/// True when `next_run_at` is due at `now`. Unparseable stamps count as due
/// (a corrupt schedule must fire visibly rather than stall silently).
pub(super) fn next_run_due(next_run_at: &str, now: chrono::DateTime<Utc>) -> bool {
    match DateTime::parse_from_rfc3339(next_run_at) {
        Ok(stamp) => stamp <= now,
        Err(_) => true,
    }
}
